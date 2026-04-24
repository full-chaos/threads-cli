//! Integration tests for threads-ingest: normalizer + orchestrator.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use threads_core::{Cursor, FetchRun, Page, Post, PostId, Result, User, UserId};
use threads_ingest::{Ingestor, NormalizeError, Normalizer, OfficialNormalizer, StoreWrite};

// ---------- Fixtures (loaded from files) ----------

fn fixture(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("invalid JSON in {path}: {e}"))
}

// =============================================================================
// Normalizer tests
// =============================================================================

#[test]
fn normalize_user_me_json() {
    let raw = fixture("me.json");
    let norm = OfficialNormalizer;
    let user = norm.normalize_user(&raw).expect("normalize_user failed");

    assert_eq!(user.id, UserId::new("1234567890"));
    assert_eq!(user.username.as_deref(), Some("testuser"));
    assert_eq!(user.name.as_deref(), Some("Test User"));
    assert_eq!(
        user.biography.as_deref(),
        Some("Building things in public.")
    );
    assert_eq!(
        user.profile_picture_url.as_deref(),
        Some("https://example.com/pic.jpg")
    );
}

#[test]
fn normalize_user_missing_id_returns_error() {
    let raw = json!({ "username": "noId" });
    let norm = OfficialNormalizer;
    let err = norm.normalize_user(&raw).unwrap_err();
    assert!(
        matches!(err, NormalizeError::MissingField("id")),
        "expected MissingField(id), got {err}"
    );
}

#[test]
fn normalize_page_me_threads_json() {
    let raw = fixture("me_threads.json");
    let norm = OfficialNormalizer;
    let (posts, next) = norm.normalize_page(&raw, None).expect("normalize_page failed");

    assert_eq!(posts.len(), 3, "expected 3 posts");
    assert_eq!(next.as_deref(), Some("cursor_after_xyz"));

    let first = &posts[0];
    assert_eq!(first.id, PostId::new("post_001"));
    assert_eq!(first.author, UserId::new("1234567890"));
    assert_eq!(
        first.text.as_deref(),
        Some("Hello from Threads! This is my first post.")
    );
    assert!(first.raw.is_some(), "Post.raw must be retained");
}

#[test]
fn normalize_page_no_next_cursor_when_after_is_null() {
    let raw = fixture("replies.json");
    let norm = OfficialNormalizer;
    // replies.json has "after": null
    let (posts, next) = norm.normalize_page(&raw, None).expect("normalize_page failed");
    assert_eq!(posts.len(), 2);
    // null value → next_cursor = None
    assert!(next.is_none(), "next should be None when after is null");
}

#[test]
fn normalize_post_parent_and_root_edges_from_replies_json() {
    let raw = fixture("replies.json");
    let norm = OfficialNormalizer;
    let (posts, _) = norm.normalize_page(&raw, None).expect("normalize_page failed");

    let reply = &posts[0];
    assert_eq!(reply.id, PostId::new("reply_001"));
    assert_eq!(reply.parent_id, Some(PostId::new("post_001")));
    assert_eq!(reply.root_id, Some(PostId::new("post_001")));
}

#[test]
fn normalize_post_root_hint_fallback() {
    // A reply payload without root_post field — root_hint should fill in root_id.
    let raw = json!({
        "id": "reply_x",
        "owner": { "id": "user_y" },
        "text": "reply without root_post field",
        "timestamp": "2024-01-20T08:00:00+0000",
        "media_type": "TEXT_POST",
        "is_quote_post": false,
        "replied_to": { "id": "parent_z" }
    });
    let hint = PostId::new("thread_root_abc");
    let norm = OfficialNormalizer;
    let post = norm
        .normalize_post(&raw, Some(&hint))
        .expect("normalize_post failed");

    assert_eq!(post.parent_id, Some(PostId::new("parent_z")));
    assert_eq!(post.root_id, Some(PostId::new("thread_root_abc")));
}

