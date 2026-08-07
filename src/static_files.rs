use axum::body::Body;
use axum::http::Uri;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/frontend/dist/"]
struct Assets;

/// 托管 Astro 构建产物；含客户端层次路由映射
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let segs: Vec<&str> = path.split('/').collect();

    // /app/book/<id>、/app/book/<id>/chapter/<cid> 为客户端路由
    let mapped: String = match segs.as_slice() {
        ["app", "book", _id] => "app/book/index.html".into(),
        ["app", "book", _id, "chapter", _cid] => "app/chapter/index.html".into(),
        _ => {
            let candidates: Vec<String> = if path.is_empty() {
                vec!["index.html".into()]
            } else if path.ends_with('/') || !path.contains('.') {
                vec![
                    format!("{}/index.html", path.trim_end_matches('/')),
                    format!("{}.html", path.trim_end_matches('/')),
                ]
            } else {
                vec![path.to_string()]
            };
            candidates
                .into_iter()
                .find(|c| Assets::get(c).is_some())
                .unwrap_or_else(|| "index.html".into())
        }
    };
    serve(&mapped)
}

fn serve(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, Body::from("frontend not built")).into_response(),
    }
}
