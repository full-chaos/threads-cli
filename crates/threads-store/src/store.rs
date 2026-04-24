use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use threads_core::model::{FetchRun, Post, PostId, User};

use crate::error::{Result, StoreError};
use crate::migrations::run_migrations;
use crate::query;

/// Thread-safe SQLite store.  The connection is wrapped in a `Mutex` so that
/// `Store` can be `Send + Sync` while rusqlite's `Connection` is `!Send`.
pub struct Store {
    conn: Mutex<Connection>,
}

// Safety: `Mutex<Connection>` provides the needed mutual exclusion; we never
// hand out references to `Connection` across threads.
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Store {
    fn configure(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(StoreError::Sqlite)?;
        run_migrations(conn)?;
        Ok(())
    }

    /// Open (or create) a store at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(StoreError::Sqlite)?;
        Self::configure(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory store (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        Self::configure(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ------------------------------------------------------------------ //
    //  Query wrappers (delegate to query module)                          //
    // ------------------------------------------------------------------ //

    pub fn upsert_user(&self, user: &User) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        query::upsert_user(&conn, user)
    }

    pub fn upsert_post(&self, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        query::upsert_post(&mut conn, post, fetch_run_id)
    }

    pub fn upsert_posts(&self, posts: &[Post], fetch_run_id: Option<&str>) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        query::upsert_posts(&mut conn, posts, fetch_run_id)
    }

    pub fn get_post(&self, id: &PostId) -> Result<Option<Post>> {
        let conn = self.conn.lock().unwrap();
        query::get_post(&conn, id)
    }

    pub fn search_text(&self, query_str: &str, limit: usize) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::search_text(&conn, query_str, limit)
    }

    pub fn thread_rooted_at(&self, root_id: &PostId) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::thread_rooted_at(&conn, root_id)
    }

    pub fn record_fetch_run_start(&self, run: &FetchRun) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        query::record_fetch_run_start(&conn, run)
    }

    pub fn record_fetch_run_end(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        posts_fetched: u64,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        query::record_fetch_run_end(&conn, id, finished_at, posts_fetched, error)
    }
}
