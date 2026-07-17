use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{PostId, UserId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: Option<String>,
    pub name: Option<String>,
    pub biography: Option<String>,
    pub profile_picture_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Carousel,
    Audio,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub kind: MediaKind,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UrlEntity {
    pub url: String,
    pub display_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mention {
    pub username: String,
    pub user_id: Option<UserId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: PostId,
    pub author: UserId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_username: Option<String>,
    pub text: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub parent_id: Option<PostId>,
    pub root_id: Option<PostId>,
    pub permalink: Option<String>,
    pub media: Vec<Media>,
    pub urls: Vec<UrlEntity>,
    pub mentions: Vec<Mention>,
    pub is_quote_post: bool,
    /// Raw provider payload retained per PRD for replay/re-normalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl Post {
    fn prefer_real(incoming: UserId, existing: UserId) -> UserId {
        match (
            incoming.as_str().starts_with('@'),
            existing.as_str().starts_with('@'),
        ) {
            (false, true) => incoming,
            (true, false) => existing,
            _ => incoming,
        }
    }

    /// Merge a re-fetched `incoming` post onto an `existing` stored post,
    /// never losing data to a sparser fetch.
    /// - text/created_at/permalink/parent_id/root_id: `incoming.or(existing)`
    /// - author: prefer a real id over an `@handle`
    /// - is_quote_post: sticky true (`existing || incoming`)
    /// - media/urls/mentions: incoming unless empty, then existing
    /// - id/raw: always incoming
    pub fn merge(existing: Post, incoming: Post) -> Post {
        Post {
            id: incoming.id,
            author: Self::prefer_real(incoming.author, existing.author),
            author_username: incoming.author_username.or(existing.author_username),
            text: incoming.text.or(existing.text),
            created_at: incoming.created_at.or(existing.created_at),
            parent_id: incoming.parent_id.or(existing.parent_id),
            root_id: incoming.root_id.or(existing.root_id),
            permalink: incoming.permalink.or(existing.permalink),
            is_quote_post: existing.is_quote_post || incoming.is_quote_post,
            media: if incoming.media.is_empty() {
                existing.media
            } else {
                incoming.media
            },
            urls: if incoming.urls.is_empty() {
                existing.urls
            } else {
                incoming.urls
            },
            mentions: if incoming.mentions.is_empty() {
                existing.mentions
            } else {
                incoming.mentions
            },
            raw: incoming.raw,
        }
    }
}
