use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::time::{sleep, Duration};

use crate::models::Chapter;
use crate::tts::{edge::EdgeTts, Progress};
use crate::AppState;

/// 单 worker 轮询 SQLite 任务队列（edge-tts 不宜高并发）
pub fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            match next_pending_job(&state.db).await {
                Ok(Some(job)) => {
                    if let Err(e) = run_job(&state, job.0, job.1).await {
                        tracing::error!("job {} failed: {e:#}", job.0);
                    }
                }
                Ok(None) => sleep(Duration::from_millis(800)).await,
                Err(e) => {
                    tracing::error!("job poll error: {e:#}");
                    sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });
}

/// 启动时恢复被中断的任务：running 的 job 与 synthesizing 的章节重置回
/// pending，并清理孤儿工作目录。若音频其实已完成但崩在写库前，会重新合成（宁可重跑）。
pub async fn recover_interrupted(db: &SqlitePool, data_dir: &std::path::Path) -> anyhow::Result<()> {
    let running: Vec<i64> = sqlx::query_as("SELECT id FROM jobs WHERE status = 'running'")
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(id,)| id)
        .collect();
    if !running.is_empty() {
        tracing::info!("recover {} interrupted job(s): {running:?}", running.len());
        sqlx::query("UPDATE jobs SET status = 'pending', error = NULL WHERE status = 'running'")
            .execute(db)
            .await?;
        sqlx::query(
            "UPDATE chapters SET status = 'pending', error = NULL WHERE status = 'synthesizing'",
        )
        .execute(db)
        .await?;
    }
    // 重启后没有活跃任务，tmp/job_* 均为孤儿目录，直接清理
    let tmp = data_dir.join("tmp");
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("job_") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(())
}

async fn next_pending_job(db: &SqlitePool) -> anyhow::Result<Option<(i64, i64)>> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, chapter_id FROM jobs WHERE status = 'pending' ORDER BY id LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if let Some((id, _)) = row {
        sqlx::query("UPDATE jobs SET status = 'running' WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(row)
}

async fn run_job(state: &AppState, job_id: i64, chapter_id: i64) -> anyhow::Result<()> {
    let chapter: Chapter = sqlx::query_as("SELECT * FROM chapters WHERE id = ?")
        .bind(chapter_id)
        .fetch_one(&state.db)
        .await?;
    let voice: (String,) = sqlx::query_as("SELECT voice FROM books WHERE id = ?")
        .bind(&chapter.book_id)
        .fetch_one(&state.db)
        .await?;
    tracing::debug!(
        "job {job_id} start, chapter {chapter_id} ({} chars), voice {}",
        chapter.text.chars().count(),
        voice.0
    );

    sqlx::query("UPDATE chapters SET status = 'synthesizing', error = NULL WHERE id = ?")
        .bind(chapter_id)
        .execute(&state.db)
        .await?;

    let out_dir = state.data_dir.join("audio").join(&chapter.book_id);
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(format!("{chapter_id}.mp3"));
    let work_dir = state.data_dir.join("tmp").join(format!("job_{job_id}"));

    let progress = Arc::new(Progress::new(0));
    let watcher = {
        let db = state.db.clone();
        let progress = progress.clone();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(2)).await;
                let (done, total) = progress.snapshot();
                let _ = sqlx::query("UPDATE jobs SET progress = ?, total = ? WHERE id = ?")
                    .bind(done as i64)
                    .bind(total as i64)
                    .bind(job_id)
                    .execute(&db)
                    .await;
            }
        })
    };

    let tts = EdgeTts::new(&voice.0);
    let result = tts
        .synthesize_long(&chapter.text, &work_dir, &out_path, progress)
        .await;
    watcher.abort();

    match result {
        Ok(sr) => {
            // 有跳过段落时标记 partial（部分完成），error 列存提示信息
            let status = if sr.skipped > 0 { "partial" } else { "done" };
            let err_msg = if sr.skipped > 0 {
                Some(format!("{} 段生成失败已跳过（共 {} 段）", sr.skipped, sr.total))
            } else {
                None
            };
            sqlx::query(
                "UPDATE chapters SET status = ?, audio_path = ?, duration_ms = ?, error = ? WHERE id = ?",
            )
            .bind(status)
            .bind(out_path.to_string_lossy().as_ref())
            .bind(sr.duration_ms as i64)
            .bind(err_msg)
            .bind(chapter_id)
            .execute(&state.db)
            .await?;
            sqlx::query("UPDATE jobs SET status = 'done', progress = total WHERE id = ?")
                .bind(job_id)
                .execute(&state.db)
                .await?;
            tracing::info!(
                "chapter {chapter_id} synthesized, {}ms, skipped {}/{}",
                sr.duration_ms,
                sr.skipped,
                sr.total
            );
            if sr.skipped > 0 {
                tracing::warn!(
                    "chapter {chapter_id} partial: {} segments skipped",
                    sr.skipped
                );
            }
        }
        Err(e) => {
            let msg = truncate(&format!("{e:#}"), 500);
            tracing::error!("chapter {chapter_id} synthesize failed: {msg}");
            sqlx::query("UPDATE chapters SET status = 'error', error = ? WHERE id = ?")
                .bind(&msg)
                .bind(chapter_id)
                .execute(&state.db)
                .await?;
            sqlx::query("UPDATE jobs SET status = 'error', error = ? WHERE id = ?")
                .bind(&msg)
                .bind(job_id)
                .execute(&state.db)
                .await?;
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
