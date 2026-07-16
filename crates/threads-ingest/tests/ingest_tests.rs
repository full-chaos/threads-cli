//! Integration tests for threads-ingest: normalizer + orchestrator.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, AudienceSnapshot, Cursor, DemographicBucket,
    DemographicDimension, Error, FetchRun, Mention, Page, Post, PostId, Result, User, UserId,
};
use threads_ingest::{Ingestor, NormalizeError, Normalizer, OfficialNormalizer, StoreWrite};

// ---------- Fixtures (loaded from files) ----------

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
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
    let (posts, next) = norm
        .normalize_page(&raw, None)
        .expect("normalize_page failed");

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
    let (posts, next) = norm
        .normalize_page(&raw, None)
        .expect("normalize_page failed");
    assert_eq!(posts.len(), 2);
    // null value → next_cursor = None
    assert!(next.is_none(), "next should be None when after is null");
}

#[test]
fn normalize_post_parent_and_root_edges_from_replies_json() {
    let raw = fixture("replies.json");
    let norm = OfficialNormalizer;
    let (posts, _) = norm
        .normalize_page(&raw, None)
        .expect("normalize_page failed");

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
    assert_eq!(post.author_username.as_deref(), Some("fallback_user"));
}

// =============================================================================
// Orchestrator tests
// =============================================================================

/// A MockProvider that replays a fixed list of post pages.
struct MockProvider {
    /// Pages of posts to return from `fetch_my_threads`.
    /// Each inner Vec is one page. The last page has no next cursor.
    pages: Vec<Vec<Post>>,
    /// Flat list of posts to return from `fetch_thread` (root + descendants).
    thread_posts: Vec<Post>,
    /// Per-post-id reply lists for `fetch_replies` (single page, no pagination).
    replies: std::collections::HashMap<PostId, Vec<Post>>,
    /// Value returned from `fetch_me`.
    me: User,
    audience: Option<MockAudience>,
    audience_queries: Mutex<Vec<AudienceInsightQuery>>,
    mention_page: Option<Page<Post>>,
    mention_error: Option<MockProviderError>,
    mention_requests: Mutex<Vec<Option<Cursor>>>,
}

struct MockAudience {
    followers_count: u64,
    demographics: Vec<DemographicBucket>,
    fail_dimension: Option<DemographicDimension>,
}

#[derive(Clone, Copy)]
enum MockProviderError {
    PermissionDenied,
}

impl MockProvider {
    fn new(pages: Vec<Vec<Post>>) -> Self {
        Self {
            pages,
            thread_posts: vec![],
            replies: std::collections::HashMap::new(),
            me: User {
                id: UserId::new("mock_user"),
                username: Some("mock".into()),
                name: None,
                biography: None,
                profile_picture_url: None,
            },
            audience: None,
            audience_queries: Mutex::new(vec![]),
            mention_page: None,
            mention_error: None,
            mention_requests: Mutex::new(vec![]),
        }
    }

    fn with_thread(mut self, posts: Vec<Post>) -> Self {
        self.thread_posts = posts;
        self
    }

    fn with_me(mut self, me: User) -> Self {
        self.me = me;
        self
    }

    fn with_reply_to(mut self, parent: &PostId, replies: Vec<Post>) -> Self {
        self.replies.insert(parent.clone(), replies);
        self
    }

    fn with_audience(mut self, audience: MockAudience) -> Self {
        self.audience = Some(audience);
        self
    }

    fn with_mentions(mut self, page: Page<Post>) -> Self {
        self.mention_page = Some(page);
        self
    }

    fn with_mention_error(mut self, error: MockProviderError) -> Self {
        self.mention_error = Some(error);
        self
    }

