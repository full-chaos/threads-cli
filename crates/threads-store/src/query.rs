use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use threads_core::model::{
    EdgeKind, FetchRun, Media, MediaKind, Mention, Post, PostId, UrlEntity, User, UserId,
};

use crate::error::{Result, StoreError};

// ------------------------------------------------------------------ //
//  Helpers                                                            //
// ------------------------------------------------------------------ //

#[allow(dead_code)]
fn edge_kind_str(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::Reply => "reply",
        EdgeKind::Root => "root",
        EdgeKind::Mention => "mention",
        EdgeKind::Quote => "quote",
    }
}

fn media_kind_str(k: &MediaKind) -> &'static str {
    match k {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Carousel => "carousel",
        MediaKind::Audio => "audio",
        MediaKind::Unknown => "unknown",
    }
}

fn media_kind_from_str(s: &str) -> MediaKind {
    match s {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        "carousel" => MediaKind::Carousel,
        "audio" => MediaKind::Audio,
        _ => MediaKind::Unknown,
    }
}

// ------------------------------------------------------------------ //
//  Users                                                              //
// ------------------------------------------------------------------ //

/// Upsert a single user (INSERT OR REPLACE).
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

// ------------------------------------------------------------------ //
//  Posts                                                              //
// ------------------------------------------------------------------ //

/// Upsert a post inside an already-open transaction.  Also upserts its
/// author, media, urls, mentions, edges, and optionally raw payload.
fn upsert_post_tx(tx: &Transaction, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
    let now = Utc::now().to_rfc3339();

    // Ensure the author stub exists so the FK is satisfied.
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, NULL, NULL, NULL, NULL, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![post.author.as_str(), &now],
    )
    .map_err(StoreError::Sqlite)?;

    // Upsert post row.
    let created_at_str = post.created_at.map(|dt| dt.to_rfc3339());
    tx.execute(
        "INSERT INTO posts (id, author_id, text, created_at, parent_id, root_id,
                            permalink, is_quote_post, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             author_id     = excluded.author_id,
             text          = excluded.text,
             created_at    = excluded.created_at,
             parent_id     = excluded.parent_id,
             root_id       = excluded.root_id,
             permalink     = excluded.permalink,
             is_quote_post = excluded.is_quote_post,
             fetched_at    = excluded.fetched_at",
        params![
            post.id.as_str(),
            post.author.as_str(),
            post.text,
            created_at_str,
            post.parent_id.as_ref().map(|p| p.as_str()),
            post.root_id.as_ref().map(|r| r.as_str()),
            post.permalink,
            post.is_quote_post as i32,
            &now,
        ],
    )
    .map_err(StoreError::Sqlite)?;

    // Delete old child rows before re-inserting (simpler than diffing).
    tx.execute("DELETE FROM media WHERE post_id = ?1", params![post.id.as_str()])
        .map_err(StoreError::Sqlite)?;
    tx.execute("DELETE FROM urls WHERE post_id = ?1", params![post.id.as_str()])
        .map_err(StoreError::Sqlite)?;
    tx.execute("DELETE FROM mentions WHERE post_id = ?1", params![post.id.as_str()])
        .map_err(StoreError::Sqlite)?;

    // Media.
    for m in &post.media {
        tx.execute(
            "INSERT INTO media (post_id, kind, url, thumbnail_url) VALUES (?1, ?2, ?3, ?4)",
            params![post.id.as_str(), media_kind_str(&m.kind), m.url, m.thumbnail_url],
        )
        .map_err(StoreError::Sqlite)?;
    }

    // URLs.
    for u in &post.urls {
        tx.execute(
            "INSERT INTO urls (post_id, url, display_text) VALUES (?1, ?2, ?3)",
            params![post.id.as_str(), u.url, u.display_text],
        )
        .map_err(StoreError::Sqlite)?;
    }

    // Mentions.
    for mention in &post.mentions {
        tx.execute(
            "INSERT INTO mentions (post_id, username, user_id) VALUES (?1, ?2, ?3)",
            params![
                post.id.as_str(),
                mention.username,
                mention.user_id.as_ref().map(|u| u.as_str()),
            ],
        )
        .map_err(StoreError::Sqlite)?;
    }

    // Edges: reply (post → parent), root (post → root), mention, quote.
    if let Some(parent) = &post.parent_id {
        tx.execute(
            "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'reply')",
            params![post.id.as_str(), parent.as_str()],
        )
        .map_err(StoreError::Sqlite)?;
    }
    if let Some(root) = &post.root_id {
        tx.execute(
            "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'root')",
            params![post.id.as_str(), root.as_str()],
        )
        .map_err(StoreError::Sqlite)?;
    }
    for mention in &post.mentions {
        if let Some(uid) = &mention.user_id {
            tx.execute(
                "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'mention')",
                params![post.id.as_str(), uid.as_str()],
            )
            .map_err(StoreError::Sqlite)?;
        }
    }
    if post.is_quote_post {
        if let Some(parent) = &post.parent_id {
            tx.execute(
                "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'quote')",
                params![post.id.as_str(), parent.as_str()],
            )
            .map_err(StoreError::Sqlite)?;
        }
    }

    // Raw payload.
    if let Some(raw) = &post.raw {
        let payload_str = serde_json::to_string(raw).map_err(StoreError::Serde)?;
        tx.execute(
            "INSERT INTO raw_payloads (post_id, provider, fetch_run_id, payload, fetched_at)
             VALUES (?1, 'unknown', ?2, ?3, ?4)",
            params![post.id.as_str(), fetch_run_id, payload_str, &now],
        )
        .map_err(StoreError::Sqlite)?;
    }

    Ok(())
}

