use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use threads_core::model::FetchRun;

use crate::error::{Result, StoreError};

pub fn record_fetch_run_start(conn: &Connection, run: &FetchRun) -> Result<()> {
    conn.execute(
        "INSERT INTO fetch_runs (id, provider, started_at, finished_at, posts_fetched, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO NOTHING",
        params![
            run.id,
            run.provider,
            run.started_at.to_rfc3339(),
            run.finished_at.map(|datetime| datetime.to_rfc3339()),
            run.posts_fetched as i64,
            run.error,
        ],
    )
    .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn record_fetch_run_end(
    conn: &Connection,
    id: &str,
    finished_at: DateTime<Utc>,
    posts_fetched: u64,
    error: Option<&str>,
) -> Result<()> {
    let rows = conn
        .execute(
            "UPDATE fetch_runs SET finished_at = ?1, posts_fetched = ?2, error = ?3
             WHERE id = ?4",
            params![finished_at.to_rfc3339(), posts_fetched as i64, error, id],
        )
        .map_err(StoreError::Sqlite)?;
    if rows == 0 {
        return Err(StoreError::NotFound(format!("fetch_run {id}")));
    }
    Ok(())
}
