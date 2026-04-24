//! Normalizer trait and `OfficialNormalizer` implementation.
//!
//! Maps raw `graph.threads.net` JSON payloads into the stable internal model
//! defined in `threads-core`. Per the PRD: responses flow
//! `raw → typed provider DTO → normalizer → internal model`.

use serde_json::Value;
use threads_core::{Media, MediaKind, Post, PostId, User, UserId};

// ---------- Error ----------

/// Errors that can occur during normalization.
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    /// A required field was absent from the provider payload.
    #[error("missing field: {0}")]
    MissingField(&'static str),
    /// The payload had an unexpected shape (not an object, wrong array, etc.).
    #[error("invalid shape: {0}")]
    InvalidShape(String),
    /// JSON deserialization failed.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

// ---------- Normalizer trait ----------

/// Maps raw provider JSON into the stable internal model.
///
/// Implementations are provider-specific; call sites only know this trait,
/// enabling the architecture rule: "two providers, one store."
pub trait Normalizer: Send + Sync {
    /// Stable provider identifier, e.g. `"official"`.
    fn provider_name(&self) -> &'static str;

    /// Map a raw `/me` response into a [`User`].
    fn normalize_user(&self, raw: &Value) -> Result<User, NormalizeError>;

    /// Map a single raw post object into a [`Post`].
    ///
    /// `root_hint` carries the root thread id when normalizing replies fetched
    /// via `/replies`; pass `None` for top-level threads.
    fn normalize_post(
        &self,
        raw: &Value,
        root_hint: Option<&PostId>,
    ) -> Result<Post, NormalizeError>;

    /// Map a pagination envelope `{ data: [...], paging: { cursors: { after } } }`
    /// into a list of [`Post`]s and an optional next-page cursor string.
    fn normalize_page(
        &self,
        raw: &Value,
        root_hint: Option<&PostId>,
    ) -> Result<(Vec<Post>, Option<String>), NormalizeError>;
}

// ---------- OfficialNormalizer ----------

/// Normalizer for `graph.threads.net` v1.0 responses.
pub struct OfficialNormalizer;

impl OfficialNormalizer {
    /// Parse media attachments from a single raw post object.
    fn parse_media(obj: &Value) -> Vec<Media> {
        let media_type = obj
            .get("media_type")
            .and_then(|v| v.as_str())
            .unwrap_or("TEXT_POST");

        match media_type {
            "CAROUSEL_ALBUM" => {
                // Walk children.data for individual carousel items.
                if let Some(items) = obj
                    .get("children")
                    .and_then(|c| c.get("data"))
                    .and_then(|d| d.as_array())
                {
                    return items
                        .iter()
                        .map(|item| {
                            let kind = match item
                                .get("media_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("IMAGE")
                            {
                                "VIDEO" => MediaKind::Video,
                                "AUDIO" => MediaKind::Audio,
                                _ => MediaKind::Image,
                            };
                            Media {
                                kind,
                                url: item
                                    .get("media_url")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                thumbnail_url: item
                                    .get("thumbnail_url")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                            }
                        })
                        .collect();
                }
                // Fallback: no children data found; return carousel placeholder.
                vec![Media {
                    kind: MediaKind::Carousel,
                    url: None,
                    thumbnail_url: None,
                }]
            }
            "IMAGE" => vec![Media {
                kind: MediaKind::Image,
                url: obj
                    .get("media_url")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                thumbnail_url: obj
                    .get("thumbnail_url")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            }],
            "VIDEO" => vec![Media {
                kind: MediaKind::Video,
                url: obj
                    .get("media_url")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                thumbnail_url: obj
                    .get("thumbnail_url")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            }],
            "AUDIO" => vec![Media {
                kind: MediaKind::Audio,
                url: obj
                    .get("media_url")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                thumbnail_url: None,
            }],
            // TEXT_POST and unknown types carry no media attachment.
            _ => vec![],
        }
    }
}

impl Normalizer for OfficialNormalizer {
    fn provider_name(&self) -> &'static str {
        "official"
    }

    /// Mapping:
    /// - `id` → `User.id`
    /// - `username` → `User.username`
    /// - `name` → `User.name`
    /// - `threads_biography` → `User.biography`
    /// - `threads_profile_picture_url` → `User.profile_picture_url`
    fn normalize_user(&self, raw: &Value) -> Result<User, NormalizeError> {
        let obj = raw
            .as_object()
            .ok_or_else(|| NormalizeError::InvalidShape("user root is not an object".into()))?;

        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or(NormalizeError::MissingField("id"))?;

        Ok(User {
            id: UserId::new(id),
            username: obj
                .get("username")
                .and_then(|v| v.as_str())
                .map(String::from),
            name: obj.get("name").and_then(|v| v.as_str()).map(String::from),
            biography: obj
                .get("threads_biography")
                .and_then(|v| v.as_str())
                .map(String::from),
            profile_picture_url: obj
                .get("threads_profile_picture_url")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }

    /// Mapping:
    /// - `id` → `Post.id`
    /// - `owner.id` → `Post.author` (if absent, synthesize `@username`)
    /// - `text` → `Post.text`
    /// - `timestamp` (ISO 8601) → `Post.created_at`
    /// - `permalink` → `Post.permalink`
    /// - `is_quote_post` → `Post.is_quote_post`
    /// - `replied_to.id` → `Post.parent_id`
    /// - `root_post.id` → `Post.root_id` (falls back to `root_hint`)
    /// - `media_type` / `children.data` → `Post.media`
    /// - full raw JSON → `Post.raw`
    fn normalize_post(
        &self,
        raw: &Value,
        root_hint: Option<&PostId>,
    ) -> Result<Post, NormalizeError> {
        let obj = raw
            .as_object()
            .ok_or_else(|| NormalizeError::InvalidShape("post root is not an object".into()))?;

        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or(NormalizeError::MissingField("id"))?;

        // Author: prefer owner.id; fall back to synthesizing from username.
        let author = if let Some(owner_id) = obj
            .get("owner")
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
        {
            UserId::new(owner_id)
        } else if let Some(username) = obj.get("username").and_then(|v| v.as_str()) {
            UserId::new(format!("@{username}"))
        } else {
            return Err(NormalizeError::MissingField("owner.id or username"));
        };

        // Timestamp → created_at (ISO 8601).
        let created_at = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());

        // replied_to.id → parent_id.
        let parent_id = obj
            .get("replied_to")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .map(PostId::new);

        // root_post.id → root_id; fall back to caller-supplied hint.
        let root_id = obj
            .get("root_post")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .map(PostId::new)
            .or_else(|| root_hint.cloned());

        let is_quote_post = obj
            .get("is_quote_post")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let media = Self::parse_media(raw);

        Ok(Post {
            id: PostId::new(id),
            author,
            text: obj.get("text").and_then(|v| v.as_str()).map(String::from),
            created_at,
            parent_id,
            root_id,
            permalink: obj
                .get("permalink")
                .and_then(|v| v.as_str())
                .map(String::from),
            media,
            urls: vec![],
            mentions: vec![],
            is_quote_post,
            // Always retain the full raw JSON per PRD.
            raw: Some(raw.clone()),
        })
    }

    fn normalize_page(
        &self,
        raw: &Value,
        root_hint: Option<&PostId>,
    ) -> Result<(Vec<Post>, Option<String>), NormalizeError> {
        let obj = raw
            .as_object()
            .ok_or_else(|| NormalizeError::InvalidShape("page root is not an object".into()))?;

        let data = obj
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or(NormalizeError::MissingField("data"))?;

        let posts: Result<Vec<Post>, NormalizeError> = data
            .iter()
            .map(|item| self.normalize_post(item, root_hint))
            .collect();
        let posts = posts?;

        // paging.cursors.after → next cursor.
        let next_cursor = obj
            .get("paging")
            .and_then(|p| p.get("cursors"))
            .and_then(|c| c.get("after"))
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok((posts, next_cursor))
    }
}
