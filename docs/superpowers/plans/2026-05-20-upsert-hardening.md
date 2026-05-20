# Upsert Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make re-ingesting a post through a sparse reply edge stop corrupting richer stored data, and resolve synthesized `@username` authors to real ids.

**Architecture:** A pure `Post::merge` in threads-core drives a read-merge-write upsert in threads-store; a `@username` sentinel makes synthesized authors detectable so a resolution pass can rewrite them to numeric ids once known.

**Tech Stack:** Rust 2024 edition (rustc 1.85), rusqlite (bundled SQLite, FTS5), tokio, chrono. Tests via `cargo test -p <crate>`.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/threads-core/src/model.rs` | Modify | Add `Post::merge` and private `prefer_real` helper |
| `crates/threads-store/src/query.rs` | Modify | Integrate merge into `upsert_post_tx`; add `resolve_author` query fn |
| `crates/threads-store/src/store.rs` | Modify | Expose `Store::resolve_author` wrapper |
| `crates/threads-store/src/lib.rs` | Modify | Add `resolve_author` to `pub use query::{…}` list |
| `crates/threads-store/src/tests.rs` | Modify | Add `reupsert_via_sparse_reply_preserves_rich_root`, `resolve_author_rewrites_handle_to_real_id`, `posts_by_author_finds_my_replies_after_resolution` |
| `crates/threads-provider-official/src/provider.rs` | Modify | Change username fallback to `@username` sentinel; update existing test |
| `crates/threads-ingest/src/store_shim.rs` | Modify | Add `upsert_user` and `resolve_author` to `StoreWrite` trait + `impl … for Store` |
| `crates/threads-ingest/src/orchestrator.rs` | Modify | Call `upsert_user(&me)` in `ingest_engagement`; add `fetch_me` + `upsert_user` at start of `run_ingest_me` |
| `crates/threads-ingest/tests/ingest_tests.rs` | Modify | Add `MockStore::upserted_users` capture; add `ingest_engagement_upserts_me_profile` and `ingest_me_upserts_me_profile` tests |

---

### Task 1: `Post::merge` — pure merge function in threads-core

**Files:**
- Modify: `crates/threads-core/src/model.rs` (append to `#[cfg(test)] mod tests` block starting at line 158; add impl block before it)
- Test path: `crates/threads-core/src/model.rs` (inline `#[cfg(test)]`)

- [ ] **Write the failing tests.** Add the following block inside `mod tests` in `crates/threads-core/src/model.rs`, appended after the existing `page_empty` test:

```rust
    // ------------------------------------------------------------------ //
    //  Post::merge                                                        //
    // ------------------------------------------------------------------ //

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn rich_post() -> Post {
        Post {
            id: PostId::new("p1"),
            author: UserId::new("123456"),
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
        let incoming = sparse_post();
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.created_at, existing.created_at);
    }

    #[test]
    fn merge_incoming_created_at_wins_when_both_present() {
        let existing = rich_post();
        let mut incoming = sparse_post();
        incoming.created_at = Some(ts("2026-06-01T12:00:00+00:00"));
        let merged = Post::merge(existing, incoming.clone());
        assert_eq!(merged.created_at, incoming.created_at);
    }

    #[test]
    fn merge_keeps_known_text_when_incoming_none() {
        let existing = rich_post();
        let incoming = sparse_post(); // text = None
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.text, existing.text);
    }

    #[test]
    fn merge_keeps_known_permalink_when_incoming_none() {
        let existing = rich_post();
        let incoming = sparse_post();
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.permalink, existing.permalink);
    }

    #[test]
    fn merge_keeps_known_parent_id_when_incoming_none() {
        let existing = rich_post();
        let incoming = sparse_post();
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.parent_id, existing.parent_id);
    }

    #[test]
    fn merge_keeps_known_root_id_when_incoming_none() {
        let existing = rich_post();
        let incoming = sparse_post();
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.root_id, existing.root_id);
    }

    #[test]
    fn merge_prefers_real_author_over_handle() {
        // existing has real numeric id; incoming has @handle — existing wins.
        let existing = rich_post(); // author = "123456"
        let incoming = sparse_post(); // author = "@alice"
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.author, existing.author);
    }

    #[test]
    fn merge_incoming_real_author_beats_existing_handle() {
        // existing has @handle; incoming has real id — incoming wins.
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
        // existing = true, incoming = false → merged stays true.
        let existing = rich_post(); // is_quote_post = true
        let incoming = sparse_post(); // is_quote_post = false
        let merged = Post::merge(existing, incoming);
        assert!(merged.is_quote_post);
    }

    #[test]
    fn merge_is_quote_post_false_plus_true_becomes_true() {
        let mut existing = rich_post();
        existing.is_quote_post = false;
        let mut incoming = sparse_post();
        incoming.is_quote_post = true;
        let merged = Post::merge(existing, incoming);
        assert!(merged.is_quote_post);
    }

    #[test]
    fn merge_keeps_media_when_incoming_empty() {
        let existing = rich_post(); // has 1 media item
        let incoming = sparse_post(); // media = []
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.media, existing.media);
    }

    #[test]
    fn merge_incoming_media_wins_when_nonempty() {
        let existing = rich_post();
        let mut incoming = sparse_post();
        incoming.media = vec![Media {
            kind: MediaKind::Video,
            url: Some("https://example.com/v.mp4".into()),
            thumbnail_url: None,
        }];
        let merged = Post::merge(existing, incoming.clone());
        assert_eq!(merged.media, incoming.media);
    }

    #[test]
    fn merge_keeps_urls_when_incoming_empty() {
        let existing = rich_post(); // has 1 url
        let incoming = sparse_post(); // urls = []
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.urls, existing.urls);
    }

    #[test]
    fn merge_keeps_mentions_when_incoming_empty() {
        let existing = rich_post(); // has 1 mention
        let incoming = sparse_post(); // mentions = []
        let merged = Post::merge(existing.clone(), incoming);
        assert_eq!(merged.mentions, existing.mentions);
    }

    #[test]
    fn merge_id_and_raw_always_from_incoming() {
        let existing = rich_post();
        let incoming = sparse_post();
        let merged = Post::merge(existing, incoming.clone());
        assert_eq!(merged.id, incoming.id);
        assert_eq!(merged.raw, incoming.raw);
    }
```

- [ ] **Run the tests (expect FAIL — `Post::merge` does not exist yet):**
  ```
  cargo test -p threads-core merge
  ```
  Expected: FAIL — `error[E0599]: no method named 'merge' found for struct 'Post'`

- [ ] **Add the implementation.** Insert the following `impl Post` block in `crates/threads-core/src/model.rs`, immediately before the `#[cfg(test)]` line (line 157):

```rust
impl Post {
    /// Choose between an incoming author id and an existing author id.
    ///
    /// Prefer the non-`@`-prefixed (real numeric) id. If both are real, or
    /// both are handles, return `incoming`.
    fn prefer_real(incoming: UserId, existing: UserId) -> UserId {
        let incoming_is_handle = incoming.as_str().starts_with('@');
        let existing_is_handle = existing.as_str().starts_with('@');
        match (incoming_is_handle, existing_is_handle) {
            // incoming is real, existing is handle → incoming wins
            (false, true) => incoming,
            // incoming is handle, existing is real → existing wins
            (true, false) => existing,
            // both real or both handles → incoming wins (latest fetch preferred)
            _ => incoming,
        }
    }

    /// Merge a re-fetched `incoming` post onto an `existing` stored post.
    ///
    /// Rules:
    /// - `text`, `created_at`, `permalink`, `parent_id`, `root_id`:
    ///   `incoming.field.or(existing.field)` — never null out a known value.
    /// - `author`: prefer real (non-`@`-prefixed) id; ties go to incoming.
    /// - `is_quote_post`: sticky true — `existing || incoming`.
    /// - `media`, `urls`, `mentions`: incoming unless empty, then existing.
    /// - `id`, `raw`: always incoming.
    pub fn merge(existing: Post, incoming: Post) -> Post {
        Post {
            id: incoming.id,
            author: Self::prefer_real(incoming.author, existing.author),
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
```

- [ ] **Run the tests (expect PASS):**
  ```
  cargo test -p threads-core merge
  ```
  Expected: PASS — all 15 `merge_*` tests green.

