use rusqlite::{Connection, params};
use threads_core::model::Post;

use crate::error::{Result, StoreError};

use super::row_conversion::load_posts;

pub fn search_text(conn: &Connection, query: &str, limit: usize) -> Result<Vec<Post>> {
    let mut statement = conn
        .prepare(
            "SELECT p.id FROM posts p
             JOIN posts_fts f ON p.rowid = f.rowid
             WHERE posts_fts MATCH ?1
             ORDER BY bm25(posts_fts)
             LIMIT ?2",
        )
        .map_err(StoreError::Sqlite)?;
    let ids: Vec<String> = statement
        .query_map(params![query, limit as i64], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?;
    load_posts(conn, ids)
}