#[test]
fn normalize_post_raw_retained() {
    let raw = json!({
        "id": "p1",
        "owner": { "id": "u1" },
        "media_type": "TEXT_POST",
        "is_quote_post": false
    });
    let norm = OfficialNormalizer;
    let post = norm.normalize_post(&raw, None).expect("normalize_post");
    assert_eq!(post.raw.as_ref().unwrap()["id"], "p1");
}

#[test]
fn normalize_post_carousel_walks_children() {
    let raw = json!({
        "id": "carousel_1",
        "owner": { "id": "u1" },
        "media_type": "CAROUSEL_ALBUM",
        "is_quote_post": false,
        "children": {
            "data": [
                { "media_type": "IMAGE", "media_url": "https://example.com/img1.jpg" },
                { "media_type": "VIDEO", "media_url": "https://example.com/vid1.mp4", "thumbnail_url": "https://example.com/thumb.jpg" }
            ]
        }
    });
    let norm = OfficialNormalizer;
    let post = norm.normalize_post(&raw, None).expect("normalize_post");
    assert_eq!(post.media.len(), 2);
    assert!(
        matches!(post.media[0].kind, threads_core::MediaKind::Image),
        "first child should be Image"
    );
    assert!(
        matches!(post.media[1].kind, threads_core::MediaKind::Video),
        "second child should be Video"
    );
    assert_eq!(
        post.media[1].thumbnail_url.as_deref(),
        Some("https://example.com/thumb.jpg")
    );
}

#[test]
fn normalize_post_synthesizes_author_from_username() {
    let raw = json!({
        "id": "p2",
        "username": "fallback_user",
        "media_type": "TEXT_POST",
        "is_quote_post": false
    });
    let norm = OfficialNormalizer;
    let post = norm.normalize_post(&raw, None).expect("normalize_post");
    assert_eq!(post.author, UserId::new("@fallback_user"));
}

// =============================================================================
// Orchestrator tests
// =============================================================================

/// A MockProvider that replays a fixed list of post pages.
struct MockProvider {
    /// Pages of posts to return from `fetch_my_threads`.
    /// Each inner Vec is one page. The last page has no next cursor.
    pages: Vec<Vec<Post>>,
}

impl MockProvider {
    fn new(pages: Vec<Vec<Post>>) -> Self {
        Self { pages }
    }

    fn make_post(id: &str, author: &str) -> Post {
        Post {
            id: PostId::new(id),
            author: UserId::new(author),
            text: Some(format!("text of {id}")),
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: Some(json!({ "id": id, "author": author })),
        }
    }
}

#[async_trait]
impl threads_core::Provider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn fetch_me(&self) -> Result<User> {
        Ok(User {
            id: UserId::new("mock_user"),
            username: Some("mock".into()),
            name: None,
            biography: None,
            profile_picture_url: None,
        })
    }

    async fn fetch_my_threads(&self, cursor: Option<Cursor>) -> Result<Page<Post>> {
        // Determine which page to return by cursor value.
        let idx = match &cursor {
            None => 0,
            Some(c) => c.0.parse::<usize>().unwrap_or(0),
        };
        if idx >= self.pages.len() {
            return Ok(Page::empty());
        }
        let items = self.pages[idx].clone();
        let next = if idx + 1 < self.pages.len() {
            Some(Cursor((idx + 1).to_string()))
        } else {
            None
        };
        Ok(Page::new(items, next))
    }

    async fn fetch_replies(
        &self,
        _post_id: &PostId,
        _cursor: Option<Cursor>,
    ) -> Result<Page<Post>> {
        Ok(Page::empty())
    }

    async fn fetch_thread(&self, _root_id: &PostId) -> Result<Vec<Post>> {
        Ok(vec![])
    }
}

/// State captured by MockStore.
#[derive(Default)]
struct MockStoreState {
    upserted: Vec<Post>,
    run_started: Vec<FetchRun>,
    run_ended: Vec<(String, u64, Option<String>)>,
}

struct MockStore {
    state: Mutex<MockStoreState>,
}

impl MockStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MockStoreState::default()),
        })
    }
}

