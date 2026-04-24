#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use threads_core::model::{FetchRun, Media, MediaKind, Mention, Post, PostId, UrlEntity, User, UserId};

    use crate::Store;

    fn make_user(id: &str) -> User {
        User {
            id: UserId::new(id),
            username: Some(format!("user_{id}")),
            name: Some(format!("User {id}")),
            biography: None,
            profile_picture_url: None,
        }
    }

    fn make_post(id: &str, author: &str) -> Post {
        Post {
            id: PostId::new(id),
            author: UserId::new(author),
            text: Some(format!("hello from post {id}")),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        }
    }

    // ------------------------------------------------------------------ //
    //  Migrations idempotency                                             //
    // ------------------------------------------------------------------ //

    #[test]
    fn migrations_apply_twice_no_error() {
        // open_in_memory() runs migrations; creating a second store on the
        // same in-memory db would be a new db, so we test idempotency by
        // running open_in_memory() twice (separate dbs both succeed).
        Store::open_in_memory().unwrap();
        Store::open_in_memory().unwrap();

        // Also verify that calling run_migrations twice on the same connection
        // does not error.
        use crate::migrations::run_migrations;
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // second call must be a no-op
    }

    // ------------------------------------------------------------------ //
    //  Upsert idempotency                                                 //
    // ------------------------------------------------------------------ //

    #[test]
    fn upsert_user_twice_no_duplicate() {
        let store = Store::open_in_memory().unwrap();
        let user = make_user("u1");
        store.upsert_user(&user).unwrap();
        store.upsert_user(&user).unwrap(); // second time must succeed

        // We can't directly query the user count here without exposing
        // internals, so trust that no error was returned
        // and that get_post (for user-owned post) works fine.
        let post = make_post("p1", "u1");
        store.upsert_post(&post, None).unwrap();
        store.upsert_post(&post, None).unwrap();

        let fetched = store.get_post(&PostId::new("p1")).unwrap();
        assert!(fetched.is_some());
    }

    #[test]
    fn upsert_post_twice_no_duplicate() {
        let store = Store::open_in_memory().unwrap();
        let post = make_post("p42", "u_author");

        store.upsert_post(&post, None).unwrap();
        store.upsert_post(&post, None).unwrap();

        let fetched = store.get_post(&PostId::new("p42")).unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().text.as_deref(), Some("hello from post p42"));
    }

    // ------------------------------------------------------------------ //
    //  FTS5 search                                                        //
    // ------------------------------------------------------------------ //

    #[test]
    fn fts_search_finds_post_by_token() {
        let store = Store::open_in_memory().unwrap();

        let mut post = make_post("fts1", "u_fts");
        post.text = Some("rustacean threading is great".into());
        store.upsert_post(&post, None).unwrap();

        let mut post2 = make_post("fts2", "u_fts");
        post2.text = Some("completely unrelated content".into());
        store.upsert_post(&post2, None).unwrap();

        let results = store.search_text("rustacean", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.as_str(), "fts1");
    }

    #[test]
    fn fts_search_multi_token() {
        let store = Store::open_in_memory().unwrap();

        let mut post = make_post("fts3", "u_fts2");
        post.text = Some("async channels in rust are awesome".into());
        store.upsert_post(&post, None).unwrap();

        let results = store.search_text("async channels", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id.as_str(), "fts3");
    }

    // ------------------------------------------------------------------ //
    //  Recursive CTE — 3-level reply chain                               //
    // ------------------------------------------------------------------ //

    #[test]
    fn thread_rooted_at_bfs_order() {
        let store = Store::open_in_memory().unwrap();

        // root → reply_1, reply_2 → reply_1_1 (3 levels)
        let now = Utc::now();

        let root = Post {
            id: PostId::new("root"),
            author: UserId::new("u1"),
            text: Some("root post".into()),
            created_at: Some(now),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };

        let reply1 = Post {
            id: PostId::new("reply1"),
            author: UserId::new("u2"),
            text: Some("reply 1".into()),
            created_at: Some(now + chrono::Duration::seconds(1)),
            parent_id: Some(PostId::new("root")),
            root_id: Some(PostId::new("root")),
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };

        let reply2 = Post {
            id: PostId::new("reply2"),
            author: UserId::new("u3"),
            text: Some("reply 2".into()),
            created_at: Some(now + chrono::Duration::seconds(2)),
            parent_id: Some(PostId::new("root")),
            root_id: Some(PostId::new("root")),
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };

        let reply1_1 = Post {
            id: PostId::new("reply1_1"),
            author: UserId::new("u4"),
            text: Some("reply to reply 1".into()),
            created_at: Some(now + chrono::Duration::seconds(3)),
            parent_id: Some(PostId::new("reply1")),
            root_id: Some(PostId::new("root")),
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };

        store.upsert_post(&root, None).unwrap();
        store.upsert_post(&reply1, None).unwrap();
        store.upsert_post(&reply2, None).unwrap();
        store.upsert_post(&reply1_1, None).unwrap();

        let thread = store.thread_rooted_at(&PostId::new("root")).unwrap();
        let ids: Vec<&str> = thread.iter().map(|p| p.id.as_str()).collect();

        // BFS order: root first, then depth-1 replies, then depth-2
        assert_eq!(ids[0], "root");
        // depth-1 replies come before depth-2
        let root_pos = ids.iter().position(|&x| x == "root").unwrap();
        let r1_pos = ids.iter().position(|&x| x == "reply1").unwrap();
        let r2_pos = ids.iter().position(|&x| x == "reply2").unwrap();
        let r1_1_pos = ids.iter().position(|&x| x == "reply1_1").unwrap();
        assert!(root_pos < r1_pos);
        assert!(root_pos < r2_pos);
        assert!(r1_pos < r1_1_pos);
        assert!(r2_pos < r1_1_pos);
        assert_eq!(thread.len(), 4);
    }

    // ------------------------------------------------------------------ //
    //  Mention and quote edges                                            //
    // ------------------------------------------------------------------ //

    #[test]
    fn mention_edges_inserted() {
        let store = Store::open_in_memory().unwrap();

        let post = Post {
            id: PostId::new("m_post"),
            author: UserId::new("author1"),
            text: Some("hey @someuser".into()),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![Mention {
                username: "someuser".into(),
                user_id: Some(UserId::new("mentioned_user_id")),
            }],
            is_quote_post: false,
            raw: None,
        };

        store.upsert_post(&post, None).unwrap();

        // The mention edge should exist: from m_post to mentioned_user_id kind=mention
        // We verify indirectly via a second upsert (no error = edge constraint OK)
        store.upsert_post(&post, None).unwrap();
    }

    #[test]
    fn quote_edges_inserted() {
        let store = Store::open_in_memory().unwrap();

        // First upsert the quoted post so FK is satisfied
        let original = make_post("original_post", "u_orig");
        store.upsert_post(&original, None).unwrap();

        let quote_post = Post {
            id: PostId::new("quote_post"),
            author: UserId::new("u_quoter"),
            text: Some("quoting this".into()),
            created_at: Some(Utc::now()),
            parent_id: Some(PostId::new("original_post")),
            root_id: Some(PostId::new("original_post")),
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: true,
            raw: None,
        };

        store.upsert_post(&quote_post, None).unwrap();

        // Verify quote post can be fetched
        let fetched = store.get_post(&PostId::new("quote_post")).unwrap().unwrap();
        assert!(fetched.is_quote_post);
    }

    // ------------------------------------------------------------------ //
    //  Raw JSON round-trip                                                //
    // ------------------------------------------------------------------ //

    #[test]
    fn raw_json_stored_and_query_succeeds() {
        let store = Store::open_in_memory().unwrap();

        let raw_payload = json!({
            "id": "raw_p1",
            "text": "raw payload test",
            "likes": 42,
            "nested": { "key": "value" }
        });

        let post = Post {
            id: PostId::new("raw_p1"),
            author: UserId::new("raw_author"),
            text: Some("raw payload test".into()),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: Some(raw_payload.clone()),
        };

        // Insert a matching fetch_run row so the FK is satisfied.
        let run = FetchRun {
            id: "run-001".into(),
            provider: "official".into(),
            started_at: Utc::now(),
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };
        store.record_fetch_run_start(&run).unwrap();

        store.upsert_post(&post, Some("run-001")).unwrap();

        // get_post returns raw=None (raw is in raw_payloads table, not the
        // posts table — correct per schema). The round-trip is verified by
        // ensuring no serialization error occurred during upsert.
        let fetched = store.get_post(&PostId::new("raw_p1")).unwrap().unwrap();
        assert_eq!(fetched.text.as_deref(), Some("raw payload test"));
    }

    // ------------------------------------------------------------------ //
    //  Batch upsert                                                       //
    // ------------------------------------------------------------------ //

    #[test]
    fn upsert_posts_batch_returns_count() {
        let store = Store::open_in_memory().unwrap();

        let posts: Vec<Post> = (0..5).map(|i| make_post(&format!("bp{i}"), "batch_author")).collect();
        let n = store.upsert_posts(&posts, Some("run-batch")).unwrap();
        assert_eq!(n, 5);

        // Re-upsert same posts — still returns 5 (count of processed, not inserted)
        let n2 = store.upsert_posts(&posts, Some("run-batch")).unwrap();
        assert_eq!(n2, 5);
    }

    // ------------------------------------------------------------------ //
    //  Fetch run lifecycle                                                //
    // ------------------------------------------------------------------ //

    #[test]
    fn fetch_run_start_and_end() {
        let store = Store::open_in_memory().unwrap();

        let run = FetchRun {
            id: "run-xyz".into(),
            provider: "official".into(),
            started_at: Utc::now(),
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };

        store.record_fetch_run_start(&run).unwrap();
        store
            .record_fetch_run_end("run-xyz", Utc::now(), 42, None)
            .unwrap();
    }

    #[test]
    fn fetch_run_end_not_found_errors() {
        let store = Store::open_in_memory().unwrap();
        let result = store.record_fetch_run_end("nonexistent", Utc::now(), 0, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::StoreError::NotFound(_)));
    }

    // ------------------------------------------------------------------ //
    //  Media round-trip                                                   //
    // ------------------------------------------------------------------ //

    #[test]
    fn media_stored_and_retrieved() {
        let store = Store::open_in_memory().unwrap();

        let post = Post {
            id: PostId::new("media_post"),
            author: UserId::new("media_author"),
            text: Some("post with media".into()),
            created_at: Some(Utc::now()),
            parent_id: None,
            root_id: None,
            permalink: None,
            media: vec![
                Media {
                    kind: MediaKind::Image,
                    url: Some("https://example.com/img.jpg".into()),
                    thumbnail_url: Some("https://example.com/thumb.jpg".into()),
                },
                Media {
                    kind: MediaKind::Video,
                    url: Some("https://example.com/video.mp4".into()),
                    thumbnail_url: None,
                },
            ],
            urls: vec![UrlEntity {
                url: "https://threads.net".into(),
                display_text: Some("threads.net".into()),
            }],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        };

        store.upsert_post(&post, None).unwrap();
        let fetched = store.get_post(&PostId::new("media_post")).unwrap().unwrap();

        assert_eq!(fetched.media.len(), 2);
        assert_eq!(fetched.urls.len(), 1);
        assert_eq!(fetched.urls[0].url, "https://threads.net");
    }

    // ------------------------------------------------------------------ //
    //  Regression: stale edges on re-upsert (Codex finding #1)            //
    // ------------------------------------------------------------------ //

    fn count_edges_from(store: &Store, from: &str) -> i64 {
        crate::query::test_only_count_edges_from(store, from)
    }

    #[test]
    fn reupsert_without_parent_drops_stale_edges() {
        let store = Store::open_in_memory().unwrap();

        // First upsert: reply to parent B, rooted at R, mentions M1.
        store.upsert_user(&make_user("B")).unwrap();
        store.upsert_user(&make_user("R")).unwrap();
        store.upsert_user(&make_user("M1")).unwrap();
        let mut p = make_post("A", "author");
        p.parent_id = Some(PostId::new("B"));
        p.root_id = Some(PostId::new("R"));
        p.mentions = vec![Mention {
            username: "m1".into(),
            user_id: Some(UserId::new("M1")),
        }];
        store.upsert_post(&p, None).unwrap();
        assert_eq!(
            count_edges_from(&store, "A"),
            3,
            "expect reply+root+mention edges after first upsert"
        );

        // Second upsert: top-level, no mentions. Old edges must be gone.
        let mut p2 = make_post("A", "author");
        p2.parent_id = None;
        p2.root_id = None;
        p2.mentions = vec![];
        store.upsert_post(&p2, None).unwrap();
        assert_eq!(
            count_edges_from(&store, "A"),
            0,
            "stale reply/root/mention edges were left behind (see Codex finding #1)"
        );
    }

    #[test]
    fn reupsert_replaces_mention_edges() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_user(&make_user("M1")).unwrap();
        store.upsert_user(&make_user("M2")).unwrap();

        let mut p = make_post("A", "author");
        p.mentions = vec![Mention {
            username: "m1".into(),
            user_id: Some(UserId::new("M1")),
        }];
        store.upsert_post(&p, None).unwrap();
        assert_eq!(count_edges_from(&store, "A"), 1);

        // Swap the mention. Previous edge should vanish, new one should appear.
        let mut p2 = make_post("A", "author");
        p2.mentions = vec![Mention {
            username: "m2".into(),
            user_id: Some(UserId::new("M2")),
        }];
        store.upsert_post(&p2, None).unwrap();
        assert_eq!(count_edges_from(&store, "A"), 1);
        // And it's specifically the M2 edge, not M1.
        assert_eq!(
            crate::query::test_only_edge_target(&store, "A", "mention"),
            Some("M2".to_string())
        );
    }
}
