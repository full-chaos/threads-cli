use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------- ID newtypes ----------

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PostId(pub String);

impl PostId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);

// ---------- Pagination ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Cursor>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next: Option<Cursor>) -> Self {
        Self { items, next }
    }
    pub fn empty() -> Self {
        Self { items: Vec::new(), next: None }
    }
}

// ---------- Entities ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: Option<String>,
    pub name: Option<String>,
    pub biography: Option<String>,
    pub profile_picture_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: PostId,
    pub author: UserId,
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

// ---------- Edges (explicit graph structure) ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Reply,
    Root,
    Mention,
    Quote,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

// ---------- Provenance ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FetchRun {
    pub id: String,
    pub provider: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub posts_fetched: u64,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_roundtrip_json() {
        let post = Post {
            id: PostId::new("123"),
            author: UserId::new("u1"),
            text: Some("hello threads".into()),
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };
        let s = serde_json::to_string(&post).unwrap();
        let parsed: Post = serde_json::from_str(&s).unwrap();
        assert_eq!(post, parsed);
    }

    #[test]
    fn page_empty() {
        let p: Page<Post> = Page::empty();
        assert!(p.items.is_empty());
        assert!(p.next.is_none());
    }
}
