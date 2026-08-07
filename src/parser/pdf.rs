use std::path::Path;

use anyhow::{bail, Context, Result};
use pdf_inspector::{MarkdownOptions, PdfOptions};

use super::{split_markdown_chapters, ParsedBook};

pub fn parse(path: &Path, filename_stem: &str) -> Result<ParsedBook> {
    // include_page_numbers：插入 <!-- Page N --> 分页标记，供按页兜屈切章
    let opts = PdfOptions::new().markdown(MarkdownOptions {
        include_page_numbers: true,
        ..MarkdownOptions::default()
    });
    let result = pdf_inspector::process_pdf_with_options(path.to_string_lossy().as_ref(), opts)
        .context("PDF 解析失败")?;

    use pdf_inspector::PdfType;
    match result.pdf_type {
        PdfType::TextBased | PdfType::Mixed => {}
        _ => bail!("暂不支持扫描版/图片型 PDF（无文本层）"),
    }

    let markdown = result.markdown.context("PDF 未提取到文本内容")?;
    Ok(split_markdown_chapters(&markdown, filename_stem))
}