- [ ] **Commit:**
  ```
  git add crates/threads-core/src/model.rs
  git commit -m "feat(core): add Post::merge with prefer_real author selection"
  ```

---

### Task 2: Integrate `Post::merge` into `upsert_post_tx` (store-level read-merge-write)

**Files:**
- Modify: `crates/threads-store/src/query.rs` (function `upsert_post_tx`, lines 79–231)
- Modify: `crates/threads-store/src/tests.rs` (append new test)
- Test path: `crates/threads-store/src/tests.rs`

- [ ] **Write the failing test.** Append the following test inside `mod tests` in `crates/threads-store/src/tests.rs`, after the `reupsert_replaces_mention_edges` test (line 791, before the closing `}`):

```rust
    // ------------------------------------------------------------------ //
    //  Merge: re-upsert via sparse reply preserves rich root              //
    // ------------------------------------------------------------------ //

    #[test]
    fn reupsert_via_sparse_reply_preserves_rich_root() {
        let store = Store::open_in_memory().unwrap();

        // 1. Insert a rich root post: media, permalink, real author, quote.
        let rich = Post {
            id: PostId::new("root_merge"),
            author: UserId::new("123456"),
            text: Some("rich post text".into()),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: Some("https://threads.net/p/rich".into()),
            media: vec![Media {
                kind: MediaKind::Image,
                url: Some("https://example.com/img.jpg".into()),
                thumbnail_url: None,
            }],
            urls: vec![UrlEntity {
                url: "https://threads.net".into(),
                display_text: Some("threads".into()),
            }],
            mentions: vec![Mention {
                username: "bob".into(),
                user_id: None,
            }],
            is_quote_post: true,
            raw: None,
        };
        store.upsert_post(&rich, None).unwrap();

        // 2. Re-upsert the same post id with a sparse version:
        //    - no media, no urls, no mentions
        //    - @handle author (synthesized, should lose)
        //    - is_quote_post=false (should stay true)
        //    - no permalink, no text, no created_at, no parent/root
        let sparse = Post {
            id: PostId::new("root_merge"),
            author: UserId::new("@alice"),
            text: None,
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: Some(serde_json::json!({ "sparse": true })),
        };
        store.upsert_post(&sparse, None).unwrap();

        // 3. Assert the rich fields survived.
        let fetched = store
            .get_post(&PostId::new("root_merge"))
            .unwrap()
            .expect("post must exist");

        assert_eq!(fetched.author, UserId::new("123456"), "real author must survive");
        assert_eq!(
            fetched.text.as_deref(),
            Some("rich post text"),
            "text must survive"
        );
        assert!(fetched.created_at.is_some(), "created_at must survive");
        assert_eq!(
            fetched.permalink.as_deref(),
            Some("https://threads.net/p/rich"),
            "permalink must survive"
        );
        assert_eq!(fetched.media.len(), 1, "media must survive");
        assert_eq!(fetched.urls.len(), 1, "urls must survive");
        assert_eq!(fetched.mentions.len(), 1, "mentions must survive");
        assert!(fetched.is_quote_post, "is_quote_post must stay true");
    }
```

- [ ] **Run the test (expect FAIL — current upsert overwrites, not merges):**
  ```
  cargo test -p threads-store reupsert_via_sparse_reply_preserves_rich_root
  ```
  Expected: FAIL — author is `@alice`, text/media/urls/mentions/is_quote_post are sparse values.

- [ ] **Add the implementation.** In `crates/threads-store/src/query.rs`, add the `threads_core::model::Post` merge call at the top of `upsert_post_tx`. The function signature and first line stay as-is; add a load-then-merge step before the first `tx.execute`. Replace the opening of `upsert_post_tx` (lines 79–91 — the `let now = …` through the author-stub INSERT) with:

```rust
fn upsert_post_tx(tx: &Transaction, post: &Post, fetch_run_id: Option<&str>) -> Result<()> {
    use threads_core::model::Post as CorePost;
    let now = Utc::now().to_rfc3339();

    // --- Read-merge-write: never lose data to a sparser re-fetch ---
    // `Transaction` derefs to `Connection`, so `load_post` accepts `&*tx`.
    let post_owned: Post;
    let post = if let Some(existing) = load_post(&**tx, post.id.as_str())? {
        post_owned = CorePost::merge(existing, post.clone());
        &post_owned
    } else {
        post
    };

    // Ensure the author stub exists so the FK is satisfied.
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, NULL, NULL, NULL, NULL, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![post.author.as_str(), &now],
    )
    .map_err(StoreError::Sqlite)?;
```

  All code after that author-stub INSERT (the post-row upsert, child-table deletes, media/urls/mentions inserts, edges, raw payload) is unchanged — it references `post` which now points to the merged value.

  Note: the import `use threads_core::model::Post as CorePost;` is needed only to call the associated function. The existing `use threads_core::model::{…, Post, …}` at the top of the file already brings `Post` into scope; the alias avoids a shadowing conflict with the local `post` variable. Alternatively, call `Post::merge(…)` directly since `Post` is already in scope — the alias is optional but makes the intent explicit.

- [ ] **Run the test (expect PASS):**
  ```
  cargo test -p threads-store reupsert_via_sparse_reply_preserves_rich_root
  ```
  Expected: PASS.

- [ ] **Run the full store suite to confirm no regressions:**
  ```
  cargo test -p threads-store
  ```
  Expected: all existing tests still PASS.

- [ ] **Commit:**
  ```
  git add crates/threads-store/src/query.rs crates/threads-store/src/tests.rs
  git commit -m "feat(store): read-merge-write in upsert_post_tx using Post::merge"
  ```

---

### Task 3: Adopt the `@username` sentinel in `dto_to_post`

**Files:**
- Modify: `crates/threads-provider-official/src/provider.rs` — `dto_to_post` function (lines 218–248) and test `dto_to_post_synthesizes_author_from_username` (line 314)
- Test path: `crates/threads-provider-official/src/provider.rs` (inline `#[cfg(test)]`)

- [ ] **Update the test first (it currently asserts `UserId::new("alice")` — change it to `UserId::new("@alice")`).** In `crates/threads-provider-official/src/provider.rs`, find and update `dto_to_post_synthesizes_author_from_username` (line 334):

```rust
    #[test]
    fn dto_to_post_synthesizes_author_from_username() {
        let dto = PostDto {
            id: "p1".into(),
            username: Some("alice".into()),
            text: Some("hi".into()),
            timestamp: None,
            permalink: None,
            media_type: None,
            media_url: None,
            thumbnail_url: None,
            is_quote_post: false,
            owner: None,
            children: None,
            replied_to: None,
            root_post: None,
            is_reply: None,
            shortcode: None,
        };
        let post = dto_to_post(dto, None);
        assert_eq!(post.id, PostId::new("p1"));
        assert_eq!(post.author, UserId::new("@alice"));
    }
```

- [ ] **Run the test (expect FAIL — production code still uses bare username):**
  ```
  cargo test -p threads-provider-official dto_to_post_synthesizes_author_from_username
  ```
  Expected: FAIL — `assertion failed: post.author == UserId::new("@alice")` (actual is `UserId::new("alice")`).

- [ ] **Implement the sentinel.** In `crates/threads-provider-official/src/provider.rs`, change the author synthesis in `dto_to_post` (the `.or_else` branch, currently line 225):

  Current:
  ```rust
      let author = dto
          .owner
          .as_ref()
          .map(|o| UserId::new(&o.id))
          .or_else(|| dto.username.as_deref().map(UserId::new))
          .unwrap_or_else(|| UserId::new(""));
  ```

  Replace with:
  ```rust
      let author = dto
          .owner
          .as_ref()
          .map(|o| UserId::new(&o.id))
          .or_else(|| {
              dto.username
                  .as_deref()
                  .map(|u| UserId::new(format!("@{u}")))
          })
          .unwrap_or_else(|| UserId::new(""));
  ```

- [ ] **Run the test (expect PASS):**
  ```
  cargo test -p threads-provider-official dto_to_post_synthesizes_author_from_username
  ```
  Expected: PASS.

- [ ] **Run the full provider suite to confirm no regressions:**
  ```
  cargo test -p threads-provider-official
  ```
  Expected: all tests PASS.

