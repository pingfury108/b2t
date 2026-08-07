use std::path::Path;

use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

pub async fn init(data_dir: &Path) -> Result<SqlitePool> {
    let db_path = data_dir.join("b2t.db");
    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await?;

    // 表结构变更：在 migrations/ 追加 NNNN_描述.sql，版本号递增，历史文件不改
    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}
