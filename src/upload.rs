use std::fs;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::routes::{create_book_from_file, ApiResult, BookSummary};
use crate::session::SessionToken;
use crate::AppState;

const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_SIZE: u64 = 200 * 1024 * 1024;
const EXTS: [&str; 4] = ["epub", "txt", "md", "pdf"];

#[derive(Serialize, Deserialize)]
struct Meta {
    filename: String,
    size: u64,
    ext: String,
    session: String,
    created: u64,
}

#[derive(Deserialize)]
pub struct InitRequest {
    filename: String,
    size: u64,
}

#[derive(Serialize)]
pub struct InitResponse {
    upload_id: String,
    chunk_size: usize,
    total_chunks: u32,
}

pub async fn init(
    State(state): State<AppState>,
    Extension(session): Extension<SessionToken>,
    Json(body): Json<InitRequest>,
) -> ApiResult<Json<InitResponse>> {
    let ext = body
        .filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    if !EXTS.contains(&ext.as_str()) {
        return Err(anyhow::anyhow!("仅支持 epub / txt / md / pdf 格式").into());
    }
    if body.size == 0 {
        return Err(anyhow::anyhow!("文件为空").into());
    }
    if body.size > MAX_SIZE {
        return Err(anyhow::anyhow!("文件超过 200MB 限制").into());
    }

    sweep_stale(&state);

    let id = nanoid::nanoid!(12);
    let dir = upload_dir(&state, &id);
    fs::create_dir_all(&dir)?;
    let meta = Meta {
        filename: body.filename,
        size: body.size,
        ext,
        session: session.0,
        created: now(),
    };
    fs::write(dir.join("meta.json"), serde_json::to_vec(&meta)?)?;

    Ok(Json(InitResponse {
        upload_id: id,
        chunk_size: CHUNK_SIZE,
        total_chunks: total_chunks(body.size),
    }))
}

#[derive(Deserialize)]
pub struct ChunkQuery {
    index: u32,
}

/// 上传单个分片（幂等，可重试）
pub async fn chunk(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ChunkQuery>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let meta = load_meta(&state, &id)?;
    let total = total_chunks(meta.size);
    if q.index >= total {
        return Err(anyhow::anyhow!("分片序号越界").into());
    }
    if body.len() > CHUNK_SIZE {
        return Err(anyhow::anyhow!("分片过大").into());
    }
    fs::write(upload_dir(&state, &id).join(format!("chunk_{:06}", q.index)), &body)?;
    let received = chunk_indexes(&state, &id).len();
    Ok(Json(serde_json::json!({ "received": received, "total": total })))
}

/// 断点续传：查询已收到的分片序号
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let meta = load_meta(&state, &id)?;
    Ok(Json(serde_json::json!({
        "indexes": chunk_indexes(&state, &id),
        "chunk_size": CHUNK_SIZE,
        "total": total_chunks(meta.size),
    })))
}

/// 校验分片到齐 → 拼装 → 解析入库
pub async fn complete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<BookSummary>> {
    let meta = load_meta(&state, &id)?;
    let dir = upload_dir(&state, &id);
    let total = total_chunks(meta.size);
    let have = chunk_indexes(&state, &id);
    if have.len() != total as usize {
        return Err(anyhow::anyhow!("分片未传完（{}/{total}）", have.len()).into());
    }

    let assembled = dir.join("assembled");
    {
        use std::io::Write;
        let mut out = fs::File::create(&assembled)?;
        for i in 0..total {
            let mut f = fs::File::open(dir.join(format!("chunk_{i:06}")))?;
            std::io::copy(&mut f, &mut out)?;
        }
        out.flush()?;
    }
    if fs::metadata(&assembled)?.len() != meta.size {
        return Err(anyhow::anyhow!("文件大小校验失败，请重新上传").into());
    }

    let stem = meta
        .filename
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| meta.filename.clone());
    let summary = create_book_from_file(
        &state,
        &SessionToken(meta.session.clone()),
        &meta.ext,
        &stem,
        &assembled,
    )
    .await?;

    let _ = fs::remove_dir_all(&dir);
    Ok(Json(summary))
}

// ---------- 工具 ----------

fn upload_dir(state: &AppState, id: &str) -> PathBuf {
    state.data_dir.join("tmp").join("uploads").join(id)
}

fn load_meta(state: &AppState, id: &str) -> ApiResult<Meta> {
    let data = fs::read(upload_dir(state, id).join("meta.json"))
        .map_err(|_| anyhow::anyhow!("上传会话不存在或已过期"))?;
    Ok(serde_json::from_slice(&data)?)
}

fn chunk_indexes(state: &AppState, id: &str) -> Vec<u32> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(upload_dir(state, id)) {
        for e in entries.flatten() {
            if let Some(idx) = e
                .file_name()
                .to_string_lossy()
                .strip_prefix("chunk_")
                .and_then(|s| s.parse::<u32>().ok())
            {
                out.push(idx);
            }
        }
    }
    out.sort();
    out
}

fn total_chunks(size: u64) -> u32 {
    size.div_ceil(CHUNK_SIZE as u64) as u32
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 清理超过 24h 未完成的上传
fn sweep_stale(state: &AppState) {
    let Ok(entries) = fs::read_dir(state.data_dir.join("tmp").join("uploads")) else { return };
    let cutoff = now().saturating_sub(24 * 3600);
    for e in entries.flatten() {
        let Ok(data) = fs::read(e.path().join("meta.json")) else { continue };
        let Ok(meta) = serde_json::from_slice::<Meta>(&data) else { continue };
        if meta.created < cutoff {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}
