use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use threads_core::model::{
    AudienceSnapshot, EngagedAccount, EngagementSort, FetchRun, Post, PostId, User, UserId,
};
#[cfg(unix)]
use tracing::warn;

use crate::error::{Result, StoreError};
use crate::migrations::run_migrations;
use crate::query;

/// Thread-safe SQLite store.  The connection is wrapped in a `Mutex` so that
/// `Store` can be `Send + Sync` while rusqlite's `Connection` is `!Send`.
pub struct Store {
    conn: Mutex<Connection>,
    database_path: Option<PathBuf>,
}

// Safety: `Mutex<Connection>` provides the needed mutual exclusion; we never
// hand out references to `Connection` across threads.
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Store {
    fn prepare_parent(path: &Path) -> Result<()> {
        let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(());
        };
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        Self::set_private_mode(parent);
        Ok(())
    }

    #[cfg(unix)]
    fn set_private_mode(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
            warn!(path = %path.display(), %error, "could not secure store directory");
        }
    }

    #[cfg(unix)]
    fn secure_database_files(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let mut shm = path.as_os_str().to_os_string();
        shm.push("-shm");
        let paths = [path.to_path_buf(), wal.into(), shm.into()];
        for file in paths {
            if !file.exists() {
                continue;
            }
            if let Err(error) =
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
            {
                warn!(path = %file.display(), %error, "could not secure store database file");
            }
        }
    }
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
        let path = path.as_ref();
        Self::prepare_parent(path)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(StoreError::Sqlite)?;
        Self::configure(&conn)?;
        #[cfg(unix)]
        Self::secure_database_files(path);
        Ok(Self {
            conn: Mutex::new(conn),
            database_path: Some(path.to_path_buf()),
        })
    }

    /// Open an in-memory store (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        Self::configure(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            database_path: None,
        })
    }

    /// Test-only locked-connection access so the sibling `tests` module and
    /// test-only probes in `query` can issue raw SQL. Not exported outside
    /// `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn raw_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
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

    pub fn upsert_audience_snapshot(
        &self,
        snapshot: &AudienceSnapshot,
        fetch_run_id: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        query::upsert_audience_snapshot(&mut conn, snapshot, fetch_run_id)?;
        #[cfg(unix)]
        if let Some(path) = &self.database_path {
            Self::secure_database_files(path);
        }
        Ok(())
    }

    pub fn audience_history(
        &self,
        account_id: &UserId,
        limit: usize,
    ) -> Result<Vec<AudienceSnapshot>> {
        let conn = self.conn.lock().unwrap();
        query::audience_history(&conn, account_id, limit)
    }

    pub fn count_audience_snapshots_before(
        &self,
        account_id: &UserId,
        cutoff: DateTime<Utc>,
    ) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        query::count_audience_snapshots_before(&conn, account_id, cutoff)
    }

    pub fn delete_audience_snapshots_before(
        &self,
        account_id: &UserId,
        cutoff: DateTime<Utc>,
    ) -> Result<u64> {
        let mut conn = self.conn.lock().unwrap();
        query::delete_audience_snapshots_before(&mut conn, account_id, cutoff)
    }

    pub fn get_post(&self, id: &PostId) -> Result<Option<Post>> {
        let conn = self.conn.lock().unwrap();
        query::get_post(&conn, id)
    }

    pub fn posts_by_author(&self, author: &threads_core::UserId) -> Result<Vec<PostId>> {
        let conn = self.conn.lock().unwrap();
        query::posts_by_author(&conn, author)
    }

    pub fn rank_engaged_accounts(
        &self,
        account_id: &UserId,
        limit: usize,
        sort: EngagementSort,
    ) -> Result<Vec<EngagedAccount>> {
        let conn = self.conn.lock().unwrap();
        query::rank_engaged_accounts(&conn, account_id, limit, sort)
    }

    pub fn search_text(&self, query_str: &str, limit: usize) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::search_text(&conn, query_str, limit)
    }

    pub fn list_posts(&self, limit: usize) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::list_posts(&conn, limit)
    }

    pub fn posts_in_window(
        &self,
        author: &UserId,
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
        kind: query::PostKind,
        limit: usize,
    ) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::posts_in_window(&conn, author, after, before, kind, limit)
    }

    pub fn delete_post(&self, id: &PostId) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        query::delete_post(&mut conn, id)
    }

    pub fn record_deletion(
        &self,
        id: &PostId,
        kind: query::PostKind,
        success: bool,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        query::record_deletion(&conn, id, kind, success, error)
    }

    pub fn deletions_in_last_24h(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        query::deletions_in_last_24h(&conn)
    }

    pub fn oldest_deletion_in_last_24h(&self) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        query::oldest_deletion_in_last_24h(&conn)
    }

    pub fn thread_rooted_at(&self, root_id: &PostId) -> Result<Vec<Post>> {
        let conn = self.conn.lock().unwrap();
        query::thread_rooted_at(&conn, root_id)
    }

    /// Resolve all posts stored under `'@' || username` to `real_id`, upsert the
    /// real user, and delete the placeholder user row.
    pub fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        query::resolve_author(&mut conn, username, real_id)
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
