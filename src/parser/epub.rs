use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use anyhow::{bail, Context, Result};
use zip::ZipArchive;

use super::{merge_tiny_chapters, ParsedBook, ParsedChapter};

struct ManifestItem {
    href: String,
    media_type: String,
    properties: String,
}

pub fn parse(path: &Path) -> Result<ParsedBook> {
    let bytes = std::fs::read(path)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("无效的 EPUB 文件")?;

    let container = read_zip_str(&mut zip, "META-INF/container.xml").context("缺少 container.xml")?;
    let opf_path = parse_container(&container).context("解析 container.xml 失败")?;
    let opf_dir = opf_path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();

    let opf = read_zip_str(&mut zip, &opf_path).context("缺少 OPF 文件")?;
    let (title, manifest, spine) = parse_opf(&opf, &opf_dir)?;

    // href(解析后) -> (章节标题, 层级)
    let mut toc_titles: HashMap<String, (String, usize)> = HashMap::new();

    if let Some(item) = manifest
        .values()
        .find(|i| i.media_type == "application/x-dtbncx+xml")
    {
        if let Some(ncx) = read_zip_str(&mut zip, &item.href) {
            for (href, t, level) in parse_ncx(&ncx) {
                toc_titles.insert(resolve(&opf_dir, &item.href, &href), (t, level));
            }
        }
    }
    if let Some(item) = manifest.values().find(|i| i.properties.split(' ').any(|p| p == "nav")) {
        if let Some(nav) = read_zip_str(&mut zip, &item.href) {
            for (href, t, level) in parse_nav(&nav) {
                toc_titles
                    .entry(resolve(&opf_dir, &item.href, &href))
                    .or_insert((t, level));
            }
        }
    }

    let mut chapters = Vec::new();
    for (i, idref) in spine.iter().enumerate() {
        let Some(item) = manifest.get(idref) else { continue };
        if !item.media_type.contains("html") {
            continue;
        }
        let Some(content) = read_zip_str(&mut zip, &item.href) else { continue };
        let text = html_to_text(content.as_bytes());
        let (title, level) = toc_titles
            .get(&item.href)
            .cloned()
            .unwrap_or_else(|| (format!("第 {} 节", i + 1), 0));
        chapters.push(ParsedChapter { title, text, level });
    }

    let chapters = merge_tiny_chapters(chapters, 30);
    if chapters.is_empty() || chapters.iter().all(|c| c.text.is_empty()) {
        bail!("未能从 EPUB 提取到正文");
    }
    tracing::debug!(
        "epub: spine={} items, toc={} entries, {} chapters after merge",
        spine.len(),
        toc_titles.len(),
        chapters.len()
    );
    Ok(ParsedBook { title, chapters })
}

fn html_to_text(bytes: &[u8]) -> String {
    // plain_no_decorate：不输出 markdown 装饰（#/##/**）与链接脚注
    let text = html2text::config::plain_no_decorate()
        .string_from_read(bytes, 2000)
        .unwrap_or_default();
    // 兜底：清理可能残留的装饰与链接脚注定义行
    let text = super::markdown_to_plain(&text);

    let re_footnote_def = regex::Regex::new(r"(?m)^\s*\[\d+\]:\s+\S+\s*$").unwrap();
    let text = re_footnote_def.replace_all(&text, "");
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn read_zip_str(zip: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Option<String> {
    let mut file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn parse_container(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(|s| s.to_string())
}

fn parse_opf(
    xml: &str,
    opf_dir: &str,
) -> Result<(String, HashMap<String, ManifestItem>, Vec<String>)> {
    let doc = roxmltree::Document::parse(xml).context("解析 OPF 失败")?;

    let title = doc
        .descendants()
        .find(|n| n.tag_name().name() == "title")
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "未命名书籍".into());

    let mut manifest = HashMap::new();
    for n in doc.descendants().filter(|n| n.tag_name().name() == "item") {
        let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) else { continue };
        manifest.insert(
            id.to_string(),
            ManifestItem {
                href: normalize(&format!("{opf_dir}/{href}")),
                media_type: n.attribute("media-type").unwrap_or_default().to_string(),
                properties: n.attribute("properties").unwrap_or_default().to_string(),
            },
        );
    }

    let spine = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "itemref")
        .filter(|n| n.attribute("linear") != Some("no"))
        .filter_map(|n| n.attribute("idref").map(str::to_string))
        .collect();

    Ok((title, manifest, spine))
}

fn parse_ncx(xml: &str) -> Vec<(String, String, usize)> {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return vec![] };
    let mut out = Vec::new();
    for nav_map in doc.descendants().filter(|n| n.tag_name().name() == "navMap") {
        walk_nav_points(nav_map, 0, &mut out);
    }
    out
}

fn walk_nav_points(node: roxmltree::Node, depth: usize, out: &mut Vec<(String, String, usize)>) {
    for child in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "navPoint")
    {
        let label = child
            .descendants()
            .find(|d| d.tag_name().name() == "text")
            .and_then(|d| d.text())
            .unwrap_or_default()
            .trim()
            .to_string();
        let src = child
            .descendants()
            .find(|d| d.tag_name().name() == "content")
            .and_then(|d| d.attribute("src"))
            .unwrap_or_default()
            .to_string();
        if !label.is_empty() && !src.is_empty() {
            out.push((src, label, depth));
        }
        walk_nav_points(child, depth + 1, out);
    }
}

fn parse_nav(xhtml: &str) -> Vec<(String, String, usize)> {
    let Ok(doc) = roxmltree::Document::parse(xhtml) else { return vec![] };
    doc.descendants()
        .filter(|n| n.tag_name().name() == "a")
        .filter_map(|n| {
            let href = n.attribute("href")?.to_string();
            let label = n.text().unwrap_or_default().trim().to_string();
            // 嵌套 li 层数即目录深度
            let depth = n
                .ancestors()
                .filter(|a| a.tag_name().name() == "li")
                .count()
                .saturating_sub(1);
            if label.is_empty() { None } else { Some((href, label, depth)) }
        })
        .collect()
}

/// 基于 opf 目录和引用文件位置解析相对 href，去掉 fragment
fn resolve(opf_dir: &str, from_file: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or_default();
    if href.is_empty() {
        return from_file.to_string();
    }
    let base = if href.starts_with('/') {
        ""
    } else {
        from_file.rsplit_once('/').map(|(d, _)| d).unwrap_or(opf_dir)
    };
    normalize(&format!("{base}/{href}"))
}

fn normalize(path: &str) -> String {
    let decoded = percent_decode(path);
    let mut stack: Vec<&str> = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            p => stack.push(p),
        }
    }
    stack.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
