use std::io::Write;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Extension, Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::license::{self, RedeemOutcome};
use crate::models::{Book, Chapter, Job};
use crate::parser;
use crate::session::SessionToken;
use crate::AppState;

pub struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::warn!("api error: {:#}", self.0);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{:#}", self.0) })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ---------- 书籍 ----------

#[derive(Serialize)]
pub struct BookSummary {
    pub id: String,
    pub title: String,
    pub source_ext: String,
    pub chapter_count: i64,
    pub done_count: i64,
    pub licensed: bool,
    pub created_at: String,
}

pub async fn upload_book(
    State(state): State<AppState>,
    Extension(session): Extension<SessionToken>,
    mut multipart: Multipart,
) -> ApiResult<Json<BookSummary>> {
    let mut file_name = String::new();
    let mut data = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(anyhow::Error::from)? {
        if field.name() == Some("file") {
            file_name = field.file_name().unwrap_or("book").to_string();
            data = field.bytes().await.map_err(anyhow::Error::from)?.to_vec();
            break;
        }
    }
    if data.is_empty() {
        return Err(anyhow::anyhow!("未收到文件").into());
    }

    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    if !["epub", "txt", "md", "pdf"].contains(&ext.as_str()) {
        return Err(anyhow::anyhow!("仅支持 epub / txt / md / pdf 格式").into());
    }

    // 写入临时文件，走统一创建流程
    let tmp = state
        .data_dir
        .join("tmp")
        .join(format!("upload_{}", nanoid::nanoid!(8)));
    std::fs::write(&tmp, &data)?;
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or(file_name);
    let summary = create_book_from_file(&state, &session, &ext, &stem, &tmp).await?;
    let _ = std::fs::remove_file(&tmp);
    Ok(Json(summary))
}

/// 解析文件并创建书籍（multipart 与分片上传共用）
pub(crate) async fn create_book_from_file(
    state: &AppState,
    session: &SessionToken,
    ext: &str,
    stem: &str,
    src: &std::path::Path,
) -> ApiResult<BookSummary> {
    let book_id = nanoid::nanoid!(10);
    let upload_path = state.data_dir.join("uploads").join(format!("{book_id}.{ext}"));
    std::fs::copy(src, &upload_path)?;

    let parsed = tokio::task::spawn_blocking({
        let path = upload_path.clone();
        let ext = ext.to_string();
        let stem = stem.to_string();
        move || parser::parse_book(&path, &ext, &stem)
    })
    .await??;

    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO books (id, session_token, title, source_ext) VALUES (?, ?, ?, ?)")
        .bind(&book_id)
        .bind(&session.0)
        .bind(&parsed.title)
        .bind(ext)
        .execute(&mut *tx)
        .await?;
    for (i, ch) in parsed.chapters.iter().enumerate() {
        sqlx::query("INSERT INTO chapters (book_id, idx, title, text, char_count, level) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&book_id)
            .bind(i as i64)
            .bind(&ch.title)
            .bind(&ch.text)
            .bind(ch.text.chars().count() as i64)
            .bind(ch.level as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(BookSummary {
        id: book_id,
        title: parsed.title,
        source_ext: ext.to_string(),
        chapter_count: parsed.chapters.len() as i64,
        done_count: 0,
        licensed: false,
        created_at: String::new(),
    })
}

pub async fn list_books(
    State(state): State<AppState>,
    Extension(session): Extension<SessionToken>,
) -> ApiResult<Json<Vec<BookSummary>>> {
    let books: Vec<Book> = sqlx::query_as(
        "SELECT * FROM books WHERE session_token = ? ORDER BY created_at DESC",
    )
    .bind(&session.0)
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::new();
    for b in books {
        let (total, done): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(status = 'done'), 0) FROM chapters WHERE book_id = ?",
        )
        .bind(&b.id)
        .fetch_one(&state.db)
        .await?;
        out.push(BookSummary {
            licensed: license::book_licensed(&state.db, &b.id).await?,
            id: b.id,
            title: b.title,
            source_ext: b.source_ext,
            chapter_count: total,
            done_count: done,
            created_at: b.created_at,
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct ChapterView {
    pub id: i64,
    pub idx: i64,
    pub level: i64,
    pub title: String,
    pub char_count: i64,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub locked: bool,
}

#[derive(Serialize)]
pub struct BookDetail {
    pub id: String,
    pub title: String,
    pub source_ext: String,
    pub voice: String,
    pub free_count: i64,
    pub licensed: bool,
    pub chapters: Vec<ChapterView>,
}

pub async fn book_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<BookDetail>> {
    let book: Book = sqlx::query_as("SELECT * FROM books WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| anyhow::anyhow!("书籍不存在"))?;
    let chapters: Vec<Chapter> =
        sqlx::query_as("SELECT * FROM chapters WHERE book_id = ? ORDER BY idx")
            .bind(&id)
            .fetch_all(&state.db)
            .await?;
    let licensed = license::book_licensed(&state.db, &id).await?;

    Ok(Json(BookDetail {
        id: book.id,
        title: book.title,
        source_ext: book.source_ext,
        voice: book.voice,
        free_count: book.free_count,
        licensed,
        chapters: chapters
            .into_iter()
            .map(|c| ChapterView {
                locked: c.idx >= book.free_count && !licensed,
                id: c.id,
                idx: c.idx,
                level: c.level,
                title: c.title,
                char_count: c.char_count,
                status: c.status,
                duration_ms: c.duration_ms,
                error: c.error,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateBook {
    voice: Option<String>,
}

/// 删除书籍：仅限上传者本人（session 匹配）；授权码记录保留不退回
pub async fn delete_book(
    State(state): State<AppState>,
    Extension(session): Extension<SessionToken>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let book: Book = sqlx::query_as("SELECT * FROM books WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| anyhow::anyhow!("书籍不存在"))?;
    if book.session_token != session.0 {
        return Err(anyhow::anyhow!("只能删除自己上传的书").into());
    }
    // 已绑定授权码的书禁止删除
    if license::book_licensed(&state.db, &id).await? {
        return Err(anyhow::anyhow!("已绑定授权码的书籍不支持删除").into());
    }

    sqlx::query("DELETE FROM jobs WHERE chapter_id IN (SELECT id FROM chapters WHERE book_id = ?)")
        .bind(&id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM chapters WHERE book_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    // 清理文件（失败不阻塞）
    for ext in ["epub", "txt", "md", "pdf"] {
        let _ = std::fs::remove_file(state.data_dir.join("uploads").join(format!("{id}.{ext}")));
    }
    let _ = std::fs::remove_dir_all(state.data_dir.join("audio").join(&id));
    for ext in ["zip", "mp3", "m4b"] {
        let _ = std::fs::remove_file(state.data_dir.join("exports").join(format!("{id}.{ext}")));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn update_book(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBook>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(voice) = body.voice {
        sqlx::query("UPDATE books SET voice = ? WHERE id = ?")
            .bind(voice)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct UpdateChapter {
    title: String,
}

pub async fn update_chapter(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateChapter>,
) -> ApiResult<Json<serde_json::Value>> {
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(anyhow::anyhow!("标题不能为空").into());
    }
    sqlx::query("UPDATE chapters SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 合成 ----------

#[derive(Deserialize)]
pub struct SynthesizeRequest {
    chapter_ids: Option<Vec<i64>>,
}

#[derive(Serialize)]
pub struct SynthesizeResponse {
    queued: usize,
    locked: usize,
    skipped: usize,
}

pub async fn synthesize(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SynthesizeRequest>,
) -> ApiResult<Json<SynthesizeResponse>> {
    let book: Book = sqlx::query_as("SELECT * FROM books WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| anyhow::anyhow!("书籍不存在"))?;
    let licensed = license::book_licensed(&state.db, &id).await?;

    let mut chapters: Vec<Chapter> =
        sqlx::query_as("SELECT * FROM chapters WHERE book_id = ? ORDER BY idx")
            .bind(&id)
            .fetch_all(&state.db)
            .await?;
    if let Some(ids) = &body.chapter_ids {
        chapters.retain(|c| ids.contains(&c.id));
    }

    let (mut queued, mut locked, mut skipped) = (0, 0, 0);
    for ch in chapters {
        if ch.idx >= book.free_count && !licensed {
            locked += 1;
            continue;
        }
        if ["done", "synthesizing", "queued"].contains(&ch.status.as_str()) {
            skipped += 1;
            continue;
        }
        sqlx::query("UPDATE chapters SET status = 'queued', error = NULL WHERE id = ?")
            .bind(ch.id)
            .execute(&state.db)
            .await?;
        sqlx::query("INSERT INTO jobs (chapter_id) VALUES (?)")
            .bind(ch.id)
            .execute(&state.db)
            .await?;
        queued += 1;
    }
    Ok(Json(SynthesizeResponse { queued, locked, skipped }))
}

pub async fn book_jobs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let jobs: Vec<Job> = sqlx::query_as(
        "SELECT j.* FROM jobs j JOIN chapters c ON c.id = j.chapter_id \
         WHERE c.book_id = ? ORDER BY j.id DESC LIMIT 50",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        jobs.into_iter()
            .map(|j| {
                serde_json::json!({
                    "id": j.id,
                    "chapter_id": j.chapter_id,
                    "status": j.status,
                    "progress": j.progress,
                    "total": j.total,
                    "error": j.error,
                })
            })
            .collect(),
    ))
}

// ---------- 播放与导出 ----------

#[derive(Serialize)]
pub struct ChapterText {
    id: i64,
    book_id: String,
    idx: i64,
    title: String,
    text: String,
    char_count: i64,
    status: String,
}

pub async fn chapter_text(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ChapterText>> {
    let (book_id, idx, title, text, char_count, status): (String, i64, String, String, i64, String) =
        sqlx::query_as("SELECT book_id, idx, title, text, char_count, status FROM chapters WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| anyhow::anyhow!("章节不存在"))?;
    Ok(Json(ChapterText { id, book_id, idx, title, text, char_count, status }))
}

pub async fn chapter_audio(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let (path,): (Option<String>,) =
        sqlx::query_as("SELECT audio_path FROM chapters WHERE id = ? AND status = 'done'")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| anyhow::anyhow!("章节音频不存在"))?;
    let path = path.ok_or_else(|| anyhow::anyhow!("章节音频不存在"))?;
    let bytes = tokio::fs::read(&path).await?;
    Ok(([(header::CONTENT_TYPE, "audio/mpeg")], Body::from(bytes)).into_response())
}

async fn done_chapters(state: &AppState, book_id: &str) -> ApiResult<(Book, Vec<Chapter>)> {
    let book: Book = sqlx::query_as("SELECT * FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| anyhow::anyhow!("书籍不存在"))?;
    let chapters: Vec<Chapter> = sqlx::query_as(
        "SELECT * FROM chapters WHERE book_id = ? AND status = 'done' ORDER BY idx",
    )
    .bind(book_id)
    .fetch_all(&state.db)
    .await?;
    if chapters.is_empty() {
        return Err(anyhow::anyhow!("还没有已合成的章节").into());
    }
    Ok((book, chapters))
}

pub async fn export_zip(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (book, chapters) = done_chapters(&state, &id).await?;
    let out = state.data_dir.join("exports").join(format!("{id}.zip"));
    let titles = display_titles(&chapters);

    let chapters_clone = chapters.clone();
    let out_clone = out.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&out_clone)?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (ch, title) in chapters_clone.iter().zip(titles.iter()) {
            let src = ch.audio_path.clone().unwrap_or_default();
            let name = format!("{:03}-{}.mp3", ch.idx + 1, sanitize(title));
            zip.start_file(name, opts)?;
            zip.write_all(&std::fs::read(&src)?)?;
        }
        zip.finish()?;
        Ok(())
    })
    .await??;

    file_response(&out, "application/zip", &format!("{}.zip", sanitize(&book.title))).await
}

pub async fn export_mp3(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    export_single(&state, &id, false).await
}

pub async fn export_m4b(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    export_single(&state, &id, true).await
}

async fn export_single(state: &AppState, id: &str, m4b: bool) -> ApiResult<Response> {
    let (book, mut chapters) = done_chapters(state, id).await?;
    let ext = if m4b { "m4b" } else { "mp3" };
    let out = state.data_dir.join("exports").join(format!("{id}.{ext}"));

    // 补齐缺失的时长
    for ch in chapters.iter_mut() {
        if ch.duration_ms.is_none() {
            let p = ch.audio_path.clone().unwrap_or_default();
            ch.duration_ms = crate::audio::ffprobe_duration_ms(p.as_ref()).await.ok().map(|d| d as i64);
        }
    }

    let inputs: Vec<PathBuf> = chapters
        .iter()
        .map(|c| PathBuf::from(c.audio_path.clone().unwrap_or_default()))
        .collect();
    let metas: Vec<(String, u64)> = display_titles(&chapters)
        .into_iter()
        .zip(chapters.iter())
        .map(|(t, c)| (t, c.duration_ms.unwrap_or(0) as u64))
        .collect();

    crate::audio::export_with_chapters(&book.title, &inputs, &metas, &out, m4b).await?;
    let mime = if m4b { "audio/mp4" } else { "audio/mpeg" };
    file_response(&out, mime, &format!("{}.{ext}", sanitize(&book.title))).await
}

async fn file_response(path: &PathBuf, mime: &str, filename: &str) -> ApiResult<Response> {
    let bytes = tokio::fs::read(path).await?;
    let disposition = format!(
        "attachment; filename=\"book.{}\"; filename*=UTF-8''{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or("bin"),
        urlencode(filename)
    );
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(bytes),
    )
        .into_response())
}

// ---------- 授权码 ----------

#[derive(Deserialize)]
pub struct RedeemRequest {
    code: String,
    book_id: String,
}

pub async fn redeem_license(
    State(state): State<AppState>,
    Json(body): Json<RedeemRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    match license::redeem(&state.db, &body.code, &body.book_id).await? {
        RedeemOutcome::Activated => Ok(Json(serde_json::json!({ "ok": true, "message": "解锁成功，全部章节已开放" }))),
        RedeemOutcome::AlreadyBoundToThisBook => {
            Ok(Json(serde_json::json!({ "ok": true, "message": "该授权码已绑定本书" })))
        }
    }
}

pub async fn license_lookup(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    match license::book_for_code(&state.db, &code).await? {
        Some(book_id) => {
            let (title,): (String,) = sqlx::query_as("SELECT title FROM books WHERE id = ?")
                .bind(&book_id)
                .fetch_one(&state.db)
                .await
                .map_err(|_| anyhow::anyhow!("绑定的书籍不存在"))?;
            Ok(Json(serde_json::json!({ "book_id": book_id, "title": title })))
        }
        None => Ok(Json(serde_json::json!({ "book_id": null }))),
    }
}

pub async fn voice_list() -> Json<Vec<crate::voices::Voice>> {
    Json(crate::voices::voices().to_vec())
}

// ---------- 工具 ----------

/// 章节导出标题：level>0 时拼接最近的父级标题，如「第三部 · 第七章」
fn display_titles(chapters: &[Chapter]) -> Vec<String> {
    let mut out = Vec::with_capacity(chapters.len());
    for (i, c) in chapters.iter().enumerate() {
        let mut parts = vec![c.title.clone()];
        let mut lvl = c.level;
        let mut j = i;
        while lvl > 0 && j > 0 {
            j -= 1;
            let p = &chapters[j];
            if p.level < lvl {
                parts.push(p.title.clone());
                lvl = p.level;
            }
        }
        parts.reverse();
        out.push(parts.join(" · "));
    }
    out
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .chars()
        .take(60)
        .collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
