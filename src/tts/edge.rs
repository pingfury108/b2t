use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::audio;
use super::Progress;

const MAX_SEGMENT_CHARS: usize = 2000;
const MAX_CONCURRENCY: usize = 3;
const RETRY: usize = 3;

pub struct EdgeTts {
    pub voice: String,
}

impl EdgeTts {
    pub fn new(voice: &str) -> Self {
        Self {
            voice: voice.to_string(),
        }
    }

    /// 长文本分段合成 + 合并，返回音频时长（毫秒）
    pub async fn synthesize_long(
        &self,
        text: &str,
        work_dir: &Path,
        output: &Path,
        progress: Arc<Progress>,
    ) -> Result<u64> {
        let segments = segment_text(text, MAX_SEGMENT_CHARS);
        if segments.is_empty() {
            bail!("章节内容为空");
        }

        std::fs::create_dir_all(work_dir)?;
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENCY));
        let mut set: JoinSet<Result<PathBuf>> = JoinSet::new();

        for (i, seg) in segments.iter().enumerate() {
            let seg_path = work_dir.join(format!("seg_{i:05}.mp3"));
            let voice = self.voice.clone();
            let text = seg.clone();
            let sem = sem.clone();
            let progress = progress.clone();
            set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                synthesize_segment(&voice, &text, &seg_path).await?;
                progress.incr();
                Ok(seg_path)
            });
        }

        let mut seg_files: Vec<PathBuf> = Vec::with_capacity(segments.len());
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(p)) => seg_files.push(p),
                Ok(Err(e)) => {
                    set.abort_all();
                    return Err(e);
                }
                Err(e) => {
                    set.abort_all();
                    return Err(e).context("TTS 任务异常");
                }
            }
        }
        seg_files.sort();

        if seg_files.len() == 1 {
            std::fs::rename(&seg_files[0], output)?;
        } else {
            audio::concat_mp3(&seg_files, output).await?;
        }

        let duration = audio::ffprobe_duration_ms(output).await.unwrap_or(0);
        let _ = std::fs::remove_dir_all(work_dir);
        Ok(duration)
    }
}

async fn synthesize_segment(voice: &str, text: &str, out: &Path) -> Result<()> {
    let mut last_err = String::new();
    for attempt in 1..=RETRY {
        let result = Command::new("edge-tts")
            .arg("--voice")
            .arg(voice)
            .arg("--text")
            .arg(text)
            .arg("--write-media")
            .arg(out)
            .output()
            .await;

        match result {
            Ok(o) if o.status.success() => {
                let ok = std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false);
                if ok {
                    return Ok(());
                }
                last_err = "edge-tts 产物为空".into();
            }
            Ok(o) => {
                last_err = String::from_utf8_lossy(&o.stderr).chars().take(300).collect();
            }
            Err(e) => {
                last_err = format!("无法执行 edge-tts: {e}");
                break;
            }
        }
        if attempt < RETRY {
            tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
        }
    }
    bail!("edge-tts 合成失败: {last_err}")
}

/// 按换行/标点切分为不超过 max_chars 的段落
pub fn segment_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for part in split_keep(text) {
        let part_len = part.chars().count();
        let cur_len = current.chars().count();
        if part_len > max_chars {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            // 超长单行硬切
            let mut chunk = String::new();
            for ch in part.chars() {
                chunk.push(ch);
                if chunk.chars().count() >= max_chars {
                    segments.push(std::mem::take(&mut chunk));
                }
            }
            current = chunk;
        } else if cur_len + part_len > max_chars {
            segments.push(std::mem::take(&mut current));
            current = part;
        } else {
            current.push_str(&part);
        }
    }
    if !current.trim().is_empty() {
        segments.push(current);
    }
    segments
}

/// 按换行和句末标点切分，保留分隔符
fn split_keep(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if ch == '\n' || matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?') {
            parts.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_respects_limit() {
        let text = "这是第一句。".repeat(500); // 3000 chars
        let segs = segment_text(&text, 2000);
        assert!(segs.len() >= 2);
        for s in &segs {
            assert!(s.chars().count() <= 2000);
        }
    }
}