- [ ] **Commit:**
  ```
  git add crates/threads-provider-official/src/provider.rs
  git commit -m "fix(provider): synthesize @username sentinel so handles are detectable"
  ```

---

### Task 4: Persist the `me` profile during ingest

**Files:**
- Modify: `crates/threads-ingest/src/store_shim.rs` — add `upsert_user` to `StoreWrite` trait and its `impl … for Store`
- Modify: `crates/threads-ingest/src/orchestrator.rs` — call `self.store.upsert_user(&me)` in `run_ingest_engagement` and `run_ingest_me`
- Modify: `crates/threads-ingest/tests/ingest_tests.rs` — extend `MockStore` + add two tests
- Test path: `crates/threads-ingest/tests/ingest_tests.rs`

- [ ] **Write the failing tests.** In `crates/threads-ingest/tests/ingest_tests.rs`:

  First, extend `MockStoreState` to capture upserted users, and update `MockStore`:

```rust
#[derive(Default)]
struct MockStoreState {
    upserted: Vec<Post>,
    upserted_users: Vec<User>,
    run_started: Vec<FetchRun>,
    run_ended: Vec<(String, u64, Option<String>)>,
}
```

  Add the `upsert_user` method to the `impl StoreWrite for MockStore` block:

```rust
    fn upsert_user(&self, user: &User) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.upserted_users.push(user.clone());
        Ok(())
    }
```

  Then add the two new tests after the existing engagement tests:

```rust
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
    store.upsert_posts(std::slice::from_ref(&my_post), None).unwrap();

    let ingestor = Ingestor::new(provider, Box::new(NoopNormalizer), Arc::clone(&store));
    ingestor.ingest_engagement(1).await.expect("ingest_engagement failed");

    let state = store.state.lock().unwrap();
    let found = state
        .upserted_users
        .iter()
        .any(|u| u.id == UserId::new("real_id_42"));
    assert!(found, "me profile must be upserted during ingest_engagement");
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
    let found = state
        .upserted_users
        .iter()
        .any(|u| u.id == UserId::new("real_id_99"));
    assert!(found, "me profile must be upserted during ingest_me");
}
```

