use serde::Serialize;

#[derive(sqlx::FromRow, Serialize, Clone)]
pub struct Book {
    pub id: String,
    #[serde(skip_serializing)]
    pub session_token: String,
    pub title: String,
    pub source_ext: String,
    pub voice: String,
    pub free_count: i64,
    pub created_at: String,
}

#[derive(sqlx::FromRow, Serialize, Clone)]
pub struct Chapter {
    pub id: i64,
    pub book_id: String,
    pub idx: i64,
    pub level: i64,
    pub title: String,
    #[serde(skip_serializing)]
    pub text: String,
    pub char_count: i64,
    pub status: String,
    pub audio_path: Option<String>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

#[derive(sqlx::FromRow, Serialize)]
pub struct Job {
    pub id: i64,
    pub chapter_id: i64,
    pub status: String,
    pub progress: i64,
    pub total: i64,
    pub error: Option<String>,
    pub created_at: String,
}