    fn make_post(id: &str, author: &str) -> Post {
        Post {
            id: PostId::new(id),
            author: UserId::new(author),
            author_username: None,
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
        Ok(self.me.clone())
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

    async fn fetch_replies(&self, post_id: &PostId, _cursor: Option<Cursor>) -> Result<Page<Post>> {
        match self.replies.get(post_id) {
            Some(items) => Ok(Page::new(items.clone(), None)),
            None => Ok(Page::empty()),
        }
    }

    async fn fetch_thread(&self, _root_id: &PostId) -> Result<Vec<Post>> {
        Ok(self.thread_posts.clone())
    }

    async fn fetch_audience_insight(
        &self,
        user_id: &UserId,
        query: AudienceInsightQuery,
    ) -> Result<AudienceInsightResult> {
        self.audience_queries.lock().unwrap().push(query.clone());
        let audience = self
            .audience
            .as_ref()
            .ok_or_else(|| Error::NotSupported("mock audience".into()))?;
        match query {
            AudienceInsightQuery::FollowersCount => Ok(AudienceInsightResult::FollowersCount(
                audience.followers_count,
            )),
            AudienceInsightQuery::FollowerDemographics(dimension) => {
                if audience.fail_dimension == Some(dimension) {
                    return Err(Error::Parse(format!("{dimension:?} unavailable")));
                }
                Ok(AudienceInsightResult::Demographics(AudienceSnapshot {
                    account_id: user_id.clone(),
                    observed_at: Utc::now(),
                    followers_count: audience.followers_count,
                    demographics: audience
                        .demographics
                        .iter()
                        .filter(|bucket| bucket.dimension == dimension)
                        .cloned()
                        .collect(),
                }))
            }
        }
    }

    async fn fetch_mentions(
        &self,
        _user_id: &UserId,
        cursor: Option<Cursor>,
        _limit: usize,
    ) -> Result<Page<Post>> {
        self.mention_requests.lock().unwrap().push(cursor);
        if let Some(error) = self.mention_error {
            return match error {
                MockProviderError::PermissionDenied => {
                    Err(Error::PermissionDenied("threads_manage_mentions".into()))
                }
            };
        }
        Ok(self.mention_page.clone().unwrap_or_else(Page::empty))
    }
}

/// State captured by MockStore.
#[derive(Default)]
struct MockStoreState {
    upserted: Vec<Post>,
    run_started: Vec<FetchRun>,
    run_ended: Vec<(String, u64, Option<String>)>,
    upserted_users: Vec<User>,
    resolve_author_calls: Vec<(String, UserId)>,
    audience_snapshots: Vec<AudienceSnapshot>,
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

    fn upsert_audience_snapshot(
        &self,
        snapshot: &AudienceSnapshot,
        _fetch_run_id: Option<&str>,
    ) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.audience_snapshots.push(snapshot.clone());
        Ok(())
    }

    fn audience_history(&self, account_id: &UserId, limit: usize) -> Result<Vec<AudienceSnapshot>> {
        let state = self.state.lock().unwrap();
        Ok(state
            .audience_snapshots
            .iter()
            .filter(|snapshot| &snapshot.account_id == account_id)
            .take(limit)
            .cloned()
            .collect())
    }

    fn count_audience_snapshots_before(
        &self,
        account_id: &UserId,
        before: DateTime<Utc>,
    ) -> Result<u64> {
        let state = self.state.lock().unwrap();
        Ok(state
            .audience_snapshots
            .iter()
            .filter(|snapshot| &snapshot.account_id == account_id && snapshot.observed_at < before)
            .count() as u64)
    }

    fn delete_audience_snapshots_before(
        &self,
        account_id: &UserId,
        before: DateTime<Utc>,
    ) -> Result<u64> {
        let mut state = self.state.lock().unwrap();
        let previous_len = state.audience_snapshots.len();
        state.audience_snapshots.retain(|snapshot| {
            &snapshot.account_id != account_id || snapshot.observed_at >= before
        });
        Ok((previous_len - state.audience_snapshots.len()) as u64)
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

    fn posts_by_author(&self, author: &UserId) -> Result<Vec<PostId>> {
        let s = self.state.lock().unwrap();
        Ok(s.upserted
            .iter()
            .filter(|p| &p.author == author)
            .map(|p| p.id.clone())
            .collect())
    }

    fn upsert_user(&self, user: &User) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.upserted_users.push(user.clone());
        Ok(())
    }

    fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.resolve_author_calls
            .push((username.to_string(), real_id.clone()));
        Ok(())
    }
}