- [ ] **Run the tests (expect FAIL — `StoreWrite` has no `upsert_user`, code won't compile):**
  ```
  cargo test -p threads-ingest ingest_engagement_upserts_me_profile
  ```
  Expected: FAIL — compile error: `no method named 'upsert_user' found for trait StoreWrite`.

- [ ] **Implement `upsert_user` in `StoreWrite`.** In `crates/threads-ingest/src/store_shim.rs`:

  Add to the trait definition (after `posts_by_author`):
  ```rust
      /// Upsert the authenticated user's profile row.
      fn upsert_user(&self, user: &User) -> Result<()>;
  ```

  Add to `impl StoreWrite for threads_store::Store` (after the `posts_by_author` impl):
  ```rust
      fn upsert_user(&self, user: &User) -> Result<()> {
          Self::upsert_user(self, user).map_err(Into::into)
      }
  ```

  The `User` type is already imported at the top of the file (`use threads_core::{FetchRun, Post, PostId, Result, UserId};`) — add `User` to that import:
  ```rust
  use threads_core::{FetchRun, Post, PostId, Result, User, UserId};
  ```

- [ ] **Call `upsert_user` in the orchestrator.** In `crates/threads-ingest/src/orchestrator.rs`:

  In `run_ingest_engagement` (line 242), `me` is already fetched at line 244. Insert `self.store.upsert_user(&me)?;` immediately after:

  Current (lines 244–251):
  ```rust
          let me = self.provider.fetch_me().await?;
          let seeds = self.store.posts_by_author(&me.id)?;
  ```

  Replace with:
  ```rust
          let me = self.provider.fetch_me().await?;
          self.store.upsert_user(&me)?;
          let seeds = self.store.posts_by_author(&me.id)?;
  ```

  In `run_ingest_me` (line 180), `fetch_me` is not called today. Add a `fetch_me` + `upsert_user` at the top of the function, before the `let mut seen` initialization:

  Current opening of `run_ingest_me` (lines 180–183):
  ```rust
      async fn run_ingest_me(&self, run_id: &str) -> Result<u64> {
          let mut seen: HashSet<PostId> = HashSet::new();
          let mut batch = Vec::new();
          let mut total: u64 = 0;
  ```

  Replace with:
  ```rust
      async fn run_ingest_me(&self, run_id: &str) -> Result<u64> {
          // Persist the authenticated user's profile so author resolution
          // can later rewrite @username placeholders for posts they authored.
          let me = self.provider.fetch_me().await?;
          self.store.upsert_user(&me)?;

          let mut seen: HashSet<PostId> = HashSet::new();
          let mut batch = Vec::new();
          let mut total: u64 = 0;
  ```

- [ ] **Run the new tests (expect PASS):**
  ```
  cargo test -p threads-ingest ingest_engagement_upserts_me_profile
  cargo test -p threads-ingest ingest_me_upserts_me_profile
  ```
  Expected: both PASS.

- [ ] **Run the full ingest suite to confirm no regressions:**
  ```
  cargo test -p threads-ingest
  ```
  Expected: all tests PASS.

- [ ] **Commit:**
  ```
  git add crates/threads-ingest/src/store_shim.rs crates/threads-ingest/src/orchestrator.rs crates/threads-ingest/tests/ingest_tests.rs
  git commit -m "feat(ingest): persist me profile via upsert_user in ingest_me and ingest_engagement"
  ```

---

### Task 5: Author resolution — `resolve_author` in store + orchestrator wiring

**Files:**
- Modify: `crates/threads-store/src/query.rs` — add `resolve_author` query function (new public function)
- Modify: `crates/threads-store/src/store.rs` — add `Store::resolve_author` wrapper
- Modify: `crates/threads-store/src/lib.rs` — add `resolve_author` to `pub use query::{…}`
- Modify: `crates/threads-store/src/tests.rs` — add two resolution tests
- Modify: `crates/threads-ingest/src/store_shim.rs` — add `resolve_author` to trait + impl
- Modify: `crates/threads-ingest/src/orchestrator.rs` — call `resolve_author` after `upsert_user` in `run_ingest_me` and `run_ingest_engagement`
- Modify: `crates/threads-ingest/tests/ingest_tests.rs` — extend `MockStore`, add `resolve_author_called_with_correct_args` test
- Test path: `crates/threads-store/src/tests.rs` and `crates/threads-ingest/tests/ingest_tests.rs`

- [ ] **Write the failing store tests.** Append to `mod tests` in `crates/threads-store/src/tests.rs`:

```rust
    // ------------------------------------------------------------------ //
    //  Author resolution                                                  //
    // ------------------------------------------------------------------ //

    #[test]
    fn resolve_author_rewrites_handle_to_real_id() {
        let store = Store::open_in_memory().unwrap();

        // Insert a post stored under the @alice placeholder.
        let post = Post {
            id: PostId::new("post_under_handle"),
            author: UserId::new("@alice"),
            text: Some("written by alice".into()),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };
        store.upsert_post(&post, None).unwrap();

        // Resolve @alice → real id "99999".
        store
            .resolve_author("alice", &UserId::new("99999"))
            .unwrap();

        // The post must now be attributed to "99999".
        let fetched = store
            .get_post(&PostId::new("post_under_handle"))
            .unwrap()
            .expect("post must exist");
        assert_eq!(
            fetched.author,
            UserId::new("99999"),
            "author must be rewritten to real id"
        );

        // The placeholder user @alice must be gone.
        let conn = store.raw_conn();
        let placeholder_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE id = '@alice'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(placeholder_count, 0, "@alice placeholder user must be deleted");

        // The real user "99999" must exist.
        let real_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE id = '99999'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(real_count, 1, "real user row must exist after resolution");
    }

    #[test]
    fn posts_by_author_finds_my_replies_after_resolution() {
        let store = Store::open_in_memory().unwrap();

        // A reply stored under @alice before resolution.
        let reply = Post {
            id: PostId::new("reply_under_handle"),
            author: UserId::new("@alice"),
            text: Some("my reply".into()),
            created_at: Some(Utc::now()),
            parent_id: Some(PostId::new("some_root")),
            root_id: Some(PostId::new("some_root")),
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };
        store.upsert_post(&reply, None).unwrap();

        // Before resolution, posts_by_author with the real id returns nothing.
        let before = store
            .posts_by_author(&UserId::new("99999"))
            .unwrap();
        assert!(before.is_empty(), "no posts under real id before resolution");

        // Resolve @alice → "99999".
        store
            .resolve_author("alice", &UserId::new("99999"))
            .unwrap();

        // After resolution, posts_by_author with the real id finds the reply.
        let after = store
            .posts_by_author(&UserId::new("99999"))
            .unwrap();
        assert_eq!(after.len(), 1, "reply must be found under real id after resolution");
        assert_eq!(after[0], PostId::new("reply_under_handle"));
    }
```

- [ ] **Run the tests (expect FAIL — `resolve_author` does not exist):**
  ```
  cargo test -p threads-store resolve_author
  ```
  Expected: FAIL — compile error: `no method named 'resolve_author' found for struct 'Store'`.

- [ ] **Implement `resolve_author` in `query.rs`.** Append the following public function to `crates/threads-store/src/query.rs`, before the `#[cfg(test)]` section at the end:

```rust
// ------------------------------------------------------------------ //
//  Author resolution                                                  //
// ------------------------------------------------------------------ //

/// Resolve a synthesized `@username` placeholder to a real numeric user id.
///
/// In one transaction:
/// 1. Upsert the real user row (id = `real_id`).
/// 2. `UPDATE posts SET author_id = real_id WHERE author_id = '@' || username`.
/// 3. Delete the placeholder user row `'@' || username` (if present).
///
/// `edges` are intentionally NOT rewritten: `edges.from_id` holds POST ids and
/// `edges.to_id` holds post-or-mentioned-user ids — an author handle is never an
/// edge endpoint, so an author rewrite has nothing to reconcile there. Step 2
/// runs before step 3 so the `posts.author_id` FK (`ON DELETE CASCADE`) finds no
/// rows still pointing at the placeholder when it is deleted.
///
/// Idempotent: safe to call multiple times for the same pair.
pub fn resolve_author(
    conn: &mut Connection,
    username: &str,
    real_id: &UserId,
) -> Result<()> {
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

    // 2. Rewrite posts authored under the placeholder (must precede the
    //    placeholder DELETE so the FK cascade finds no rows to remove).
    tx.execute(
        "UPDATE posts SET author_id = ?1 WHERE author_id = ?2",
        params![real_id.as_str(), &placeholder],
    )
    .map_err(StoreError::Sqlite)?;

    // 3. Remove the now-orphaned placeholder user row.
    tx.execute(
        "DELETE FROM users WHERE id = ?1",
        params![&placeholder],
    )
    .map_err(StoreError::Sqlite)?;

    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}
```

- [ ] **Expose through `Store` wrapper.** In `crates/threads-store/src/store.rs`, add after the `record_fetch_run_end` method:

```rust
    /// Resolve all posts and edges stored under `'@' || username` to `real_id`.
    /// Upserts the real user row, rewrites `posts.author_id` and `edges.from_id`,
    /// then deletes the `@username` placeholder user.
    pub fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        query::resolve_author(&mut conn, username, real_id)
    }
```

- [ ] **Re-export from `lib.rs`.** In `crates/threads-store/src/lib.rs`, add `resolve_author` to the `pub use query::{…}` list:

```rust
pub use query::{
    PostKind, delete_post, deletions_in_last_24h, get_post, list_posts,
    oldest_deletion_in_last_24h, posts_by_author, posts_in_window, record_deletion,
    record_fetch_run_end, record_fetch_run_start, resolve_author, search_text,
    thread_rooted_at, upsert_post, upsert_posts, upsert_user,
};
```

- [ ] **Run the store resolution tests (expect PASS):**
  ```
  cargo test -p threads-store resolve_author
  cargo test -p threads-store posts_by_author_finds_my_replies_after_resolution
  ```
  Expected: both PASS.

- [ ] **Run the full store suite to confirm no regressions:**
  ```
  cargo test -p threads-store
  ```
  Expected: all tests PASS.

- [ ] **Add `resolve_author` to `StoreWrite` in `store_shim.rs`.** In `crates/threads-ingest/src/store_shim.rs`, add to the trait definition (after `upsert_user`):

```rust
    /// Rewrite posts and edges stored under `'@' || username` to `real_id`.
    fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()>;
```

  Add to `impl StoreWrite for threads_store::Store` (after `upsert_user`):
  ```rust
      fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()> {
          Self::resolve_author(self, username, real_id).map_err(Into::into)
      }
  ```

- [ ] **Call `resolve_author` in the orchestrator.** In `crates/threads-ingest/src/orchestrator.rs`:

  After `self.store.upsert_user(&me)?;` in `run_ingest_engagement`:
  ```rust
          let me = self.provider.fetch_me().await?;
          self.store.upsert_user(&me)?;
          if let Some(username) = &me.username {
              self.store.resolve_author(username, &me.id)?;
          }
          let seeds = self.store.posts_by_author(&me.id)?;
  ```

  After `self.store.upsert_user(&me)?;` in `run_ingest_me`:
  ```rust
          let me = self.provider.fetch_me().await?;
          self.store.upsert_user(&me)?;
          if let Some(username) = &me.username {
              self.store.resolve_author(username, &me.id)?;
          }

          let mut seen: HashSet<PostId> = HashSet::new();
  ```

- [ ] **Extend `MockStore` and add an ingest test.** In `crates/threads-ingest/tests/ingest_tests.rs`:

  Add a `resolve_author_calls` field to `MockStoreState`:
  ```rust
  #[derive(Default)]
  struct MockStoreState {
      upserted: Vec<Post>,
      upserted_users: Vec<User>,
      resolve_author_calls: Vec<(String, UserId)>,
      run_started: Vec<FetchRun>,
      run_ended: Vec<(String, u64, Option<String>)>,
  }
  ```

  Add `resolve_author` to `impl StoreWrite for MockStore`:
  ```rust
      fn resolve_author(&self, username: &str, real_id: &UserId) -> Result<()> {
          let mut s = self.state.lock().unwrap();
          s.resolve_author_calls.push((username.to_string(), real_id.clone()));
          Ok(())
      }
  ```

  Add the test:
  ```rust
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
      let call = state
          .resolve_author_calls
          .iter()
          .find(|(u, id)| u == "carol" && *id == UserId::new("real_77"));
      assert!(
          call.is_some(),
          "resolve_author must be called with (\"carol\", UserId(\"real_77\"))"
      );
  }
  ```

- [ ] **Run the ingest test (expect PASS):**
  ```
  cargo test -p threads-ingest resolve_author_called_with_correct_args
  ```
  Expected: PASS.

- [ ] **Run the full ingest suite to confirm no regressions:**
  ```
  cargo test -p threads-ingest
  ```
  Expected: all tests PASS.

- [ ] **Commit:**
  ```
  git add crates/threads-store/src/query.rs \
          crates/threads-store/src/store.rs \
          crates/threads-store/src/lib.rs \
          crates/threads-store/src/tests.rs \
          crates/threads-ingest/src/store_shim.rs \
          crates/threads-ingest/src/orchestrator.rs \
          crates/threads-ingest/tests/ingest_tests.rs
  git commit -m "feat(store): add resolve_author to rewrite @handle posts to real user id"
  ```

---

## Self-check for the executor

After completing all five tasks, run the full suite for every touched crate in order:

```bash
# 1. Core — 15 new merge_* tests + 2 existing must all pass.
cargo test -p threads-core

# 2. Store — 3 new tests (reupsert_via_sparse_reply_preserves_rich_root,
#             resolve_author_rewrites_handle_to_real_id,
#             posts_by_author_finds_my_replies_after_resolution)
#           + all pre-existing tests must pass.
cargo test -p threads-store

# 3. Provider — updated dto_to_post_synthesizes_author_from_username
#               + all pre-existing tests must pass.
cargo test -p threads-provider-official

# 4. Ingest — 3 new tests (ingest_engagement_upserts_me_profile,
#             ingest_me_upserts_me_profile, resolve_author_called_with_correct_args)
#           + all pre-existing tests must pass.
cargo test -p threads-ingest
```

Zero failures across all four crates is the green signal to open a PR for this branch.
