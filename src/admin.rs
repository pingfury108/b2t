use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::license;
use crate::routes::ApiResult;
use crate::AppState;

const COOKIE_NAME: &str = "b2t_admin";

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn expected_token(key: &str) -> String {
    sha256_hex(&format!("b2t-admin:{key}"))
}

fn cookie_token(req: &Request) -> Option<String> {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE_NAME)
        .map(|(_, v)| v.to_string())
}

/// 保护 /api/admin/*：未配置 key 时整组 404，登录校验失败 401
pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(key) = &state.admin_key else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if cookie_token(&req).as_deref() != Some(expected_token(key).as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "未登录" })),
        )
            .into_response();
    }
    next.run(req).await
}

#[derive(Deserialize)]
pub struct LoginRequest {
    key: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(key) = &state.admin_key else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if sha256_hex(&body.key) != sha256_hex(key) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "key 不正确" })),
        )
            .into_response();
    }
    let cookie = format!(
        "{COOKIE_NAME}={}; Path=/; Max-Age=43200; HttpOnly; SameSite=Lax",
        expected_token(key)
    );
    ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response()
}

pub async fn logout() -> Response {
    (
        [(header::SET_COOKIE, format!("{COOKIE_NAME}=; Path=/; Max-Age=0"))],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let s = license::code_stats(&state.db).await?;
    Ok(Json(json!({
        "unused": s.unused,
        "used": s.used,
        "total": s.unused + s.used,
    })))
}

#[derive(Deserialize)]
pub struct CreateCodesRequest {
    count: usize,
}

/// 生成码：明文只在本次响应返回，库中只存哈希
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateCodesRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let count = body.count.clamp(1, 500);
    let codes = license::create_codes(&state.db, count).await?;
    Ok(Json(json!({ "codes": codes })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<license::CodeRow>>> {
    Ok(Json(
        license::list_codes(&state.db, q.status.as_deref()).await?,
    ))
}

/// 已绑定书籍（used）的码不支持删除
pub async fn delete_code(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    license::delete_code(&state.db, &hash).await?;
    Ok(Json(json!({ "ok": true })))
}
