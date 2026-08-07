use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;

const COOKIE_NAME: &str = "b2t_session";

#[derive(Clone)]
pub struct SessionToken(pub String);

/// 匿名会话：无 cookie 则签发一个，书挂在 session 下
pub async fn session_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response<Body> {
    let existing = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|c| c.trim().split_once('='))
                .find(|(k, _)| *k == COOKIE_NAME)
                .map(|(_, v)| v.to_string())
        });

    let (token, is_new) = match existing {
        Some(t) if !t.is_empty() => (t, false),
        _ => (nanoid::nanoid!(24), true),
    };

    if is_new {
        let _ = sqlx::query("INSERT OR IGNORE INTO sessions (token) VALUES (?)")
            .bind(&token)
            .execute(&state.db)
            .await;
    }

    req.extensions_mut().insert(SessionToken(token.clone()));
    let mut resp = next.run(req).await;
    if is_new {
        resp.headers_mut().insert(
            header::SET_COOKIE,
            format!("{COOKIE_NAME}={token}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax")
                .parse()
                .unwrap(),
        );
    }
    resp
}
