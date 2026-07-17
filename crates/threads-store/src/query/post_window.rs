use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use threads_core::model::{Post, UserId};

use crate::error::{Result, StoreError};

use super::row_conversion::load_posts;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostKind {
    Post,
    Reply,
}

pub fn posts_in_window(
    conn: &Connection,
    author: &UserId,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    kind: PostKind,
    limit: usize,
) -> Result<Vec<Post>> {
    let parent_filter = match kind {
        PostKind::Post => "parent_id IS NULL",
        PostKind::Reply => "parent_id IS NOT NULL",
    };
    let (after, before) = (
        after.map(|dt| dt.to_rfc3339()),
        before.map(|dt| dt.to_rfc3339()),
    );
    let ids: Vec<String> = match (after.as_deref(), before.as_deref()) {
        (Some(after), Some(before)) => query_window(
            conn,
            parent_filter,
            "AND created_at >= ?2 AND created_at < ?3",
            params![author.as_str(), after, before, limit as i64],
        )?,
        (Some(after), None) => query_window(
            conn,
            parent_filter,
            "AND created_at >= ?2",
            params![author.as_str(), after, limit as i64],
        )?,
        (None, Some(before)) => query_window(
            conn,
            parent_filter,
            "AND created_at < ?2",
            params![author.as_str(), before, limit as i64],
        )?,
        (None, None) => query_window(
            conn,
            parent_filter,
            "",
            params![author.as_str(), limit as i64],
        )?,
    };
    load_posts(conn, ids)
}

fn query_window(
    conn: &Connection,
    parent_filter: &str,
    date_filter: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<String>> {
    let sql = format!(
        "SELECT id FROM posts
         WHERE author_id = ?1
           AND created_at IS NOT NULL
           AND {parent_filter}
           {date_filter}
         ORDER BY created_at ASC
         LIMIT ?{}",
        if date_filter.is_empty() {
            2
        } else if date_filter.contains("?3") {
            4
        } else {
            3
        }
    );
    let mut statement = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
    statement
        .query_map(params, |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)
}