/// Upsert a single post (opens its own transaction).
pub fn upsert_post(conn: &mut Connection, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    upsert_post_tx(&tx, post, fetch_run_id)?;
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

/// Batch-upsert a slice of posts in a single transaction.
/// Returns the number of posts successfully upserted.
pub fn upsert_posts(
    conn: &mut Connection,
    posts: &[Post],
    fetch_run_id: Option<&str>,
) -> Result<usize> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    for post in posts {
        upsert_post_tx(&tx, post, fetch_run_id)?;
    }
    let n = posts.len();
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(n)
}

// ------------------------------------------------------------------ //
//  Retrieval                                                          //
// ------------------------------------------------------------------ //

/// Load a post with its media, urls, and mentions from the DB.
fn load_post(conn: &Connection, id: &str) -> Result<Option<Post>> {
    type PostRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i32,
    );
    let row: Option<PostRow> = conn.query_row(
            "SELECT author_id, text, created_at, parent_id, root_id, permalink, is_quote_post
             FROM posts WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)?;

    let Some((author_id, text, created_at_str, parent_id, root_id, permalink, is_quote)) = row
    else {
        return Ok(None);
    };

    let created_at = created_at_str
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    // Media.
    let mut media_stmt = conn
        .prepare("SELECT kind, url, thumbnail_url FROM media WHERE post_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let media: Vec<Media> = media_stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?))
        })
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .map(|(k, url, thumb)| Media {
            kind: media_kind_from_str(&k),
            url,
            thumbnail_url: thumb,
        })
        .collect();

    // URLs.
    let mut url_stmt = conn
        .prepare("SELECT url, display_text FROM urls WHERE post_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let urls: Vec<UrlEntity> = url_stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .map(|(url, display_text)| UrlEntity { url, display_text })
        .collect();

    // Mentions.
    let mut mention_stmt = conn
        .prepare("SELECT username, user_id FROM mentions WHERE post_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let mentions: Vec<Mention> = mention_stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .map(|(username, user_id)| Mention {
            username,
            user_id: user_id.map(UserId::new),
        })
        .collect();

    Ok(Some(Post {
        id: PostId::new(id),
        author: UserId::new(author_id),
        text,
        created_at,
        parent_id: parent_id.map(PostId::new),
        root_id: root_id.map(PostId::new),
        permalink,
        media,
        urls,
        mentions,
        is_quote_post: is_quote != 0,
        raw: None,
    }))
}

