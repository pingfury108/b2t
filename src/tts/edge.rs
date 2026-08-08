use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::audio;
use super::Progress;

const MAX_SEGMENT_CHARS: usize = 1000;
const FALLBACK_SEGMENT_CHARS: usize = 300;
/// 单 worker 串行处理章节；章节内也一次一个片段，避免触发 edge-tts 限流
const MAX_CONCURRENCY: usize = 1;
const RETRY: usize = 3;
/// 跳过段落占比超过该值才判整章失败
const MAX_SKIP_RATIO: f64 = 0.2;

pub struct SynthesisResult {
    pub duration_ms: u64,
    pub skipped: usize,
    pub total: usize,
}

pub struct EdgeTts {
    pub voice: String,
}

impl EdgeTts {
    pub fn new(voice: &str) -> Self {
        Self {
            voice: voice.to_string(),
        }
    }

    /// 长文本分段合成 + 合并。失败段降级（细分重试）后仍失败则静音占位跳过，
    /// 跳过占比在阈值内返回部分成功，避免整章重来。
    pub async fn synthesize_long(
        &self,
        text: &str,
        work_dir: &Path,
        output: &Path,
        progress: Arc<Progress>,
    ) -> Result<SynthesisResult> {
        let segments = segment_text(text, MAX_SEGMENT_CHARS);
        if segments.is_empty() {
            bail!("章节内容为空");
        }
        let total = segments.len();
        std::fs::create_dir_all(work_dir)?;
        tracing::debug!(
            "segment split: {total} segments (max {MAX_SEGMENT_CHARS} chars), voice {}",
            self.voice
        );

        // 静音占位（跳过段落处保持时间线完整）
        let silent = work_dir.join("silent.mp3");
        audio::silent_mp3(&silent, 0.5).await?;

        // 阶段一：并发合成全部段，失败不中断，只收集结果
        let mut slots: Vec<Option<PathBuf>> = vec![None; total];
        {
            let sem = Arc::new(Semaphore::new(MAX_CONCURRENCY));
            let mut set: JoinSet<(usize, Result<PathBuf>)> = JoinSet::new();
            for (i, seg) in segments.iter().enumerate() {
                let seg_path = work_dir.join(format!("seg_{i:05}.mp3"));
                let voice = self.voice.clone();
                let text = seg.clone();
                let sem = sem.clone();
                let progress = progress.clone();
                set.spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    let r = synthesize_segment(&voice, &text, &seg_path).await;
                    if r.is_ok() {
                        progress.incr();
                        tracing::debug!("segment {i} done");
                    }
                    (i, r.map(|_| seg_path))
                });
            }
            while let Some(res) = set.join_next().await {
                match res {
                    Ok((i, Ok(p))) => slots[i] = Some(p),
                    Ok((i, Err(e))) => {
                        tracing::warn!("segment {i} synthesize failed: {e:#}");
                        slots[i] = None;
                    }
                    Err(e) => {
                        tracing::warn!("segment task panicked: {e:#}");
                    }
                }
            }
            let ok_count = slots.iter().filter(|s| s.is_some()).count();
            tracing::debug!("stage1 done: {ok_count}/{total} segments ok");
        }

        // 阶段二：失败段按更小粒度细分重试，仍失败则静音占位
        let failed: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_none())
            .map(|(i, _)| i)
            .collect();
        if !failed.is_empty() {
            tracing::debug!(
                "stage2: retrying {} failed segments at {FALLBACK_SEGMENT_CHARS} chars",
                failed.len()
            );
        }
        let mut skipped = 0usize;
        for i in failed {
            let slot = &mut slots[i];
            let sub_segs = segment_text(&segments[i], FALLBACK_SEGMENT_CHARS);
            let mut parts = Vec::new();
            let mut ok = true;
            for (j, sub) in sub_segs.iter().enumerate() {
                let sub_path = work_dir.join(format!("seg_{i:05}_{j:05}.mp3"));
                if let Err(e) = synthesize_segment(&self.voice, sub, &sub_path).await {
                    tracing::warn!("segment {i} fallback part {j} failed: {e:#}");
                    parts.clear();
                    ok = false;
                    break;
                }
                parts.push(sub_path);
            }
            if ok && !parts.is_empty() {
                let combined = work_dir.join(format!("seg_{i:05}.mp3"));
                if parts.len() == 1 {
                    std::fs::rename(&parts[0], &combined)?;
                } else {
                    audio::concat_mp3(&parts, &combined).await?;
                }
                progress.incr();
                *slot = Some(combined);
            } else {
                tracing::warn!("segment {i} skipped (replaced with silence)");
                skipped += 1;
                *slot = Some(silent.clone());
            }
        }

        if skipped as f64 / total as f64 > MAX_SKIP_RATIO {
            bail!("生成失败段落过多（{skipped}/{total}），请重试");
        }

        let seg_files: Vec<PathBuf> = slots.into_iter().flatten().collect();
        tracing::debug!("concatenating {} segments -> {}", seg_files.len(), output.display());
        if seg_files.len() == 1 {
            std::fs::rename(&seg_files[0], output)?;
        } else {
            audio::concat_mp3(&seg_files, output).await?;
        }

        let duration = audio::ffprobe_duration_ms(output).await.unwrap_or(0);
        let _ = std::fs::remove_dir_all(work_dir);
        tracing::debug!("synthesized, duration {duration}ms, skipped {skipped}/{total}");
        Ok(SynthesisResult {
            duration_ms: duration,
            skipped,
            total,
        })
    }
}

async fn synthesize_segment(voice: &str, text: &str, out: &Path) -> Result<()> {
    let text = clean_text(text);
    if text.trim().is_empty() {
        bail!("段落为空（清理后无有效字符）");
    }
    let mut last_err = String::new();
    for attempt in 1..=RETRY {
        let result = Command::new("edge-tts")
            .arg("--voice")
            .arg(voice)
            .arg("--text")
            .arg(&text)
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
                // Python traceback 的异常信息在 stderr 末尾，取尾部才有用
                let stderr = String::from_utf8_lossy(&o.stderr);
                last_err = stderr.chars().rev().take(300).collect::<String>().chars().rev().collect();
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

/// 清理会破坏 edge-tts/argv 的字符：NUL、C0 控制字符（保留 \t\n\r）、孤立代理
fn clean_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let cp = *c as u32;
            !(cp == 0
                || (0x01..=0x08).contains(&cp)
                || cp == 0x0b
                || cp == 0x0c
                || (0x0e..=0x1f).contains(&cp)
                || (0xd800..=0xdfff).contains(&cp))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_bad_chars() {
        // Rust String 无法含 surrogate（char::from_u32 拒绝代理区），主要防 NUL 与控制字符
        let s = "a\0b\x00c\t\n\u{1c}";
        let c = clean_text(&s);
        assert_eq!(c, "abc\t\n");
    }
}
