use rusqlite::Connection;

use crate::error::{Result, StoreError};

/// Each migration is identified by a monotonically increasing version integer.
/// `apply` is called once per migration; it must be idempotent (using
/// `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, etc.).
struct Migration {
    version: i64,
    apply: fn(&Connection) -> Result<()>,
}

/// All versioned migrations in ascending order.
fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            apply: migration_v1,
        },
        Migration {
            version: 2,
            apply: migration_v2_fts,
        },
    ]
}

/// V1: core tables — users, posts, edges, media, urls, mentions, fetch_runs, raw_payloads.
fn migration_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT    NOT NULL
        );

        CREATE TABLE IF NOT EXISTS users (
            id                  TEXT PRIMARY KEY,
            username            TEXT,
            name                TEXT,
            biography           TEXT,
            profile_picture_url TEXT,
            updated_at          TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS posts (
            id           TEXT PRIMARY KEY,
            author_id    TEXT    NOT NULL,
            text         TEXT,
            created_at   TEXT,
            parent_id    TEXT,
            root_id      TEXT,
            permalink    TEXT,
            is_quote_post INTEGER NOT NULL DEFAULT 0,
            fetched_at   TEXT    NOT NULL,
            FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS posts_author_idx  ON posts(author_id);
        CREATE INDEX IF NOT EXISTS posts_parent_idx  ON posts(parent_id);
        CREATE INDEX IF NOT EXISTS posts_root_idx    ON posts(root_id);
        CREATE INDEX IF NOT EXISTS posts_created_idx ON posts(created_at);

        CREATE TABLE IF NOT EXISTS edges (
            from_id TEXT NOT NULL,
            to_id   TEXT NOT NULL,
            kind    TEXT NOT NULL CHECK (kind IN ('reply','root','mention','quote')),
            PRIMARY KEY (from_id, to_id, kind)
        );

        CREATE TABLE IF NOT EXISTS media (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id       TEXT NOT NULL,
            kind          TEXT NOT NULL,
            url           TEXT,
            thumbnail_url TEXT,
            FOREIGN KEY (post_id) REFERENCES posts(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS media_post_idx ON media(post_id);

        CREATE TABLE IF NOT EXISTS urls (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id      TEXT NOT NULL,
            url          TEXT NOT NULL,
            display_text TEXT,
            FOREIGN KEY (post_id) REFERENCES posts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS mentions (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id  TEXT NOT NULL,
            username TEXT NOT NULL,
            user_id  TEXT,
            FOREIGN KEY (post_id) REFERENCES posts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS fetch_runs (
            id            TEXT PRIMARY KEY,
            provider      TEXT    NOT NULL,
            started_at    TEXT    NOT NULL,
            finished_at   TEXT,
            posts_fetched INTEGER NOT NULL DEFAULT 0,
            error         TEXT
        );

        CREATE TABLE IF NOT EXISTS raw_payloads (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id      TEXT NOT NULL,
            provider     TEXT NOT NULL,
            fetch_run_id TEXT,
            payload      TEXT NOT NULL,
            fetched_at   TEXT NOT NULL,
            FOREIGN KEY (post_id)      REFERENCES posts(id)       ON DELETE CASCADE,
            FOREIGN KEY (fetch_run_id) REFERENCES fetch_runs(id)
        );
        CREATE INDEX IF NOT EXISTS raw_payloads_post_idx ON raw_payloads(post_id);
        ",
    )
    .map_err(StoreError::Sqlite)
}

/// V2: FTS5 virtual table mirroring posts.text, plus INSERT/UPDATE/DELETE triggers.
fn migration_v2_fts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS posts_fts
            USING fts5(text, content='posts', content_rowid='rowid');

        -- Keep FTS in sync with posts table.
        CREATE TRIGGER IF NOT EXISTS posts_ai AFTER INSERT ON posts BEGIN
            INSERT INTO posts_fts(rowid, text) VALUES (new.rowid, new.text);
        END;

        CREATE TRIGGER IF NOT EXISTS posts_ad AFTER DELETE ON posts BEGIN
            INSERT INTO posts_fts(posts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
        END;

        CREATE TRIGGER IF NOT EXISTS posts_au AFTER UPDATE ON posts BEGIN
            INSERT INTO posts_fts(posts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
            INSERT INTO posts_fts(rowid, text) VALUES (new.rowid, new.text);
        END;
        ",
    )
    .map_err(StoreError::Sqlite)
}

/// Apply all pending migrations to `conn`.  Called by [`Store::open`] on
/// every connection open; safe to call multiple times (idempotent).
pub fn run_migrations(conn: &Connection) -> Result<()> {
    // Ensure the tracking table exists before we query it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(StoreError::Sqlite)?;

    let applied: std::collections::HashSet<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations")
            .map_err(StoreError::Sqlite)?;
        stmt.query_map([], |row| row.get(0))
            .map_err(StoreError::Sqlite)?
            .filter_map(|r| r.ok())
            .collect()
    };

    for mig in migrations() {
        if applied.contains(&mig.version) {
            continue;
        }
        (mig.apply)(conn).map_err(|e| {
            StoreError::Migration(format!("migration v{} failed: {e}", mig.version))
        })?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![mig.version, now],
        )
        .map_err(StoreError::Sqlite)?;
    }

    Ok(())
}
