use chrono::Utc;
use rusqlite::{Connection, Transaction, params};
use threads_core::model::Post;

use crate::error::{Result, StoreError};

use super::row_conversion::{load_post, media_kind_str};
use super::users::reconcile_author_tx;

fn upsert_post_tx(tx: &Transaction, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
    let now = Utc::now().to_rfc3339();
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

    if !post.author.as_str().starts_with('@') {
        if let Some(username) = post
            .author_username
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            reconcile_author_tx(tx, username, &post.author, &now)?;
        }
    }

    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, ?3)
         ON CONFLICT(id) DO UPDATE SET
             username = COALESCE(excluded.username, users.username),
             updated_at = excluded.updated_at",
        params![post.author.as_str(), post.author_username, &now],
    )
    .map_err(StoreError::Sqlite)?;
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
            post.parent_id.as_ref().map(|parent| parent.as_str()),
            post.root_id.as_ref().map(|root| root.as_str()),
            post.permalink,
            post.is_quote_post as i32,
            &now,
        ],
    )
    .map_err(StoreError::Sqlite)?;

    for sql in [
        "DELETE FROM media WHERE post_id = ?1",
        "DELETE FROM urls WHERE post_id = ?1",
        "DELETE FROM mentions WHERE post_id = ?1",
        "DELETE FROM edges
         WHERE from_id = ?1 AND kind IN ('reply','root','mention','quote')",
    ] {
        tx.execute(sql, params![post.id.as_str()])
            .map_err(StoreError::Sqlite)?;
    }
    for media in &post.media {
        tx.execute(
            "INSERT INTO media (post_id, kind, url, thumbnail_url) VALUES (?1, ?2, ?3, ?4)",
            params![
                post.id.as_str(),
                media_kind_str(&media.kind),
                media.url,
                media.thumbnail_url
            ],
        )
        .map_err(StoreError::Sqlite)?;
    }
    for url in &post.urls {
        tx.execute(
            "INSERT INTO urls (post_id, url, display_text) VALUES (?1, ?2, ?3)",
            params![post.id.as_str(), url.url, url.display_text],
        )
        .map_err(StoreError::Sqlite)?;
    }
    for mention in &post.mentions {
        tx.execute(
            "INSERT INTO mentions (post_id, username, user_id) VALUES (?1, ?2, ?3)",
            params![
                post.id.as_str(),
                mention.username,
                mention.user_id.as_ref().map(|user_id| user_id.as_str()),
            ],
        )
        .map_err(StoreError::Sqlite)?;
    }
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
        if let Some(user_id) = &mention.user_id {
            tx.execute(
                "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'mention')",
                params![post.id.as_str(), user_id.as_str()],
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

pub fn upsert_post(conn: &mut Connection, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    upsert_post_tx(&tx, post, fetch_run_id)?;
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn upsert_posts(
    conn: &mut Connection,
    posts: &[Post],
    fetch_run_id: Option<&str>,
) -> Result<usize> {
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    for post in posts {
        upsert_post_tx(&tx, post, fetch_run_id)?;
    }
    let count = posts.len();
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(count)
}
