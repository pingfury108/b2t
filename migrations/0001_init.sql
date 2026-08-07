CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS books (
    id            TEXT PRIMARY KEY,
    session_token TEXT NOT NULL,
    title         TEXT NOT NULL,
    source_ext    TEXT NOT NULL,
    voice         TEXT NOT NULL DEFAULT 'zh-CN-XiaoxiaoNeural',
    free_count    INTEGER NOT NULL DEFAULT 3,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS chapters (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    book_id     TEXT NOT NULL REFERENCES books(id),
    idx         INTEGER NOT NULL,
    title       TEXT NOT NULL,
    text        TEXT NOT NULL,
    char_count  INTEGER NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    audio_path  TEXT,
    duration_ms INTEGER,
    error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_chapters_book ON chapters(book_id, idx);

CREATE TABLE IF NOT EXISTS jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    chapter_id  INTEGER NOT NULL REFERENCES chapters(id),
    status      TEXT NOT NULL DEFAULT 'pending',
    progress    INTEGER NOT NULL DEFAULT 0,
    total       INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);

CREATE TABLE IF NOT EXISTS license_codes (
    code_hash  TEXT PRIMARY KEY,
    status     TEXT NOT NULL DEFAULT 'unused',
    book_id    TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    used_at    TEXT
);
