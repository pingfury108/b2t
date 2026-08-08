use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::process::Command;

/// ffprobe 获取音频时长（毫秒）
pub async fn ffprobe_duration_ms(path: &Path) -> Result<u64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .await
        .context("无法执行 ffprobe")?;
    if !out.status.success() {
        bail!("ffprobe 失败: {}", String::from_utf8_lossy(&out.stderr));
    }
    let secs: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0.0);
    Ok((secs * 1000.0) as u64)
}

/// ffmpeg concat 无损合并多个 mp3
pub async fn concat_mp3(inputs: &[PathBuf], output: &Path) -> Result<()> {
    let list = concat_list(inputs, output)?;
    run_ffmpeg(&[
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.to_string_lossy().into_owned(),
        "-codec".into(),
        "copy".into(),
        output.to_string_lossy().into_owned(),
    ])
    .await?;
    let _ = std::fs::remove_file(&list);
    Ok(())
}

/// 生成 ffmetadata（含章节信息）
pub fn build_ffmetadata(title: &str, chapters: &[(String, u64)]) -> String {
    let mut s = String::from(";FFMETADATA1\n");
    s.push_str(&format!("title={}\n", escape_meta(title)));
    let mut start = 0u64;
    for (name, dur) in chapters {
        let end = start + dur;
        s.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={start}\nEND={end}\ntitle={}\n",
            escape_meta(name)
        ));
        start = end;
    }
    s
}

/// 合并为带章节标记的单文件。m4b 转 AAC，mp3 直接 copy
pub async fn export_with_chapters(
    title: &str,
    inputs: &[PathBuf],
    chapters: &[(String, u64)],
    output: &Path,
    m4b: bool,
) -> Result<()> {
    let list = concat_list(inputs, output)?;
    let meta_path = output.with_extension("ffmeta");
    std::fs::write(&meta_path, build_ffmetadata(title, chapters))?;

    let mut args = vec![
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.to_string_lossy().into_owned(),
        "-i".into(),
        meta_path.to_string_lossy().into_owned(),
        "-map_metadata".into(),
        "1".into(),
        "-map".into(),
        "0:a".into(),
    ];
    if m4b {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "64k".into(), "-f".into(), "mp4".into()]);
    } else {
        args.extend(["-codec".into(), "copy".into()]);
    }
    args.push(output.to_string_lossy().into_owned());

    let result = run_ffmpeg(&args).await;
    let _ = std::fs::remove_file(&list);
    let _ = std::fs::remove_file(&meta_path);
    result
}

/// 生成指定时长的静音 mp3（占位用）
pub async fn silent_mp3(out: &Path, seconds: f64) -> Result<()> {
    run_ffmpeg(&[
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "anullsrc=r=24000:cl=mono".into(),
        "-t".into(),
        format!("{seconds}"),
        "-q:a".into(),
        "9".into(),
        out.to_string_lossy().into_owned(),
    ])
    .await
}

fn concat_list(inputs: &[PathBuf], near: &Path) -> Result<PathBuf> {
    let list = near.with_extension("concat.txt");
    // ffmpeg concat demuxer 会把 list 内相对路径相对于 list 所在目录解析，
    // 必须写成绝对路径，否则跨目录引用（如 tmp/audio）会找不到
    let cwd = std::env::current_dir()?;
    let content = inputs
        .iter()
        .map(|p| {
            let abs = if p.is_absolute() {
                p.clone()
            } else {
                cwd.join(p)
            };
            format!("file '{}'", abs.to_string_lossy().replace('\'', "'\\''"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&list, content)?;
    Ok(list)
}

async fn run_ffmpeg(args: &[String]) -> Result<()> {
    let out = Command::new("ffmpeg")
        .args(args)
        .output()
        .await
        .context("无法执行 ffmpeg")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("ffmpeg 失败: {}", stderr.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>());
    }
    Ok(())
}

fn escape_meta(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\\n")
        .replace('=', "\\=")
        .replace(';', "\\;")
        .replace('#', "\\#")
}
