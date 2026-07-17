use chrono::Utc;
use rusqlite::{Connection, Transaction, params};
use threads_core::model::{User, UserId};

use crate::error::{Result, StoreError};

pub fn upsert_user(conn: &Connection, user: &User) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
             username            = excluded.username,
             name                = excluded.name,
             biography           = excluded.biography,
             profile_picture_url = excluded.profile_picture_url,
             updated_at          = excluded.updated_at",
        params![
            user.id.as_str(),
            user.username,
            user.name,
            user.biography,
            user.profile_picture_url,
            now,
        ],
    )
    .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn resolve_author(conn: &mut Connection, username: &str, real_id: &UserId) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    reconcile_author_tx(&tx, username, real_id, &now)?;
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

pub(super) fn reconcile_author_tx(
    tx: &Transaction,
    username: &str,
    id: &UserId,
    now: &str,
) -> Result<()> {
    let placeholder = format!("@{username}");
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, ?3)
         ON CONFLICT(id) DO UPDATE SET
             username = excluded.username,
             updated_at = excluded.updated_at",
        params![id.as_str(), username, now],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute(
        "UPDATE posts SET author_id = ?1 WHERE author_id = ?2",
        params![id.as_str(), &placeholder],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute("DELETE FROM users WHERE id = ?1", params![&placeholder])
        .map_err(StoreError::Sqlite)?;
    Ok(())
}
