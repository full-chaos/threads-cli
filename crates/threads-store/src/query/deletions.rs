use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use threads_core::model::PostId;
use tracing::warn;

use crate::error::{Result, StoreError};

use super::post_window::PostKind;

pub fn delete_post(conn: &mut Connection, id: &PostId) -> Result<bool> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    tx.execute(
        "DELETE FROM edges WHERE from_id = ?1 OR to_id = ?1",
        params![id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;
    let deleted = tx
        .execute("DELETE FROM posts WHERE id = ?1", params![id.as_str()])
        .map_err(StoreError::Sqlite)?;
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(deleted > 0)
}

pub fn record_deletion(
    conn: &Connection,
    id: &PostId,
    kind: PostKind,
    success: bool,
    error: Option<&str>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let kind = match kind {
        PostKind::Post => "post",
        PostKind::Reply => "reply",
    };
    if let Err(error) = conn.execute(
        "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.as_str(), kind, now, success as i32, error],
    ) {
        warn!(post_id = id.as_str(), error = %error, "failed to record deletion audit row");
    }
    Ok(())
}

pub fn deletions_in_last_24h(conn: &Connection) -> Result<u64> {
    let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deletions WHERE deleted_at >= ?1 AND success = 1",
            params![cutoff],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    Ok(count as u64)
}

pub fn oldest_deletion_in_last_24h(conn: &Connection) -> Result<Option<DateTime<Utc>>> {
    let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let row: Option<String> = conn
        .query_row(
            "SELECT MIN(deleted_at) FROM deletions
             WHERE deleted_at >= ?1 AND success = 1",
            params![cutoff],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)?
        .flatten();
    Ok(row.and_then(|timestamp| {
        DateTime::parse_from_rfc3339(&timestamp)
            .ok()
            .map(|datetime| datetime.with_timezone(&Utc))
    }))
}
