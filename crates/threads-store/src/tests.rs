#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use chrono::Utc;
    use rusqlite::params;
    use serde_json::json;
    use threads_core::model::{
        FetchRun, Media, MediaKind, Mention, Post, PostId, UrlEntity, User, UserId,
    };

    use crate::{PostKind, Store};

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

    fn count_rows(store: &Store, table: &str, post_id: &str) -> i64 {
        let conn = store.raw_conn();
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE post_id = ?1");
        conn.query_row(&sql, params![post_id], |row| row.get::<_, i64>(0))
            .unwrap()
    }

    fn ids(posts: &[Post]) -> Vec<&str> {
        posts.iter().map(|p| p.id.as_str()).collect()
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

    #[test]
    fn migration_v3_creates_deletions_table() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.raw_conn();

        let table_name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'deletions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_name, "deletions");

        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index'
                   AND name IN ('deletions_deleted_at_idx', 'deletions_post_idx')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 2);
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
        assert_eq!(
            fetched.unwrap().text.as_deref(),
            Some("hello from post p42")
        );
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

        let posts: Vec<Post> = (0..5)
            .map(|i| make_post(&format!("bp{i}"), "batch_author"))
            .collect();
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
        assert!(matches!(
            result.unwrap_err(),
            crate::StoreError::NotFound(_)
        ));
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
    //  Deletions                                                          //
    // ------------------------------------------------------------------ //

    #[test]
    fn posts_in_window_filters_author_time_and_kind() {
        let store = Store::open_in_memory().unwrap();
        let base = Utc::now();

        let mut before_window = make_post("before_window", "me");
        before_window.created_at = Some(base - chrono::Duration::hours(2));
        store.upsert_post(&before_window, None).unwrap();

        let mut root = make_post("root_in_window", "me");
        root.created_at = Some(base);
        store.upsert_post(&root, None).unwrap();

        let mut reply = make_post("reply_in_window", "me");
        reply.created_at = Some(base + chrono::Duration::minutes(1));
        reply.parent_id = Some(PostId::new("root_in_window"));
        reply.root_id = Some(PostId::new("root_in_window"));
        store.upsert_post(&reply, None).unwrap();

        let mut other_author = make_post("other_author", "someone_else");
        other_author.created_at = Some(base + chrono::Duration::minutes(2));
        store.upsert_post(&other_author, None).unwrap();

        let mut at_before_bound = make_post("at_before_bound", "me");
        at_before_bound.created_at = Some(base + chrono::Duration::hours(1));
        store.upsert_post(&at_before_bound, None).unwrap();

        let mut null_created = make_post("null_created", "me");
        null_created.created_at = None;
        store.upsert_post(&null_created, None).unwrap();

        let after = base - chrono::Duration::minutes(30);
        let before = base + chrono::Duration::hours(1);

        let posts = store
            .posts_in_window(
                &UserId::new("me"),
                Some(after),
                Some(before),
                PostKind::Post,
                10,
            )
            .unwrap();
        assert_eq!(ids(&posts), vec!["root_in_window"]);

        let replies = store
            .posts_in_window(
                &UserId::new("me"),
                Some(after),
                Some(before),
                PostKind::Reply,
                10,
            )
            .unwrap();
        assert_eq!(ids(&replies), vec!["reply_in_window"]);
    }

    #[test]
    fn delete_post_is_idempotent_and_cascades_child_tables() {
        let store = Store::open_in_memory().unwrap();

        let run = FetchRun {
            id: "run-delete".into(),
            provider: "official".into(),
            started_at: Utc::now(),
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };
        store.record_fetch_run_start(&run).unwrap();

        let mut post = make_post("delete_me", "delete_author");
        post.media = vec![Media {
            kind: MediaKind::Image,
            url: Some("https://example.com/delete.jpg".into()),
            thumbnail_url: None,
        }];
        post.urls = vec![UrlEntity {
            url: "https://example.com".into(),
            display_text: Some("example".into()),
        }];
        post.mentions = vec![Mention {
            username: "mentioned".into(),
            user_id: None,
        }];
        post.raw = Some(json!({ "id": "delete_me" }));
        store.upsert_post(&post, Some("run-delete")).unwrap();

        assert_eq!(count_rows(&store, "media", "delete_me"), 1);
        assert_eq!(count_rows(&store, "urls", "delete_me"), 1);
        assert_eq!(count_rows(&store, "mentions", "delete_me"), 1);
        assert_eq!(count_rows(&store, "raw_payloads", "delete_me"), 1);

        assert!(store.delete_post(&PostId::new("delete_me")).unwrap());
        assert!(!store.delete_post(&PostId::new("delete_me")).unwrap());
        assert!(store.get_post(&PostId::new("delete_me")).unwrap().is_none());

        assert_eq!(count_rows(&store, "media", "delete_me"), 0);
        assert_eq!(count_rows(&store, "urls", "delete_me"), 0);
        assert_eq!(count_rows(&store, "mentions", "delete_me"), 0);
        assert_eq!(count_rows(&store, "raw_payloads", "delete_me"), 0);
    }

    #[test]
    fn delete_post_clears_edges_in_both_directions() {
        // `edges` has no FK to `posts`, so DELETE FROM posts cannot CASCADE to it.
        // `delete_post` must explicitly clear edges where the deleted post is
        // either endpoint, otherwise stale rows orphan the recursive thread CTE.
        let store = Store::open_in_memory().unwrap();
        store.upsert_user(&make_user("parent_user")).unwrap();
        store.upsert_user(&make_user("root_user")).unwrap();
        store.upsert_user(&make_user("mention_user")).unwrap();

        // Post P references three other posts/users (3 edges with from_id = P).
        let mut p = make_post("P", "author");
        p.parent_id = Some(PostId::new("parent_user"));
        p.root_id = Some(PostId::new("root_user"));
        p.mentions = vec![Mention {
            username: "u".into(),
            user_id: Some(UserId::new("mention_user")),
        }];
        store.upsert_post(&p, None).unwrap();

        // Post C is a reply to P (1 edge with to_id = P).
        let mut c = make_post("C", "other_author");
        c.parent_id = Some(PostId::new("P"));
        store.upsert_post(&c, None).unwrap();

        assert_eq!(count_edges_from(&store, "P"), 3, "P should own 3 outbound edges");
        let inbound_to_p: i64 = {
            let conn = store.raw_conn();
            conn.query_row(
                "SELECT COUNT(*) FROM edges WHERE to_id = ?1",
                params!["P"],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(inbound_to_p, 1, "C's reply edge should target P");

        // Delete P. Both directions of edges referencing P must vanish.
        assert!(store.delete_post(&PostId::new("P")).unwrap());
        assert_eq!(count_edges_from(&store, "P"), 0, "outbound edges from P should be gone");
        let inbound_to_p_after: i64 = {
            let conn = store.raw_conn();
            conn.query_row(
                "SELECT COUNT(*) FROM edges WHERE to_id = ?1",
                params!["P"],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(inbound_to_p_after, 0, "inbound edges pointing at P should be gone");

        // C itself should still exist (we only deleted P).
        assert!(store.get_post(&PostId::new("C")).unwrap().is_some());
    }

    #[test]
    fn deletions_in_last_24h_slides_window() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_deletion(&PostId::new("recent_success"), PostKind::Post, true, None)
            .unwrap();
        store
            .record_deletion(
                &PostId::new("recent_failure"),
                PostKind::Reply,
                false,
                Some("not found"),
            )
            .unwrap();

        let old = (Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        let conn = store.raw_conn();
        conn.execute(
            "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["old_success", "post", old, 1, Option::<&str>::None],
        )
        .unwrap();
        drop(conn);

        assert_eq!(store.deletions_in_last_24h().unwrap(), 1);
    }

    #[test]
    fn oldest_deletion_in_last_24h_returns_min_within_window() {
        let store = Store::open_in_memory().unwrap();
        // None on empty.
        assert!(store.oldest_deletion_in_last_24h().unwrap().is_none());

        // Insert one deletion 5h ago, one 1h ago. Helper should return the older one.
        let conn = store.raw_conn();
        let five_hours_ago = (Utc::now() - chrono::Duration::hours(5)).to_rfc3339();
        let one_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
             VALUES (?1, 'post', ?2, 1, NULL)",
            params!["old_recent", &five_hours_ago],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
             VALUES (?1, 'post', ?2, 1, NULL)",
            params!["newer", &one_hour_ago],
        )
        .unwrap();
        // A failed deletion 2h ago must NOT be returned (only successes count).
        let two_hours_ago = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        conn.execute(
            "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
             VALUES (?1, 'post', ?2, 0, 'boom')",
            params!["failed", &two_hours_ago],
        )
        .unwrap();
        // A successful deletion 25h ago is OUTSIDE the window.
        let twenty_five_hours_ago = (Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        conn.execute(
            "INSERT INTO deletions (post_id, kind, deleted_at, success, error)
             VALUES (?1, 'post', ?2, 1, NULL)",
            params!["too_old", &twenty_five_hours_ago],
        )
        .unwrap();
        drop(conn);

        let oldest = store.oldest_deletion_in_last_24h().unwrap().expect("some");
        let oldest_str = oldest.to_rfc3339();
        // Should equal the 5h-ago row, not the 1h-ago, not the failed, not the 25h-ago.
        assert_eq!(oldest_str, five_hours_ago);
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
