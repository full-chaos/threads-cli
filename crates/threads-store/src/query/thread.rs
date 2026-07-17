use rusqlite::{Connection, params};
use threads_core::model::{Post, PostId};

use crate::error::{Result, StoreError};

use super::row_conversion::load_posts;

pub fn thread_rooted_at(conn: &Connection, root_id: &PostId) -> Result<Vec<Post>> {
    let mut statement = conn
        .prepare(
            "WITH RECURSIVE thread(id, depth) AS (
                 SELECT ?1, 0
                 UNION ALL
                 SELECT e.from_id, t.depth + 1
                 FROM edges e
                 JOIN thread t ON e.to_id = t.id AND e.kind = 'reply'
             )
             SELECT DISTINCT p.id FROM posts p
             JOIN thread t ON p.id = t.id
             ORDER BY t.depth, p.created_at",
        )
        .map_err(StoreError::Sqlite)?;
    let ids: Vec<String> = statement
        .query_map(params![root_id.as_str()], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?;
    load_posts(conn, ids)
}
