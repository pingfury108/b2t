pub mod epub;
pub mod pdf;
pub mod txt;

use std::path::Path;

use anyhow::{bail, Result};

#[derive(Debug)]
pub struct ParsedBook {
    pub title: String,
    pub chapters: Vec<ParsedChapter>,
}

#[derive(Debug)]
pub struct ParsedChapter {
    pub title: String,
    pub text: String,
    pub level: usize,
}

pub fn parse_book(path: &Path, ext: &str, filename_stem: &str) -> Result<ParsedBook> {
    match ext {
        "epub" => epub::parse(path),
        "txt" => txt::parse(path, filename_stem),
        "md" => txt::parse_markdown(path, filename_stem),
        "pdf" => pdf::parse(path, filename_stem),
        _ => bail!("不支持的格式: {ext}（支持 epub / txt / md / pdf）"),
    }
}

/// 按 markdown 一二级标题切章；识别不到标题时回退到 txt 的章节正则
pub(crate) fn split_markdown_chapters(markdown: &str, fallback_title: &str) -> ParsedBook {
    let mut chapters: Vec<ParsedChapter> = Vec::new();
    let mut buf: Vec<&str> = Vec::new();

    for line in markdown.lines() {
        if matches!(heading_level(line), Some(1 | 2)) && !buf.is_empty() {
            push_md_chapter(&mut chapters, &buf);
            buf.clear();
        }
        buf.push(line);
    }
    if !buf.is_empty() {
        push_md_chapter(&mut chapters, &buf);
    }

    let mut chapters = merge_tiny_chapters(chapters, 30);
    // 层级归一化：最小标题深度作为 0 级
    if let Some(min) = chapters.iter().map(|c| c.level).min() {
        for c in chapters.iter_mut() {
            c.level -= min;
        }
    }
    if chapters.len() >= 2 {
        return ParsedBook {
            title: fallback_title.to_string(),
            chapters,
        };
    }
    // 降级 2：「第X章」正则切章
    if let Ok(book) = txt::split_chapters(&markdown_to_plain(markdown), fallback_title) {
        if book.chapters.len() >= 2 {
            return book;
        }
    }
    // 降级 3：目录不可靠时按页分组（依赖 <!-- Page N --> 标记）
    if let Some(book) = split_by_pages(markdown, fallback_title, 8) {
        return book;
    }
    ParsedBook {
        title: fallback_title.to_string(),
        chapters,
    }
}

/// 按页分组成章：每 `pages_per_chapter` 页一章
fn split_by_pages(markdown: &str, fallback_title: &str, pages_per_chapter: usize) -> Option<ParsedBook> {
    let re = regex::Regex::new(r"(?m)^\s*<!--\s*Page\s+(\d+)\s*-->\s*$").unwrap();
    let marks: Vec<_> = re.captures_iter(markdown).collect();
    if marks.len() < 2 {
        return None;
    }

    // (页码, 该页文本段)：第一个标记前的卷首文本归入前一页
    let mut pages: Vec<(u32, &str)> = Vec::new();
    let first_no: u32 = marks[0][1].parse().ok()?;
    let head = markdown[..marks[0].get(0)?.start()].trim();
    if !head.is_empty() {
        pages.push((first_no.saturating_sub(1).max(1), head));
    }
    for (i, m) in marks.iter().enumerate() {
        let no: u32 = m[1].parse().ok()?;
        let start = m.get(0)?.end();
        let end = marks
            .get(i + 1)
            .and_then(|n| n.get(0))
            .map(|g| g.start())
            .unwrap_or(markdown.len());
        let text = markdown[start..end].trim();
        if !text.is_empty() {
            pages.push((no, text));
        }
    }

    let mut chapters = Vec::new();
    for chunk in pages.chunks(pages_per_chapter) {
        let text = markdown_to_plain(
            &chunk
                .iter()
                .map(|(_, t)| *t)
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        if text.is_empty() {
            continue;
        }
        let (start, end) = (chunk[0].0, chunk[chunk.len() - 1].0);
        let title = if start == end {
            format!("第 {start} 页")
        } else {
            format!("第 {start}–{end} 页")
        };
        chapters.push(ParsedChapter { title, text, level: 0 });
    }
    if chapters.len() < 2 {
        return None;
    }
    Some(ParsedBook {
        title: fallback_title.to_string(),
        chapters,
    })
}

fn push_md_chapter(chapters: &mut Vec<ParsedChapter>, lines: &[&str]) {
    let mut title = String::new();
    let mut level = 1usize;
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        if title.is_empty() && heading_level(line).is_some() {
            title = line.trim_start_matches('#').trim().to_string();
            level = heading_level(line).unwrap_or(1);
            continue;
        }
        body.push(line);
    }
    if title.is_empty() {
        title = format!("第 {} 节", chapters.len() + 1);
    }
    chapters.push(ParsedChapter {
        title,
        text: markdown_to_plain(&body.join("\n")),
        level,
    });
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
        Some(hashes)
    } else {
        None
    }
}

/// markdown → 纯文本（供阅读与 TTS）
pub fn markdown_to_plain(md: &str) -> String {
    use regex::Regex;
    let re_heading = Regex::new(r"^#{1,6}\s+").unwrap();
    let re_list = Regex::new(r"^\s*(?:[-*+]|\d+[.、])\s+").unwrap();
    let re_hr = Regex::new(r"^\s*(?:-{3,}|_{3,}|\*{3,})\s*$").unwrap();
    let re_table_sep = Regex::new(r"^\|?[\s:|\-]+\|[\s:|\-]*$").unwrap();
    let re_blank = Regex::new(r"\n{3,}").unwrap();
    let re_comment = Regex::new(r"<!--.*?-->").unwrap();

    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in md.lines() {
        let t = line.trim_end();
        if t.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || re_hr.is_match(t) || re_table_sep.is_match(t) {
            continue;
        }
        if t.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut s = re_comment.replace_all(t, "").to_string();
        s = re_heading.replace(&s, "").to_string();
        s = re_list.replace(&s, "").to_string();
        s = s.trim_start_matches('>').trim_start().to_string();
        // 表格行 → 单元格空格连接
        if s.contains('|') {
            s = s.split('|')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join("  ");
        }
        out.push(inline_clean(&s));
    }
    re_blank
        .replace_all(&out.join("\n"), "\n\n")
        .trim()
        .to_string()
}

fn inline_clean(s: &str) -> String {
    use regex::Regex;
    let re_link = Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap();
    let re_bold = Regex::new(r"\*\*([^*]+)\*\*|__([^_]+)__").unwrap();
    let re_del = Regex::new(r"~~([^~]+)~~").unwrap();
    let re_code = Regex::new(r"`([^`]*)`").unwrap();
    let s = re_link.replace_all(s, "$1");
    let s = re_bold.replace_all(&s, "$1$2");
    let s = re_del.replace_all(&s, "$1");
    let s = re_code.replace_all(&s, "$1");
    s.replace("**", "").replace("__", "")
}

/// 过短的章节合并进前一章（通常是封面、版权页等）
pub fn merge_tiny_chapters(chapters: Vec<ParsedChapter>, min_chars: usize) -> Vec<ParsedChapter> {
    let mut out: Vec<ParsedChapter> = Vec::new();
    for ch in chapters {
        let len = ch.text.chars().count();
        if len < min_chars {
            if let Some(prev) = out.last_mut() {
                prev.text.push('\n');
                prev.text.push_str(&ch.text);
                continue;
            }
        }
        out.push(ch);
    }
    out
}
