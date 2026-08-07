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
        Ok(duration_ms) => {
            sqlx::query(
                "UPDATE chapters SET status = 'done', audio_path = ?, duration_ms = ?, error = NULL WHERE id = ?",
            )
            .bind(out_path.to_string_lossy().as_ref())
            .bind(duration_ms as i64)
            .bind(chapter_id)
            .execute(&state.db)
            .await?;
            sqlx::query("UPDATE jobs SET status = 'done', progress = total WHERE id = ?")
                .bind(job_id)
                .execute(&state.db)
                .await?;
            tracing::info!("chapter {chapter_id} synthesized, {duration_ms}ms");
        }
        Err(e) => {
            let msg = truncate(&format!("{e:#}"), 500);
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
