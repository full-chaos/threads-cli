use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use threads_core::model::{
    AudienceSnapshot, DemographicBucket, DemographicDimension, EdgeKind, FetchRun, Media,
    MediaKind, Mention, Post, PostId, UrlEntity, User, UserId,
};
use tracing::warn;

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

fn demographic_dimension_str(dimension: DemographicDimension) -> &'static str {
    match dimension {
        DemographicDimension::Country => "country",
        DemographicDimension::City => "city",
        DemographicDimension::Age => "age",
        DemographicDimension::Gender => "gender",
    }
}

fn demographic_dimension_from_str(value: &str) -> Result<DemographicDimension> {
    match value {
        "country" => Ok(DemographicDimension::Country),
        "city" => Ok(DemographicDimension::City),
        "age" => Ok(DemographicDimension::Age),
        "gender" => Ok(DemographicDimension::Gender),
        _ => Err(StoreError::InvalidData(format!(
            "unknown demographic dimension {value}"
        ))),
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

    // Read-merge-write: never lose richer stored data to a sparser re-fetch.
    // We only merge when the incoming post looks sparse — specifically when its
    // author is a `@username` sentinel (set by the provider when no numeric id
    // is available). A non-sentinel author means this is an intentional update
    // (the caller has full data), so we skip the merge and let the upsert
    // overwrite normally.
    let post_owned: Post;
    let post = if post.author.as_str().starts_with('@') {
        if let Some(existing) = load_post(tx, post.id.as_str())? {
            post_owned = Post::merge(existing, post.clone());
            &post_owned
        } else {
            post
        }
    } else {
        post
    };

    // Ensure the author stub exists so the FK is satisfied.
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, ?3)
         ON CONFLICT(id) DO UPDATE SET
             username = COALESCE(excluded.username, users.username),
             updated_at = excluded.updated_at",
        params![post.author.as_str(), post.author_username, &now],
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
    tx.execute(
        "DELETE FROM media WHERE post_id = ?1",
        params![post.id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute(
        "DELETE FROM urls WHERE post_id = ?1",
        params![post.id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute(
        "DELETE FROM mentions WHERE post_id = ?1",
        params![post.id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;
    // Also drop existing edges OWNED by this post (`from_id = post.id`) for
    // the kinds we manage here. Without this, reingesting a post whose
    // parent_id / root_id / mentions changed would LEAVE behind stale
    // rows — recursive thread traversal would keep returning posts in
    // threads they no longer belong to. `INSERT OR IGNORE` below only
    // dedups, it doesn't reconcile.
    tx.execute(
        "DELETE FROM edges
         WHERE from_id = ?1 AND kind IN ('reply','root','mention','quote')",
        params![post.id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;

    // Media.
    for m in &post.media {
        tx.execute(
            "INSERT INTO media (post_id, kind, url, thumbnail_url) VALUES (?1, ?2, ?3, ?4)",
            params![
                post.id.as_str(),
                media_kind_str(&m.kind),
                m.url,
                m.thumbnail_url
            ],
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

pub fn upsert_audience_snapshot(
    conn: &mut Connection,
    snapshot: &AudienceSnapshot,
    fetch_run_id: Option<&str>,
) -> Result<()> {
    let followers_count = i64::try_from(snapshot.followers_count).map_err(|_| {
        StoreError::InvalidData("followers count exceeds SQLite INTEGER range".into())
    })?;
    let observed_at = snapshot.observed_at.to_rfc3339();
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, NULL, NULL, NULL, NULL, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![snapshot.account_id.as_str(), now],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute(
        "INSERT INTO audience_snapshots (account_id, observed_at, followers_count, fetch_run_id)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id, observed_at) DO UPDATE SET
             followers_count = excluded.followers_count,
             fetch_run_id = excluded.fetch_run_id",
        params![
            snapshot.account_id.as_str(),
            observed_at,
            followers_count,
            fetch_run_id,
        ],
    )
    .map_err(StoreError::Sqlite)?;
    let snapshot_id: i64 = tx
        .query_row(
            "SELECT id FROM audience_snapshots WHERE account_id = ?1 AND observed_at = ?2",
            params![snapshot.account_id.as_str(), observed_at],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    tx.execute(
        "DELETE FROM audience_demographics WHERE snapshot_id = ?1",
        params![snapshot_id],
    )
    .map_err(StoreError::Sqlite)?;
    for demographic in &snapshot.demographics {
        let value = i64::try_from(demographic.value).map_err(|_| {
            StoreError::InvalidData("demographic value exceeds SQLite INTEGER range".into())
        })?;
        tx.execute(
            "INSERT INTO audience_demographics (snapshot_id, dimension, bucket, value)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot_id,
                demographic_dimension_str(demographic.dimension),
                demographic.bucket,
                value,
            ],
        )
        .map_err(StoreError::Sqlite)?;
    }
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn audience_history(
    conn: &Connection,
    account_id: &UserId,
    limit: usize,
) -> Result<Vec<AudienceSnapshot>> {
    let limit = i64::try_from(limit).map_err(|_| {
        StoreError::InvalidData("history limit exceeds SQLite INTEGER range".into())
    })?;
    let mut snapshot_stmt = conn
        .prepare(
            "SELECT id, observed_at, followers_count
             FROM (
                 SELECT id, observed_at, followers_count
                 FROM audience_snapshots
                 WHERE account_id = ?1
                 ORDER BY observed_at DESC
                 LIMIT ?2
             )
             ORDER BY observed_at ASC",
        )
        .map_err(StoreError::Sqlite)?;
    let snapshots: Vec<(i64, String, i64)> = snapshot_stmt
        .query_map(params![account_id.as_str(), limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?;
    let mut demographic_stmt = conn
        .prepare(
            "SELECT dimension, bucket, value
             FROM audience_demographics
             WHERE snapshot_id = ?1
             ORDER BY dimension ASC, bucket ASC",
        )
        .map_err(StoreError::Sqlite)?;
    let mut history = Vec::with_capacity(snapshots.len());
    for (snapshot_id, observed_at, followers_count) in snapshots {
        let observed_at = DateTime::parse_from_rfc3339(&observed_at)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?
            .with_timezone(&Utc);
        let followers_count = u64::try_from(followers_count)
            .map_err(|_| StoreError::InvalidData("negative audience follower count".into()))?;
        let demographics: Vec<(String, String, i64)> = demographic_stmt
            .query_map(params![snapshot_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;
        let demographics = demographics
            .into_iter()
            .map(|(dimension, bucket, value)| {
                Ok(DemographicBucket {
                    dimension: demographic_dimension_from_str(&dimension)?,
                    bucket,
                    value: u64::try_from(value).map_err(|_| {
                        StoreError::InvalidData("negative demographic value".into())
                    })?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        history.push(AudienceSnapshot {
            account_id: account_id.clone(),
            observed_at,
            followers_count,
            demographics,
        });
    }
    Ok(history)
}

pub fn count_audience_snapshots_before(
    conn: &Connection,
    account_id: &UserId,
    cutoff: DateTime<Utc>,
) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audience_snapshots
             WHERE account_id = ?1 AND observed_at < ?2",
            params![account_id.as_str(), cutoff.to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    u64::try_from(count).map_err(|_| StoreError::InvalidData("negative snapshot count".into()))
}

pub fn delete_audience_snapshots_before(
    conn: &mut Connection,
    account_id: &UserId,
    cutoff: DateTime<Utc>,
) -> Result<u64> {
    let deleted = conn
        .execute(
            "DELETE FROM audience_snapshots
             WHERE account_id = ?1 AND observed_at < ?2",
            params![account_id.as_str(), cutoff.to_rfc3339()],
        )
        .map_err(StoreError::Sqlite)?;
    u64::try_from(deleted)
        .map_err(|_| StoreError::InvalidData("negative deleted snapshot count".into()))
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
        Option<String>,
        i32,
    );
    let row: Option<PostRow> = conn
        .query_row(
            "SELECT p.author_id, u.username, p.text, p.created_at, p.parent_id, p.root_id,
                    p.permalink, p.is_quote_post
             FROM posts p
             JOIN users u ON u.id = p.author_id
             WHERE p.id = ?1",
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
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)?;

    let Some((
        author_id,
        author_username,
        text,
        created_at_str,
        parent_id,
        root_id,
        permalink,
        is_quote,
    )) = row
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
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
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
        author_username,
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

/// Return every post id where `author_id = ?1`. Used by the ingestion
/// orchestrator to enumerate "posts I authored" as BFS seeds for
/// engagement (replies-to-my-stuff) crawls.
pub fn posts_by_author(conn: &Connection, author: &UserId) -> Result<Vec<PostId>> {
    let mut stmt = conn
        .prepare("SELECT id FROM posts WHERE author_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let rows: Vec<PostId> = stmt
        .query_map(params![author.as_str()], |row| {
            row.get::<_, String>(0).map(PostId::new)
        })
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Return up to `limit` posts ordered by `fetched_at DESC`. Used for
/// enumeration (export / list) since FTS5 can't match "all posts" cleanly.
pub fn list_posts(conn: &Connection, limit: usize) -> Result<Vec<Post>> {
    let mut stmt = conn
        .prepare("SELECT id FROM posts ORDER BY fetched_at DESC LIMIT ?1")
        .map_err(StoreError::Sqlite)?;
    let ids: Vec<String> = stmt
        .query_map(params![limit as i64], |row| row.get(0))
        .map_err(StoreError::Sqlite)?
        .filter_map(|r| r.ok())
        .collect();
    let mut posts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(p) = load_post(conn, &id)? {
            posts.push(p);
        }
    }
    Ok(posts)
}

// ----- Deletions -----

/// Selects whether deletion candidate queries return top-level posts or replies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostKind {
    Post,
    Reply,
}

impl PostKind {
    fn as_str(self) -> &'static str {
        match self {
            PostKind::Post => "post",
            PostKind::Reply => "reply",
        }
    }
}

/// Posts in [after, before) authored by `author`, matched on non-NULL `created_at`.
/// `kind` selects root posts (parent_id IS NULL) vs replies (parent_id NOT NULL).
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
    let after = after.map(|dt| dt.to_rfc3339());
    let before = before.map(|dt| dt.to_rfc3339());

    let ids: Vec<String> = match (after.as_deref(), before.as_deref()) {
        (Some(after), Some(before)) => {
            let sql = format!(
                "SELECT id FROM posts
                 WHERE author_id = ?1
                   AND created_at IS NOT NULL
                   AND {parent_filter}
                   AND created_at >= ?2
                   AND created_at < ?3
                 ORDER BY created_at ASC
                 LIMIT ?4"
            );
            let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
            stmt.query_map(
                params![author.as_str(), after, before, limit as i64],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)?
            .filter_map(|r| r.ok())
            .collect()
        }
        (Some(after), None) => {
            let sql = format!(
                "SELECT id FROM posts
                 WHERE author_id = ?1
                   AND created_at IS NOT NULL
                   AND {parent_filter}
                   AND created_at >= ?2
                 ORDER BY created_at ASC
                 LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
            stmt.query_map(params![author.as_str(), after, limit as i64], |row| {
                row.get(0)
            })
            .map_err(StoreError::Sqlite)?
            .filter_map(|r| r.ok())
            .collect()
        }
        (None, Some(before)) => {
            let sql = format!(
                "SELECT id FROM posts
                 WHERE author_id = ?1
                   AND created_at IS NOT NULL
                   AND {parent_filter}
                   AND created_at < ?2
                 ORDER BY created_at ASC
                 LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
            stmt.query_map(params![author.as_str(), before, limit as i64], |row| {
                row.get(0)
            })
            .map_err(StoreError::Sqlite)?
            .filter_map(|r| r.ok())
            .collect()
        }
        (None, None) => {
            let sql = format!(
                "SELECT id FROM posts
                 WHERE author_id = ?1
                   AND created_at IS NOT NULL
                   AND {parent_filter}
                 ORDER BY created_at ASC
                 LIMIT ?2"
            );
            let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
            stmt.query_map(params![author.as_str(), limit as i64], |row| row.get(0))
                .map_err(StoreError::Sqlite)?
                .filter_map(|r| r.ok())
                .collect()
        }
    };

    let mut posts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(post) = load_post(conn, &id)? {
            posts.push(post);
        }
    }
    Ok(posts)
}

/// Hard-delete a post by id.
///
/// `media`, `urls`, `mentions`, and `raw_payloads` are removed by SQLite
/// foreign-key CASCADE (declared in migration v1). The `edges` table has no
/// FK to `posts`, so we DELETE its rows in both directions explicitly,
/// otherwise stale edges would orphan the recursive-CTE thread traversal.
///
/// Idempotent: returns `Ok(false)` if the row was not present.
/// Wrapped in a single transaction so a partial failure leaves nothing
/// half-deleted.
pub fn delete_post(conn: &mut Connection, id: &PostId) -> Result<bool> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    // `edges` has no ON DELETE CASCADE; clear both endpoints manually.
    tx.execute(
        "DELETE FROM edges WHERE from_id = ?1 OR to_id = ?1",
        params![id.as_str()],
    )
    .map_err(StoreError::Sqlite)?;
    let n = tx
        .execute("DELETE FROM posts WHERE id = ?1", params![id.as_str()])
        .map_err(StoreError::Sqlite)?;
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(n > 0)
}

/// Append a row to `deletions`. NEVER fails the caller if the audit insert
/// fails — log and continue (loss of audit must not abort actual deletion).
pub fn record_deletion(
    conn: &Connection,
    id: &PostId,
    kind: PostKind,
    success: bool,
    error: Option<&str>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    if let Err(err) = conn.execute(
        "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.as_str(), kind.as_str(), now, success as i32, error],
    ) {
        warn!(post_id = id.as_str(), error = %err, "failed to record deletion audit row");
    }
    Ok(())
}

/// Count rows in `deletions` with deleted_at >= now - 24h AND success = 1.
/// Used for the 100/24h pre-flight rate-limit check.
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

/// Return the timestamp of the OLDEST successful deletion still inside the
/// 24h window, or `None` if there have been no recent successful deletions.
///
/// CLI uses this to render `quota resets at <oldest + 24h>` when the cap is
/// hit, so the user can see exactly when the next slot opens up.
pub fn oldest_deletion_in_last_24h(conn: &Connection) -> Result<Option<DateTime<Utc>>> {
    let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let row: Option<String> = conn
        .query_row(
            "SELECT MIN(deleted_at) FROM deletions
             WHERE deleted_at >= ?1 AND success = 1",
            params![cutoff],
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)?
        .flatten();
    Ok(row.and_then(|s| {
        DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }))
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

// ------------------------------------------------------------------ //
//  Author resolution                                                  //
// ------------------------------------------------------------------ //

/// Resolve a synthesized `@username` placeholder author to a real numeric id.
/// In one transaction: upsert the real user, rewrite `posts.author_id` from
/// `'@' || username` to `real_id`, then delete the placeholder user row.
/// Idempotent.
pub fn resolve_author(conn: &mut Connection, username: &str, real_id: &UserId) -> Result<()> {
    let placeholder = format!("@{username}");
    let now = Utc::now().to_rfc3339();
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;

    // 1. Upsert the real user stub (preserves any existing richer row).
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, ?3)
         ON CONFLICT(id) DO UPDATE SET
             username   = COALESCE(excluded.username, users.username),
             updated_at = excluded.updated_at",
        params![real_id.as_str(), username, now],
    )
    .map_err(StoreError::Sqlite)?;

    // 2. Rewrite posts authored under the placeholder (must precede the DELETE
    //    so the FK cascade finds no rows still pointing at the placeholder).
    tx.execute(
        "UPDATE posts SET author_id = ?1 WHERE author_id = ?2",
        params![real_id.as_str(), &placeholder],
    )
    .map_err(StoreError::Sqlite)?;

    // 3. Remove the now-orphaned placeholder user row.
    tx.execute("DELETE FROM users WHERE id = ?1", params![&placeholder])
        .map_err(StoreError::Sqlite)?;

    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

// ------------------------------------------------------------------ //
//  Test-only probes                                                   //
// ------------------------------------------------------------------ //

#[cfg(test)]
pub(crate) fn test_only_count_edges_from(store: &crate::Store, from: &str) -> i64 {
    let conn = store.raw_conn();
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE from_id = ?1 AND kind IN ('reply','root','mention','quote')",
        params![from],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn test_only_edge_target(
    store: &crate::Store,
    from: &str,
    kind: &str,
) -> Option<String> {
    let conn = store.raw_conn();
    conn.query_row(
        "SELECT to_id FROM edges WHERE from_id = ?1 AND kind = ?2",
        params![from, kind],
        |r| r.get::<_, String>(0),
    )
    .ok()
}