pub fn get_post(conn: &Connection, id: &PostId) -> Result<Option<Post>> {
    load_post(conn, id.as_str())
}

// ------------------------------------------------------------------ //
//  FTS5 search                                                        //
// ------------------------------------------------------------------ //

/// Full-text search over posts, ranked by BM25.  Returns up to `limit` posts.
pub fn search_text(conn: &Connection, query_str: &str, limit: usize) -> Result<Vec<Post>> {
    let mut stmt = conn
        .prepare(
            "SELECT p.id FROM posts p
             JOIN posts_fts f ON p.rowid = f.rowid
             WHERE posts_fts MATCH ?1
             ORDER BY bm25(posts_fts)
             LIMIT ?2",
        )
        .map_err(StoreError::Sqlite)?;

    let ids: Vec<String> = stmt
        .query_map(params![query_str, limit as i64], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .collect();

    let mut posts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(post) = load_post(conn, &id)? {
            posts.push(post);
        }
    }
    Ok(posts)
}

// ------------------------------------------------------------------ //
//  Thread traversal (recursive CTE)                                   //
// ------------------------------------------------------------------ //

/// Return all posts in the thread rooted at `root_id`, in BFS order.
/// Uses a recursive CTE over the `edges` table (kind='reply').
pub fn thread_rooted_at(conn: &Connection, root_id: &PostId) -> Result<Vec<Post>> {
    // The CTE walks reply edges: a post P is in the thread if:
    //   - P.id = root_id (anchor), OR
    //   - there is an edge (P.id, ancestor.id, 'reply') where ancestor is
    //     already in the result set.
    //
    // We traverse from root downward: for each known node, find posts that
    // reply to it (i.e., edges where to_id = known_node and kind='reply').
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE thread(id, depth) AS (
                 -- anchor: the root post itself
                 SELECT ?1, 0
                 UNION ALL
                 -- replies: posts whose parent is a known thread node
                 SELECT e.from_id, t.depth + 1
                 FROM edges e
                 JOIN thread t ON e.to_id = t.id AND e.kind = 'reply'
             )
             SELECT DISTINCT p.id FROM posts p
             JOIN thread t ON p.id = t.id
             ORDER BY t.depth, p.created_at",
        )
        .map_err(StoreError::Sqlite)?;

    let ids: Vec<String> = stmt
        .query_map(params![root_id.as_str()], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .collect();

    let mut posts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(post) = load_post(conn, &id)? {
            posts.push(post);
        }
    }
    Ok(posts)
}

// ------------------------------------------------------------------ //
//  Fetch runs                                                         //
// ------------------------------------------------------------------ //

pub fn record_fetch_run_start(conn: &Connection, run: &FetchRun) -> Result<()> {
    conn.execute(
        "INSERT INTO fetch_runs (id, provider, started_at, finished_at, posts_fetched, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO NOTHING",
        params![
            run.id,
            run.provider,
            run.started_at.to_rfc3339(),
            run.finished_at.map(|dt| dt.to_rfc3339()),
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

// ------------------------------------------------------------------ //
//  Edge kind (kept for potential direct use)                          //
// ------------------------------------------------------------------ //

#[allow(dead_code)]
fn _edge_kind_from_str(s: &str) -> Option<EdgeKind> {
    match s {
        "reply" => Some(EdgeKind::Reply),
        "root" => Some(EdgeKind::Root),
        "mention" => Some(EdgeKind::Mention),
        "quote" => Some(EdgeKind::Quote),
        _ => None,
    }
}
