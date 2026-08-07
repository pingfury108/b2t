mod admin;
mod audio;
mod db;
mod jobs;
mod license;
mod models;
mod parser;
mod routes;
mod session;
mod static_files;
mod tts;
mod upload;
mod voices;

use std::path::PathBuf;

use anyhow::Result;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post, put};
use axum::Router;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "b2t", version, about = "B2T · 把书变成有声书")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// 监听地址
    #[arg(long, env = "B2T_HOST", default_value = "0.0.0.0")]
    host: String,

    /// 监听端口
    #[arg(long, env = "PORT", default_value_t = 3000)]
    port: u16,

    /// 数据目录（SQLite、上传、音频产物）
    #[arg(long, env = "B2T_DATA", default_value = "data")]
    data_dir: PathBuf,

    /// 管理后台 key，不设置则后台关闭
    #[arg(long, env = "B2T_ADMIN_KEY")]
    admin_key: Option<String>,

    /// 站点外部访问地址（用于 sitemap/canonical），如 https://b2t.example.com
    #[arg(long, env = "B2T_BASE_URL")]
    base_url: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// 批量生成授权码并打印明文（库中只存哈希）
    GenCodes {
        /// 生成数量
        #[arg(default_value_t = 10)]
        count: usize,
    },
}

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub data_dir: PathBuf,
    pub admin_key: Option<String>,
    pub base_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 先加载 .env，让 RUST_LOG / B2T_* 可以被 .env 配置
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "b2t=info,tower_http=warn".into()))
        .init();

    let cli = Cli::parse();

    for sub in ["uploads", "audio", "exports", "tmp"] {
        std::fs::create_dir_all(cli.data_dir.join(sub))?;
    }

    let pool = db::init(&cli.data_dir).await?;

    if let Some(Commands::GenCodes { count }) = cli.command {
        let codes = license::create_codes(&pool, count).await?;
        for c in codes {
            println!("{c}");
        }
        return Ok(());
    }

    for bin in ["edge-tts", "ffmpeg", "ffprobe"] {
        if which(bin) {
            tracing::info!("found dependency: {bin}");
        } else {
            tracing::warn!("missing dependency: {bin} (TTS/导出功能将不可用)");
        }
    }

    if cli.admin_key.is_none() {
        tracing::warn!("管理后台未启用（通过 --admin-key 或 B2T_ADMIN_KEY 设置）");
    }

    let state = AppState {
        db: pool,
        data_dir: cli.data_dir,
        admin_key: cli.admin_key,
        base_url: cli.base_url.map(|u| u.trim_end_matches('/').to_string()),
    };

    jobs::spawn_worker(state.clone());

    // 后台路由：登录/登出公开，其余接口由 cookie 中间件保护
    let admin_public = Router::new()
        .route("/login", post(admin::login))
        .route("/logout", post(admin::logout));
    let admin_protected = Router::new()
        .route("/stats", get(admin::stats))
        .route("/codes", post(admin::create).get(admin::list))
        .route("/codes/{hash}", axum::routing::delete(admin::delete_code))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            admin::auth_middleware,
        ));

    let app = Router::new()
        .route("/api/books", post(routes::upload_book).get(routes::list_books))
        .route("/api/uploads/init", post(upload::init))
        .route("/api/uploads/{id}", get(upload::status))
        .route("/api/uploads/{id}/chunk", put(upload::chunk))
        .route("/api/uploads/{id}/complete", post(upload::complete))
        .route(
            "/api/books/{id}",
            get(routes::book_detail)
                .patch(routes::update_book)
                .delete(routes::delete_book),
        )
        .route("/api/books/{id}/synthesize", post(routes::synthesize))
        .route("/api/books/{id}/jobs", get(routes::book_jobs))
        .route("/api/books/{id}/export.zip", get(routes::export_zip))
        .route("/api/books/{id}/export.mp3", get(routes::export_mp3))
        .route("/api/books/{id}/export.m4b", get(routes::export_m4b))
        .route("/api/chapters/{id}", patch(routes::update_chapter))
        .route("/api/chapters/{id}/text", get(routes::chapter_text))
        .route("/api/chapters/{id}/audio", get(routes::chapter_audio))
        .route("/api/license/redeem", post(routes::redeem_license))
        .route("/api/license/{code}", get(routes::license_lookup))
        .route("/api/voices", get(routes::voice_list))
        .route("/sitemap.xml", get(static_files::sitemap))
        .nest("/api/admin", admin_public.merge(admin_protected))
        .fallback(get(static_files::static_handler))
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            session::session_middleware,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let listen = format!("{}:{}", cli.host, cli.port);
    tracing::info!("listening on http://{listen}");
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
        .unwrap_or(false)
}
