use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::AppState;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/frontend/dist/"]
struct Assets;

/// 托管 Astro 构建产物；书籍/章节客户端路由做内容注入（SEO）
pub async fn static_handler(State(state): State<AppState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let segs: Vec<&str> = path.split('/').collect();

    match segs.as_slice() {
        ["app", "book", id] => {
            if let Some(resp) = seo_book_page(&state, id).await {
                return resp;
            }
            serve("app/book/index.html")
        }
        ["app", "book", _id, "chapter", cid] => {
            if let Some(resp) = seo_chapter_page(&state, cid).await {
                return resp;
            }
            serve("app/chapter/index.html")
        }
        _ => serve(&map_path(path)),
    }
}

fn map_path(path: &str) -> String {
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

fn serve_html(html: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn inject_head(html: &mut String, description: &str, extra: &str) {
    let inject = format!(
        "<meta name=\"description\" content=\"{}\" />{}",
        esc(description),
        extra
    );
    *html = html.replacen("</head>", &format!("{inject}</head>"), 1);
}

/// 工作台：注入书名 + 章节链接列表，供爬虫发现章节页
async fn seo_book_page(state: &AppState, book_id: &str) -> Option<Response> {
    let (title,): (String,) = sqlx::query_as("SELECT title FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&state.db)
        .await
        .ok()?;
    let chapters: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, idx, title FROM chapters WHERE book_id = ? ORDER BY idx")
            .bind(book_id)
            .fetch_all(&state.db)
            .await
            .ok()?;

    let base = Assets::get("app/book/index.html")?;
    let mut html = String::from_utf8_lossy(&base.data).into_owned();

    html = html.replace(
        "<title>制作台 · B2T</title>",
        &format!("<title>{} · 有声书 | B2T</title>", esc(&title)),
    );
    let json_ld = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "Book",
        "name": title,
        "numberOfPages": chapters.len(),
    });
    inject_head(
        &mut html,
        &format!("《{title}》有声书，共 {} 章，在线收听与下载", chapters.len()),
        &format!("<script type=\"application/ld+json\">{json_ld}</script>"),
    );

    html = html.replace(
        r#"id="crumb-book"></span>"#,
        &format!(r#"id="crumb-book">{}</span>"#, esc(&title)),
    );
    html = html.replace(
        r#"id="book-title"></h1>"#,
        &format!(r#"id="book-title">{}</h1>"#, esc(&title)),
    );
    html = html.replace(r#"id="workbench" hidden>"#, r#"id="workbench">"#);

    let rows: String = chapters
        .iter()
        .map(|(cid, idx, t)| {
            format!(
                "<tr><td>{}</td><td><a href=\"/app/book/{}/chapter/{}\">{}</a></td><td></td><td></td><td></td></tr>",
                idx + 1,
                book_id,
                cid,
                esc(t)
            )
        })
        .collect();
    html = html.replace(
        r#"id="chapter-rows"></tbody>"#,
        &format!(r#"id="chapter-rows">{rows}</tbody>"#),
    );

    Some(serve_html(html))
}

/// 阅读页：注入标题、正文、面包屑、上下章链接
async fn seo_chapter_page(state: &AppState, chapter_id: &str) -> Option<Response> {
    let cid: i64 = chapter_id.parse().ok()?;
    let (book_id, idx, title, text): (String, i64, String, String) = sqlx::query_as(
        "SELECT book_id, idx, title, text FROM chapters WHERE id = ?",
    )
    .bind(cid)
    .fetch_one(&state.db)
    .await
    .ok()?;
    let (book_title,): (String,) = sqlx::query_as("SELECT title FROM books WHERE id = ?")
        .bind(&book_id)
        .fetch_one(&state.db)
        .await
        .ok()?;
    let prev: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM chapters WHERE book_id = ? AND idx = ?",
    )
    .bind(&book_id)
    .bind(idx - 1)
    .fetch_optional(&state.db)
    .await
    .ok()?;
    let next: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM chapters WHERE book_id = ? AND idx = ?",
    )
    .bind(&book_id)
    .bind(idx + 1)
    .fetch_optional(&state.db)
    .await
    .ok()?;

    let base = Assets::get("app/chapter/index.html")?;
    let mut html = String::from_utf8_lossy(&base.data).into_owned();

    html = html.replace(
        "<title>章节阅读 · B2T</title>",
        &format!("<title>{} · {} | B2T</title>", esc(&title), esc(&book_title)),
    );
    let description: String = text.chars().take(150).collect();
    let json_ld = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "Chapter",
        "name": title,
        "position": idx + 1,
        "isPartOf": { "@type": "Book", "name": book_title },
    });
    inject_head(
        &mut html,
        &description,
        &format!("<script type=\"application/ld+json\">{json_ld}</script>"),
    );

    html = html.replace(r#"id="reader" hidden>"#, r#"id="reader">"#);
    html = html.replace(
        r##"id="crumb-book" href="#"></a>"##,
        &format!(
            r#"id="crumb-book" href="/app/book/{}">{}</a>"#,
            book_id,
            esc(&book_title)
        ),
    );
    html = html.replace(
        r#"id="crumb-chapter"></span>"#,
        &format!(r#"id="crumb-chapter">{}</span>"#, esc(&title)),
    );
    html = html.replace(
        r#"id="chapter-title"></h1>"#,
        &format!(r#"id="chapter-title">{}</h1>"#, esc(&title)),
    );
    html = html.replace(
        r#"id="chapter-meta"></p>"#,
        &format!(r#"id="chapter-meta">第 {} 章 · {}</p>"#, idx + 1, esc(&book_title)),
    );
    html = html.replace(
        r#"id="chapter-text"></article>"#,
        &format!(r#"id="chapter-text">{}</article>"#, esc(&text)),
    );

    if let Some((pid, ptitle)) = prev {
        html = html.replace(
            r#"id="prev-chapter" class="pager-link" hidden>"#,
            &format!(
                r#"id="prev-chapter" class="pager-link" href="/app/book/{}/chapter/{}">"#,
                book_id, pid
            ),
        );
        html = html.replace(
            r#"id="prev-title"></b>"#,
            &format!(r#"id="prev-title">{}</b>"#, esc(&ptitle)),
        );
    }
    if let Some((nid, ntitle)) = next {
        html = html.replace(
            r#"id="next-chapter" class="pager-link next" hidden>"#,
            &format!(
                r#"id="next-chapter" class="pager-link next" href="/app/book/{}/chapter/{}">"#,
                book_id, nid
            ),
        );
        html = html.replace(
            r#"id="next-title"></b>"#,
            &format!(r#"id="next-title">{}</b>"#, esc(&ntitle)),
        );
    }

    Some(serve_html(html))
}

/// 动态 sitemap（需 --base-url）
pub async fn sitemap(State(state): State<AppState>) -> Response {
    let Some(base) = &state.base_url else {
        return (StatusCode::NOT_FOUND, "base url not configured").into_response();
    };
    let books: Vec<(String,)> = sqlx::query_as("SELECT id FROM books ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let chapters: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, book_id FROM chapters ORDER BY book_id, idx")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for p in ["/", "/help"] {
        xml.push_str(&format!("  <url><loc>{base}{p}</loc></url>\n"));
    }
    for (id,) in &books {
        xml.push_str(&format!("  <url><loc>{base}/app/book/{id}</loc></url>\n"));
    }
    for (cid, bid) in &chapters {
        xml.push_str(&format!(
            "  <url><loc>{base}/app/book/{bid}/chapter/{cid}</loc></url>\n"
        ));
    }
    xml.push_str("</urlset>");
    (
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}