impl StoreWrite for MockStore {
    fn upsert_posts(&self, posts: &[Post], _fetch_run_id: Option<&str>) -> Result<usize> {
        let mut s = self.state.lock().unwrap();
        s.upserted.extend_from_slice(posts);
        Ok(posts.len())
    }

    fn record_fetch_run_start(&self, run: &FetchRun) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.run_started.push(run.clone());
        Ok(())
    }

    fn record_fetch_run_end(
        &self,
        id: &str,
        _finished_at: DateTime<Utc>,
        posts_fetched: u64,
        error: Option<&str>,
    ) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.run_ended
            .push((id.to_string(), posts_fetched, error.map(String::from)));
        Ok(())
    }

    fn get_post(&self, id: &PostId) -> Result<Option<Post>> {
        let s = self.state.lock().unwrap();
        Ok(s.upserted.iter().find(|p| &p.id == id).cloned())
    }
}

/// NoopNormalizer: the orchestrator does not call the normalizer directly;
/// the MockProvider returns already-normalized `Post` values.
struct NoopNormalizer;

impl Normalizer for NoopNormalizer {
    fn provider_name(&self) -> &'static str {
        "mock"
    }
    fn normalize_user(&self, _raw: &serde_json::Value) -> std::result::Result<User, NormalizeError> {
        unimplemented!()
    }
    fn normalize_post(
        &self,
        _raw: &serde_json::Value,
        _root_hint: Option<&PostId>,
    ) -> std::result::Result<Post, NormalizeError> {
        unimplemented!()
    }
    fn normalize_page(
        &self,
        _raw: &serde_json::Value,
        _root_hint: Option<&PostId>,
    ) -> std::result::Result<(Vec<Post>, Option<String>), NormalizeError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn orchestrator_ingest_me_records_run_start_and_end() {
    let page1 = vec![
        MockProvider::make_post("p1", "u1"),
        MockProvider::make_post("p2", "u1"),
    ];
    let page2 = vec![MockProvider::make_post("p3", "u1")];

    let provider = Arc::new(MockProvider::new(vec![page1, page2]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor.ingest_me().await.expect("ingest_me failed");

    let state = store.state.lock().unwrap();

    // FetchRun start recorded once.
    assert_eq!(state.run_started.len(), 1, "expected exactly one run_start");

    // FetchRun end recorded once.
    assert_eq!(state.run_ended.len(), 1, "expected exactly one run_end");
    assert!(
        state.run_ended[0].2.is_none(),
        "run should have ended without error"
    );

    // All posts upserted.
    assert_eq!(state.upserted.len(), 3, "expected 3 upserted posts");

    // posts_fetched count matches.
    assert_eq!(run.posts_fetched, 3);
    assert_eq!(state.run_ended[0].1, 3, "run_end posts_fetched should be 3");
}

#[tokio::test]
async fn orchestrator_deduplicates_posts_within_run() {
    // Both pages contain the same post id — should be upserted only once.
    let dup = MockProvider::make_post("dup_post", "u1");
    let page1 = vec![dup.clone(), MockProvider::make_post("unique_1", "u1")];
    let page2 = vec![dup, MockProvider::make_post("unique_2", "u1")];

    let provider = Arc::new(MockProvider::new(vec![page1, page2]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor.ingest_me().await.expect("ingest_me failed");

    let state = store.state.lock().unwrap();
    // dup_post deduplicated: 1 + 1 + 1 = 3 unique posts.
    assert_eq!(state.upserted.len(), 3, "dedup should yield 3 unique posts");
    assert_eq!(run.posts_fetched, 3);
}

#[tokio::test]
async fn orchestrator_single_page_no_cursor() {
    let page = vec![MockProvider::make_post("only_post", "u1")];
    let provider = Arc::new(MockProvider::new(vec![page]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor.ingest_me().await.expect("ingest_me failed");
    assert_eq!(run.posts_fetched, 1);

    let state = store.state.lock().unwrap();
    assert_eq!(state.upserted.len(), 1);
}
