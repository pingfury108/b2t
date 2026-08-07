use std::path::Path;

use anyhow::{bail, Result};
use regex::Regex;

use super::{merge_tiny_chapters, ParsedBook, ParsedChapter};

pub fn parse(path: &Path, filename_stem: &str) -> Result<ParsedBook> {
    let bytes = std::fs::read(path)?;
    let text = decode_text(&bytes);
    split_chapters(&text, filename_stem)
}

/// Markdown 文件：按标题切章，正文清洗为纯文本
pub fn parse_markdown(path: &Path, filename_stem: &str) -> Result<ParsedBook> {
    let bytes = std::fs::read(path)?;
    let text = decode_text(&bytes);
    Ok(super::split_markdown_chapters(&text, filename_stem))
}

/// 供 pdf 等纯文本兜底复用
pub fn split_chapters(text: &str, fallback_title: &str) -> Result<ParsedBook> {
    let text = text.replace("\r\n", "\n");
    let re = chapter_heading_regex();
    let matches: Vec<_> = re.find_iter(&text).collect();

    let mut chapters: Vec<ParsedChapter> = Vec::new();
    if matches.len() >= 2 && matches.len() < 2000 {
        for (i, m) in matches.iter().enumerate() {
            let body_start = m.end();
            let body_end = matches.get(i + 1).map(|n| n.start()).unwrap_or(text.len());
            let body = text[body_start..body_end].trim().to_string();
            chapters.push(ParsedChapter {
                title: m.as_str().trim().to_string(),
                text: body,
                level: 0,
            });
        }
    } else {
        chapters.push(ParsedChapter {
            title: "全文".into(),
            text: text.trim().to_string(),
            level: 0,
        });
    }

    let chapters = merge_tiny_chapters(chapters, 30);
    if chapters.is_empty() || chapters.iter().all(|c| c.text.is_empty()) {
        bail!("未能提取到正文内容");
    }
    Ok(ParsedBook {
        title: fallback_title.to_string(),
        chapters,
    })
}

fn chapter_heading_regex() -> Regex {
    Regex::new(
        r"(?m)^[ \t]*(第[0-9零一二三四五六七八九十百千万两]+[章节卷部回集篇][^\n]{0,50}|楔子|序章|序言|前言|尾声|终章|后记|番外[^\n]{0,30}|Chapter\s+[0-9IVXL]+[^\n]{0,60}|Prologue|Epilogue)[ \t]*$",
    )
    .unwrap()
}

fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            // 中文 txt 常见 GB18030 编码
            let (s, _, _) = encoding_rs::GB18030.decode(bytes);
            s.into_owned()
        }
    }
}