/// NoopNormalizer: the orchestrator does not call the normalizer directly;
/// the MockProvider returns already-normalized `Post` values.
struct NoopNormalizer;

impl Normalizer for NoopNormalizer {
    fn provider_name(&self) -> &'static str {
        "mock"
    }
    fn normalize_user(
        &self,
        _raw: &serde_json::Value,
    ) -> std::result::Result<User, NormalizeError> {
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

// ---------------------------------------------------------------------------
// Codex adversarial-review finding #3 regression tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_thread_persists_root_even_with_no_replies() {
    // Thread with only the root post (no replies). Pre-fix this stored ZERO
    // posts while reporting success, silently dropping the requested root.
    let root_id = PostId::new("root_solo");
    let root_post = MockProvider::make_post("root_solo", "author");
    let provider = Arc::new(MockProvider::new(vec![]).with_thread(vec![root_post.clone()]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor
        .ingest_thread(&root_id)
        .await
        .expect("ingest_thread failed");

    let state = store.state.lock().unwrap();
    assert_eq!(state.upserted.len(), 1, "root post must be persisted");
    assert_eq!(state.upserted[0].id, root_id);
    assert_eq!(run.posts_fetched, 1);
    assert!(run.error.is_none());
}

#[tokio::test]
async fn ingest_thread_persists_root_and_descendants() {
    let root_id = PostId::new("root_with_kids");
    let root = MockProvider::make_post("root_with_kids", "author");
    let reply_a = MockProvider::make_post("reply_a", "other");
    let reply_b = MockProvider::make_post("reply_b", "other");
    let provider = Arc::new(MockProvider::new(vec![]).with_thread(vec![
        root.clone(),
        reply_a.clone(),
        reply_b.clone(),
    ]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor
        .ingest_thread(&root_id)
        .await
        .expect("ingest_thread failed");

    let state = store.state.lock().unwrap();
    assert_eq!(state.upserted.len(), 3, "root + 2 replies should be stored");
    assert_eq!(run.posts_fetched, 3);
    let ids: Vec<_> = state.upserted.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&"root_with_kids"));
    assert!(ids.contains(&"reply_a"));
    assert!(ids.contains(&"reply_b"));
}

#[tokio::test]
async fn ingest_thread_empty_result_still_records_run_end() {
    // fetch_thread returning empty (root not found) should still close out
    // the FetchRun with 0 posts, not panic or leave a dangling run.
    let provider = Arc::new(MockProvider::new(vec![]));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    let run = ingestor
        .ingest_thread(&PostId::new("missing"))
        .await
        .expect("ingest_thread failed with empty result");
    assert_eq!(run.posts_fetched, 0);

    let state = store.state.lock().unwrap();
    assert_eq!(state.run_started.len(), 1);
    assert_eq!(state.run_ended.len(), 1);
}

// ---------------------------------------------------------------------------
// ingest_engagement: BFS "replies to everything I authored"
// ---------------------------------------------------------------------------

fn post(id: &str, author: &str) -> Post {
    let mut p = MockProvider::make_post(id, author);
    p.author = UserId::new(author);
    p
}

#[tokio::test]
async fn engagement_collects_direct_replies_to_my_posts() {
    // Store seeded with one of MY posts.
    let me = User {
        id: UserId::new("me"),
        username: Some("me".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let my_post = post("my_post", "me");
    let reply_a = post("ra", "stranger1");
    let reply_b = post("rb", "stranger2");

    let provider = Arc::new(MockProvider::new(vec![]).with_me(me.clone()).with_reply_to(
        &PostId::new("my_post"),
        vec![reply_a.clone(), reply_b.clone()],
    ));
    let store = MockStore::new();
    // Pre-seed the store: engagement uses posts_by_author to find seeds.
    store
        .upsert_posts(std::slice::from_ref(&my_post), None)
        .unwrap();

    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    let run = ingestor
        .ingest_engagement(8)
        .await
        .expect("engagement failed");

    // 2 new replies stored (my_post was already there and is just the seed).
    assert_eq!(run.posts_fetched, 2);
    let state = store.state.lock().unwrap();
    let ids: Vec<_> = state.upserted.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&"ra"));
    assert!(ids.contains(&"rb"));
}

#[tokio::test]
async fn engagement_recurses_into_replies_to_replies() {
    // Shape:
    //   my_post
    //   └── ra (stranger)
    //       └── rb (stranger)
    //           └── rc (stranger)    <- only collected if BFS keeps going
    let me_id = UserId::new("me");
    let me = User {
        id: me_id.clone(),
        username: Some("me".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let my_post = post("my_post", "me");
    let ra = post("ra", "stranger");
    let rb = post("rb", "stranger");
    let rc = post("rc", "stranger");

    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(me)
            .with_reply_to(&PostId::new("my_post"), vec![ra.clone()])
            .with_reply_to(&PostId::new("ra"), vec![rb.clone()])
            .with_reply_to(&PostId::new("rb"), vec![rc.clone()]),
    );
    let store = MockStore::new();
    store.upsert_posts(&[my_post], None).unwrap();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    let run = ingestor
        .ingest_engagement(8)
        .await
        .expect("engagement failed");

    assert_eq!(run.posts_fetched, 3, "3 descendants across 3 BFS levels");
    let state = store.state.lock().unwrap();
    let ids: Vec<_> = state.upserted.iter().map(|p| p.id.as_str()).collect();
    for expected in ["ra", "rb", "rc"] {
        assert!(ids.contains(&expected), "missing {expected}");
    }
}

#[tokio::test]
async fn engagement_respects_depth_cap() {
    // Same chain as above but cap depth at 1: should stop after ra
    // (ra is at depth 0 relative to my_post seed; rb would be depth 1; rc
    // depth 2).
    let me_id = UserId::new("me");
    let me = User {
        id: me_id.clone(),
        username: Some("me".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let my_post = post("my_post", "me");

    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(me)
            .with_reply_to(&PostId::new("my_post"), vec![post("ra", "stranger")])
            .with_reply_to(&PostId::new("ra"), vec![post("rb", "stranger")])
            .with_reply_to(&PostId::new("rb"), vec![post("rc", "stranger")]),
    );
    let store = MockStore::new();
    store.upsert_posts(&[my_post], None).unwrap();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    // depth=1: only descend from seed, not from replies.
    let run = ingestor
        .ingest_engagement(1)
        .await
        .expect("engagement failed");

    // Only `ra` collected (direct reply). `rb` and `rc` are past the cap.
    assert_eq!(run.posts_fetched, 1);
    let state = store.state.lock().unwrap();
    let ids: Vec<_> = state.upserted.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&"ra"));
    assert!(!ids.contains(&"rb"));
    assert!(!ids.contains(&"rc"));
}

#[tokio::test]
async fn engagement_deduplicates_across_seeds_and_levels() {
    // Two seeds that both eventually hit the same reply id — it should
    // only be fetched/stored once.
    let me_id = UserId::new("me");
    let me = User {
        id: me_id.clone(),
        username: Some("me".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let seed_a = post("seed_a", "me");
    let seed_b = post("seed_b", "me");
    let shared = post("shared", "stranger");

    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(me)
            .with_reply_to(&PostId::new("seed_a"), vec![shared.clone()])
            .with_reply_to(&PostId::new("seed_b"), vec![shared.clone()]),
    );
    let store = MockStore::new();
    store.upsert_posts(&[seed_a, seed_b], None).unwrap();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    let run = ingestor
        .ingest_engagement(8)
        .await
        .expect("engagement failed");

    // `shared` only counts once.
    assert_eq!(run.posts_fetched, 1);
    let state = store.state.lock().unwrap();
    let shared_count = state
        .upserted
        .iter()
        .filter(|p| p.id == PostId::new("shared"))
        .count();
    assert_eq!(shared_count, 1);
}

#[tokio::test]
async fn ingest_engagement_upserts_me_profile() {
    let me = User {
        id: UserId::new("real_id_42"),
        username: Some("alice".into()),
        name: Some("Alice".into()),
        biography: None,
        profile_picture_url: None,
    };
    let my_post = Post {
        id: PostId::new("my_post_e"),
        author: UserId::new("real_id_42"),
        author_username: Some("alice".into()),
        text: Some("seed".into()),
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
    let provider = Arc::new(MockProvider::new(vec![]).with_me(me.clone()));
    let store = MockStore::new();
    store
        .upsert_posts(std::slice::from_ref(&my_post), None)
        .unwrap();

    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    ingestor
        .ingest_engagement(1)
        .await
        .expect("ingest_engagement failed");

    let state = store.state.lock().unwrap();
    assert!(
        state
            .upserted_users
            .iter()
            .any(|u| u.id == UserId::new("real_id_42")),
        "me profile must be upserted during ingest_engagement"
    );
}

#[tokio::test]
async fn ingest_me_upserts_me_profile() {
    let me = User {
        id: UserId::new("real_id_99"),
        username: Some("bob".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let page = vec![MockProvider::make_post("p_for_me", "real_id_99")];
    let provider = Arc::new(MockProvider::new(vec![page]).with_me(me.clone()));
    let store = MockStore::new();

    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    ingestor.ingest_me().await.expect("ingest_me failed");

    let state = store.state.lock().unwrap();
    assert!(
        state
            .upserted_users
            .iter()
            .any(|u| u.id == UserId::new("real_id_99")),
        "me profile must be upserted during ingest_me"
    );
}

#[tokio::test]
async fn resolve_author_called_with_correct_args() {
    let me = User {
        id: UserId::new("real_77"),
        username: Some("carol".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    };
    let provider = Arc::new(MockProvider::new(vec![vec![]]).with_me(me.clone()));
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    ingestor.ingest_me().await.expect("ingest_me failed");
    let state = store.state.lock().unwrap();
    assert!(
        state
            .resolve_author_calls
            .iter()
            .any(|(u, id)| u == "carol" && *id == UserId::new("real_77")),
        "resolve_author must be called with (\"carol\", UserId(\"real_77\"))"
    );
}

fn audience_with_all_breakdowns(followers_count: u64) -> MockAudience {
    MockAudience {
        followers_count,
        demographics: vec![
            DemographicBucket {
                dimension: DemographicDimension::Country,
                bucket: "US".into(),
                value: 80,
            },
            DemographicBucket {
                dimension: DemographicDimension::City,
                bucket: "New York".into(),
                value: 30,
            },
            DemographicBucket {
                dimension: DemographicDimension::Age,
                bucket: "25-34".into(),
                value: 45,
            },
            DemographicBucket {
                dimension: DemographicDimension::Gender,
                bucket: "female".into(),
                value: 52,
            },
        ],
        fail_dimension: None,
    }
}

fn audience_account() -> User {
    User {
        id: UserId::new("audience-account"),
        username: Some("audience_owner".into()),
        name: None,
        biography: None,
        profile_picture_url: None,
    }
}

#[tokio::test]
async fn refresh_audience_writes_count_only_below_demographic_threshold() {
    // Given: an account below the official demographic threshold.
    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(audience_account())
            .with_audience(audience_with_all_breakdowns(99)),
    );
    let store = MockStore::new();
    let ingestor = Ingestor::new(
        Arc::clone(&provider),
        Box::new(NoopNormalizer),
        Arc::clone(&store),
    );

    // When: the audience is refreshed.
    let summary = ingestor
        .refresh_audience()
        .await
        .expect("count-only audience refresh should succeed");

    // Then: only the count is persisted and no demographic call is issued.
    assert_eq!(summary.account_id, UserId::new("audience-account"));
    assert_eq!(summary.followers_count, 99);
    assert_eq!(summary.demographics_count, 0);
    assert_eq!(
        *provider.audience_queries.lock().unwrap(),
        vec![AudienceInsightQuery::FollowersCount]
    );
    let state = store.state.lock().unwrap();
    assert_eq!(state.audience_snapshots.len(), 1);
    assert!(state.audience_snapshots[0].demographics.is_empty());
}

#[tokio::test]
async fn refresh_audience_persists_all_breakdowns_in_one_snapshot() {
    // Given: an eligible account and one bucket for every required breakdown.
    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(audience_account())
            .with_audience(audience_with_all_breakdowns(100)),
    );
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    // When: the audience is refreshed.
    let summary = ingestor
        .refresh_audience()
        .await
        .expect("eligible audience refresh should succeed");

    // Then: the summary and atomic snapshot contain every breakdown.
    assert_eq!(summary.followers_count, 100);
    assert_eq!(summary.demographics_count, 4);
    let state = store.state.lock().unwrap();
    assert_eq!(state.audience_snapshots.len(), 1);
    let snapshot = &state.audience_snapshots[0];
    assert_eq!(snapshot.account_id, UserId::new("audience-account"));
    assert_eq!(snapshot.followers_count, 100);
    assert_eq!(snapshot.demographics.len(), 4);
    assert!(snapshot.observed_at <= Utc::now());
}

#[tokio::test]
async fn refresh_audience_leaves_no_snapshot_when_a_breakdown_fails() {
    // Given: the third required breakdown fails before snapshot persistence.
    let mut audience = audience_with_all_breakdowns(100);
    audience.fail_dimension = Some(DemographicDimension::Age);
    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(audience_account())
            .with_audience(audience),
    );
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    // When: the audience refresh reaches the failing breakdown.
    let result = ingestor.refresh_audience().await;

    // Then: no partial snapshot is written.
    assert!(matches!(result, Err(Error::Parse(_))));
    assert!(store.state.lock().unwrap().audience_snapshots.is_empty());
}

#[tokio::test]
async fn refresh_audience_deduplicates_mentions_and_stops_on_repeated_cursor() {
    // Given: a mention page whose next cursor repeats and contains a duplicate post.
    let mut mention = MockProvider::make_post("mentioned-post", "another-user");
    mention.mentions.push(Mention {
        username: "audience_owner".into(),
        user_id: Some(UserId::new("audience-account")),
    });
    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(audience_account())
            .with_audience(audience_with_all_breakdowns(99))
            .with_mentions(Page::new(vec![mention], Some(Cursor("repeat".into())))),
    );
    let store = MockStore::new();
    let ingestor = Ingestor::new(
        Arc::clone(&provider),
        Box::new(NoopNormalizer),
        Arc::clone(&store),
    );

    // When: mention ingestion encounters the repeated cursor.
    let summary = ingestor
        .refresh_audience()
        .await
        .expect("repeated mention cursor must not fail the refresh");

    // Then: the post is persisted once with the authenticated mention target.
    assert_eq!(summary.mentions_ingested, 1);
    assert_eq!(provider.mention_requests.lock().unwrap().len(), 2);
    let state = store.state.lock().unwrap();
    assert_eq!(state.upserted.len(), 1);
    assert_eq!(state.upserted[0].mentions.len(), 1);
    assert_eq!(state.upserted[0].mentions[0].username, "audience_owner");
    assert_eq!(
        state.upserted[0].mentions[0].user_id,
        Some(UserId::new("audience-account"))
    );
}

#[tokio::test]
async fn refresh_audience_retains_snapshot_when_mentions_are_permission_denied() {
    // Given: successful insights and a documented-scope failure for mentions.
    let provider = Arc::new(
        MockProvider::new(vec![])
            .with_me(audience_account())
            .with_audience(audience_with_all_breakdowns(99))
            .with_mention_error(MockProviderError::PermissionDenied),
    );
    let store = MockStore::new();
    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));

    // When: the mentions phase fails after the snapshot phase succeeds.
    let summary = ingestor
        .refresh_audience()
        .await
        .expect("mention permission denial must become a warning");

    // Then: the snapshot remains durable and the warning is typed in the summary.
    assert!(summary.mention_warning.is_some());
    assert_eq!(store.state.lock().unwrap().audience_snapshots.len(), 1);
}
