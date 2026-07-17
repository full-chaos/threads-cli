use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use threads_core::model::{Media, MediaKind, Mention, Post, PostId, UrlEntity, UserId};

use crate::error::{Result, StoreError};

pub(super) fn media_kind_str(kind: &MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Carousel => "carousel",
        MediaKind::Audio => "audio",
        MediaKind::Unknown => "unknown",
    }
}

fn media_kind_from_str(value: &str) -> MediaKind {
    match value {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        "carousel" => MediaKind::Carousel,
        "audio" => MediaKind::Audio,
        _ => MediaKind::Unknown,
    }
}

pub(super) fn load_post(conn: &Connection, id: &str) -> Result<Option<Post>> {
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
        .and_then(|value| value.parse::<DateTime<Utc>>().ok());
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
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?
        .into_iter()
        .map(|(kind, url, thumbnail_url)| Media {
            kind: media_kind_from_str(&kind),
            url,
            thumbnail_url,
        })
        .collect();
    let mut url_stmt = conn
        .prepare("SELECT url, display_text FROM urls WHERE post_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let urls: Vec<UrlEntity> = url_stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?
        .into_iter()
        .map(|(url, display_text)| UrlEntity { url, display_text })
        .collect();
    let mut mention_stmt = conn
        .prepare("SELECT username, user_id FROM mentions WHERE post_id = ?1")
        .map_err(StoreError::Sqlite)?;
    let mentions: Vec<Mention> = mention_stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?
        .into_iter()
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

pub(super) fn load_posts(conn: &Connection, ids: Vec<String>) -> Result<Vec<Post>> {
    ids.into_iter()
        .map(|id| load_post(conn, &id))
        .collect::<Result<Vec<_>>>()
        .map(|posts| posts.into_iter().flatten().collect())
}
