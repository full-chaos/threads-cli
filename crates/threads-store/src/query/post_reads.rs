use rusqlite::{Connection, params};
use threads_core::model::{Post, PostId, UserId};

use crate::error::{Result, StoreError};

use super::row_conversion::load_post;

pub fn get_post(conn: &Connection, id: &PostId) -> Result<Option<Post>> {
    load_post(conn, id.as_str())
}

pub fn posts_by_author(conn: &Connection, author: &UserId) -> Result<Vec<PostId>> {
    let mut statement = conn
        .prepare("SELECT id FROM posts WHERE author_id = ?1")
        .map_err(StoreError::Sqlite)?;
    statement
        .query_map(params![author.as_str()], |row| {
            row.get::<_, String>(0).map(PostId::new)
        })
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)
}

pub fn list_posts(conn: &Connection, limit: usize) -> Result<Vec<Post>> {
    let mut statement = conn
        .prepare("SELECT id FROM posts ORDER BY fetched_at DESC LIMIT ?1")
        .map_err(StoreError::Sqlite)?;
    let ids: Vec<String> = statement
        .query_map(params![limit as i64], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?;
    let mut posts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(post) = load_post(conn, &id)? {
            posts.push(post);
        }
    }
    Ok(posts)
}
