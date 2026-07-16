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
        Self {
            items: Vec::new(),
            next: None,
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemographicDimension {
    Country,
    City,
    Age,
    Gender,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemographicBucket {
    pub dimension: DemographicDimension,
    pub bucket: String,
    pub value: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudienceSnapshot {
    pub account_id: UserId,
    pub observed_at: DateTime<Utc>,
    pub followers_count: u64,
    pub demographics: Vec<DemographicBucket>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceInsightQuery {
    FollowersCount,
    FollowerDemographics(DemographicDimension),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AudienceInsightResult {
    FollowersCount(u64),
    Demographics(AudienceSnapshot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngagementSort {
    Total,
    Replies,
    Mentions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagedAccount {
    pub user_id: UserId,
    pub username: Option<String>,
    pub replies: u64,
    pub mentions: u64,
    pub total: u64,
}

impl Post {
    /// Choose between an incoming and existing author id, preferring the
    /// non-`@`-prefixed (real numeric) id. Ties (both real or both handles)
    /// go to `incoming`.
    fn prefer_real(incoming: UserId, existing: UserId) -> UserId {
        match (
            incoming.as_str().starts_with('@'),
            existing.as_str().starts_with('@'),
        ) {
            (false, true) => incoming, // incoming real, existing handle
            (true, false) => existing, // incoming handle, existing real
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_roundtrip_json() {
        let post = Post {
            id: PostId::new("123"),
            author: UserId::new("u1"),
            author_username: None,
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

    // ------------------------------------------------------------------ //
    //  Post::merge                                                        //
    // ------------------------------------------------------------------ //

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn rich_post() -> Post {
        Post {
            id: PostId::new("p1"),
            author: UserId::new("123456"),
            author_username: None,
            text: Some("hello threads".into()),
            created_at: Some(ts("2026-01-01T00:00:00+00:00")),
            parent_id: Some(PostId::new("parent-root")),
            root_id: Some(PostId::new("parent-root")),
            permalink: Some("https://threads.net/p/abc".into()),
            media: vec![Media {
                kind: MediaKind::Image,
                url: Some("https://example.com/img.jpg".into()),
                thumbnail_url: None,
            }],
            urls: vec![UrlEntity {
                url: "https://example.com".into(),
                display_text: Some("example".into()),
            }],
            mentions: vec![Mention {
                username: "bob".into(),
                user_id: Some(UserId::new("789")),
            }],
            is_quote_post: true,
            raw: Some(serde_json::json!({ "id": "p1" })),
        }
    }

    fn sparse_post() -> Post {
        Post {
            id: PostId::new("p1"),
            author: UserId::new("@alice"),
            author_username: None,
            text: None,
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: Some(serde_json::json!({ "id": "p1", "sparse": true })),
        }
    }

    #[test]
    fn merge_keeps_known_created_at_when_incoming_none() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.created_at, existing.created_at);
    }

    #[test]
    fn merge_incoming_created_at_wins_when_both_present() {
        let mut incoming = sparse_post();
        incoming.created_at = Some(ts("2026-06-01T12:00:00+00:00"));
        let merged = Post::merge(rich_post(), incoming.clone());
        assert_eq!(merged.created_at, incoming.created_at);
    }

    #[test]
    fn merge_keeps_known_text_when_incoming_none() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.text, existing.text);
    }

    #[test]
    fn baseline_sparse_merge_preserves_known_text() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());

        assert_eq!(merged.text, existing.text);
    }

    #[test]
    fn merge_keeps_known_permalink_when_incoming_none() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.permalink, existing.permalink);
    }

    #[test]
    fn merge_keeps_known_parent_id_when_incoming_none() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.parent_id, existing.parent_id);
    }

    #[test]
    fn merge_keeps_known_root_id_when_incoming_none() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.root_id, existing.root_id);
    }

    #[test]
    fn merge_prefers_real_author_over_handle() {
        let existing = rich_post(); // author = "123456"
        let merged = Post::merge(existing.clone(), sparse_post()); // incoming "@alice"
        assert_eq!(merged.author, existing.author);
    }

    #[test]
    fn merge_incoming_real_author_beats_existing_handle() {
        let mut existing = rich_post();
        existing.author = UserId::new("@alice");
        let mut incoming = sparse_post();
        incoming.author = UserId::new("654321");
        let merged = Post::merge(existing, incoming.clone());
        assert_eq!(merged.author, incoming.author);
    }

    #[test]
    fn merge_both_handles_incoming_wins() {
        let mut existing = rich_post();
        existing.author = UserId::new("@alice");
        let mut incoming = sparse_post();
        incoming.author = UserId::new("@bob");
        let merged = Post::merge(existing, incoming.clone());
        assert_eq!(merged.author, incoming.author);
    }

    #[test]
    fn merge_is_quote_post_sticky_true() {
        let merged = Post::merge(rich_post(), sparse_post()); // true, then false
        assert!(merged.is_quote_post);
    }

    #[test]
    fn merge_is_quote_post_false_plus_true_becomes_true() {
        let mut existing = rich_post();
        existing.is_quote_post = false;
        let mut incoming = sparse_post();
        incoming.is_quote_post = true;
        assert!(Post::merge(existing, incoming).is_quote_post);
    }

    #[test]
    fn merge_keeps_media_when_incoming_empty() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.media, existing.media);
    }

    #[test]
    fn merge_incoming_media_wins_when_nonempty() {
        let mut incoming = sparse_post();
        incoming.media = vec![Media {
            kind: MediaKind::Video,
            url: Some("https://example.com/v.mp4".into()),
            thumbnail_url: None,
        }];
        let merged = Post::merge(rich_post(), incoming.clone());
        assert_eq!(merged.media, incoming.media);
    }

    #[test]
    fn merge_keeps_urls_when_incoming_empty() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.urls, existing.urls);
    }

    #[test]
    fn merge_keeps_mentions_when_incoming_empty() {
        let existing = rich_post();
        let merged = Post::merge(existing.clone(), sparse_post());
        assert_eq!(merged.mentions, existing.mentions);
    }

    #[test]
    fn merge_id_and_raw_always_from_incoming() {
        let incoming = sparse_post();
        let merged = Post::merge(rich_post(), incoming.clone());
        assert_eq!(merged.id, incoming.id);
        assert_eq!(merged.raw, incoming.raw);
    }

    #[test]
    fn demographic_dimensions_serialize_to_provider_wire_values() {
        assert_eq!(
            serde_json::to_value(DemographicDimension::Country).unwrap(),
            "country"
        );
        assert_eq!(
            serde_json::to_value(DemographicDimension::City).unwrap(),
            "city"
        );
        assert_eq!(
            serde_json::to_value(DemographicDimension::Age).unwrap(),
            "age"
        );
        assert_eq!(
            serde_json::to_value(DemographicDimension::Gender).unwrap(),
            "gender"
        );
    }

    #[test]
    fn audience_query_encodes_count_or_one_demographic_dimension() {
        assert_eq!(
            serde_json::to_value(AudienceInsightQuery::FollowersCount).unwrap(),
            serde_json::json!("followers_count")
        );
        assert_eq!(
            serde_json::to_value(AudienceInsightQuery::FollowerDemographics(
                DemographicDimension::Country
            ))
            .unwrap(),
            serde_json::json!({ "follower_demographics": "country" })
        );
    }

    #[test]
    fn audience_result_keeps_a_typed_demographic_snapshot() {
        let snapshot = AudienceSnapshot {
            account_id: UserId::new("account-1"),
            observed_at: ts("2026-01-01T00:00:00+00:00"),
            followers_count: 101,
            demographics: vec![DemographicBucket {
                dimension: DemographicDimension::Country,
                bucket: "US".into(),
                value: 80,
            }],
        };

        let result = AudienceInsightResult::Demographics(snapshot.clone());

        assert!(matches!(result, AudienceInsightResult::Demographics(value) if value == snapshot));
    }

    #[test]
    fn engaged_account_exposes_typed_counts_and_sort_values() {
        let account = EngagedAccount {
            user_id: UserId::new("account-1"),
            username: Some("alice".into()),
            replies: 4,
            mentions: 3,
            total: 7,
        };

        assert_eq!(account.total, account.replies + account.mentions);
        assert_eq!(
            serde_json::to_value(EngagementSort::Mentions).unwrap(),
            serde_json::json!("mentions")
        );
    }

    #[test]
    fn merge_preserves_known_author_username_when_incoming_is_sparse() {
        let mut existing = rich_post();
        existing.author_username = Some("alice".into());
        let mut incoming = sparse_post();
        incoming.author_username = None;

        let merged = Post::merge(existing, incoming);

        assert_eq!(merged.author_username.as_deref(), Some("alice"));
    }

    #[test]
    fn merge_upgrades_missing_author_username_from_incoming() {
        let mut existing = rich_post();
        existing.author_username = None;
        let mut incoming = sparse_post();
        incoming.author_username = Some("alice".into());

        let merged = Post::merge(existing, incoming);

        assert_eq!(merged.author_username.as_deref(), Some("alice"));
    }
}
