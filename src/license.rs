use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use anyhow::{bail, Result};

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const GROUP: usize = 4;
const GROUPS: usize = 3;

/// XXXX-XXXX-XXXX
pub fn generate_code() -> String {
    let mut rng = rand::rng();
    (0..GROUPS)
        .map(|_| {
            (0..GROUP)
                .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

pub fn hash_code(code: &str) -> String {
    let normalized: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 批量生成授权码并入库（存哈希），返回明文码列表
pub async fn create_codes(db: &SqlitePool, n: usize) -> Result<Vec<String>> {
    let mut codes = Vec::with_capacity(n);
    for _ in 0..n {
        let code = generate_code();
        let hash = hash_code(&code);
        sqlx::query("INSERT OR IGNORE INTO license_codes (code_hash) VALUES (?)")
            .bind(&hash)
            .execute(db)
            .await?;
        codes.push(code);
    }
    Ok(codes)
}

pub enum RedeemOutcome {
    Activated,
    AlreadyBoundToThisBook,
}

/// 核验并绑定授权码。一码一书，原子更新防并发复用
pub async fn redeem(db: &SqlitePool, code: &str, book_id: &str) -> Result<RedeemOutcome> {
    let hash = hash_code(code);
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, book_id FROM license_codes WHERE code_hash = ?")
            .bind(&hash)
            .fetch_optional(db)
            .await?;

    let Some((status, bound_book)) = row else {
        bail!("授权码无效");
    };

    if status == "used" {
        if bound_book.as_deref() == Some(book_id) {
            return Ok(RedeemOutcome::AlreadyBoundToThisBook);
        }
        bail!("授权码已被使用");
    }
    if status != "unused" {
        bail!("授权码不可用");
    }

    let res = sqlx::query(
        "UPDATE license_codes SET status = 'used', book_id = ?, used_at = datetime('now') \
         WHERE code_hash = ? AND status = 'unused'",
    )
    .bind(book_id)
    .bind(&hash)
    .execute(db)
    .await?;

    if res.rows_affected() == 0 {
        bail!("授权码核验失败，请重试");
    }
    Ok(RedeemOutcome::Activated)
}

pub async fn book_licensed(db: &SqlitePool, book_id: &str) -> Result<bool> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM license_codes WHERE book_id = ? AND status = 'used'",
    )
    .bind(book_id)
    .fetch_one(db)
    .await?;
    Ok(count.0 > 0)
}

/// 通过授权码找回书籍
pub async fn book_for_code(db: &SqlitePool, code: &str) -> Result<Option<String>> {
    let hash = hash_code(code);
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT book_id FROM license_codes WHERE code_hash = ? AND status = 'used'")
            .bind(&hash)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|r| r.0))
}

// ---------- 后台管理 ----------

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct CodeRow {
    pub code_hash: String,
    pub status: String,
    pub book_id: Option<String>,
    pub book_title: Option<String>,
    pub created_at: String,
    pub used_at: Option<String>,
}

pub struct CodeStats {
    pub unused: i64,
    pub used: i64,
}

pub async fn code_stats(db: &SqlitePool) -> Result<CodeStats> {
    let (unused,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM license_codes WHERE status = 'unused'")
            .fetch_one(db)
            .await?;
    let (used,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM license_codes WHERE status = 'used'")
            .fetch_one(db)
            .await?;
    Ok(CodeStats { unused, used })
}

pub async fn list_codes(db: &SqlitePool, status: Option<&str>) -> Result<Vec<CodeRow>> {
    const SQL: &str = "SELECT c.code_hash, c.status, c.book_id, b.title AS book_title, \
         c.created_at, c.used_at \
         FROM license_codes c LEFT JOIN books b ON b.id = c.book_id";
    let rows = match status {
        Some(s) if !s.is_empty() => {
            sqlx::query_as(&format!("{SQL} WHERE c.status = ? ORDER BY c.created_at DESC LIMIT 1000"))
                .bind(s)
                .fetch_all(db)
                .await?
        }
        _ => sqlx::query_as(&format!("{SQL} ORDER BY c.created_at DESC LIMIT 1000"))
            .fetch_all(db)
            .await?,
    };
    Ok(rows)
}

/// 删除授权码；已绑定书籍的码拒绝删除
pub async fn delete_code(db: &SqlitePool, code_hash: &str) -> Result<()> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT status FROM license_codes WHERE code_hash = ?")
            .bind(code_hash)
            .fetch_optional(db)
            .await?;
    match row {
        None => bail!("授权码不存在"),
        Some((status,)) if status != "unused" => bail!("已绑定书籍的授权码不支持删除"),
        _ => {}
    }
    sqlx::query("DELETE FROM license_codes WHERE code_hash = ?")
        .bind(code_hash)
        .execute(db)
        .await?;
    Ok(())
}
