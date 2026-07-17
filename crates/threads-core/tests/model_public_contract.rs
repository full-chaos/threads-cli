use chrono::{DateTime, Utc};
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, AudienceSnapshot, Cursor, DemographicBucket,
    DemographicDimension, DemographicInsight, Edge, EdgeKind, EngagedAccount, EngagementSort,
    FetchRun, Media, MediaKind, Mention, Page, Post, PostId, UrlEntity, User, UserId,
};

#[test]
fn public_model_paths_and_serde_shapes_are_stable() {
    let observed_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
        .unwrap()
        .with_timezone(&Utc);
    let user = User {
        id: UserId::new("user-1"),
        username: Some("alice".into()),
        name: Some("Alice".into()),
        biography: Some("Bio".into()),
        profile_picture_url: Some("https://example.test/alice.jpg".into()),
    };
    let mention = Mention {
        username: "bob".into(),
        user_id: Some(UserId::new("user-2")),
    };
    let post = Post {
        id: PostId::new("post-1"),
        author: UserId::new("user-1"),
        author_username: None,
        text: Some("Hello".into()),
        created_at: Some(observed_at),
        parent_id: Some(PostId::new("parent-1")),
        root_id: Some(PostId::new("root-1")),
        permalink: Some("https://threads.net/post-1".into()),
        media: vec![Media {
            kind: MediaKind::Image,
            url: Some("https://example.test/image.jpg".into()),
            thumbnail_url: None,
        }],
        urls: vec![UrlEntity {
            url: "https://example.test".into(),
            display_text: Some("Example".into()),
        }],
        mentions: vec![mention.clone()],
        is_quote_post: false,
        raw: None,
    };
    let page = Page::new(vec![mention.clone()], Some(Cursor("cursor-1".into())));
    let edge = Edge {
        from: "post-1".into(),
        to: "root-1".into(),
        kind: EdgeKind::Root,
    };
    let fetch_run = FetchRun {
        id: "run-1".into(),
        provider: "official".into(),
        started_at: observed_at,
        finished_at: None,
        posts_fetched: 3,
        error: None,
    };
    let snapshot = AudienceSnapshot {
        account_id: UserId::new("user-1"),
        observed_at,
        followers_count: 42,
        demographics: vec![DemographicBucket {
            dimension: DemographicDimension::Country,
            bucket: "US".into(),
            value: 40,
        }],
    };
    let engaged = EngagedAccount {
        user_id: UserId::new("user-2"),
        username: Some("bob".into()),
        replies: 2,
        mentions: 1,
        total: 3,
    };

    let _: threads_core::model::User = user.clone();
    let _: threads_core::model::Post = post.clone();
    let _: threads_core::model::Mention = mention.clone();
    let _: threads_core::model::Page<Mention> = page.clone();
    let _: threads_core::model::Edge = edge.clone();
    let _: threads_core::model::FetchRun = fetch_run.clone();
    let _: threads_core::model::AudienceSnapshot = snapshot.clone();
    let _: threads_core::model::EngagedAccount = engaged.clone();

    assert_eq!(
        serde_json::to_value(PostId::new("post-1")).unwrap(),
        "post-1"
    );
    assert_eq!(
        serde_json::to_value(UserId::new("user-1")).unwrap(),
        "user-1"
    );
    assert_eq!(
        serde_json::to_value(Cursor("cursor-1".into())).unwrap(),
        "cursor-1"
    );
    assert_eq!(
        serde_json::to_value(user).unwrap(),
        serde_json::json!({
            "id": "user-1",
            "username": "alice",
            "name": "Alice",
            "biography": "Bio",
            "profile_picture_url": "https://example.test/alice.jpg",
        })
    );
    assert_eq!(
        serde_json::to_value(post).unwrap(),
        serde_json::json!({
            "id": "post-1",
            "author": "user-1",
            "text": "Hello",
            "created_at": "2026-01-01T00:00:00Z",
            "parent_id": "parent-1",
            "root_id": "root-1",
            "permalink": "https://threads.net/post-1",
            "media": [{
                "kind": "image",
                "url": "https://example.test/image.jpg",
                "thumbnail_url": null,
            }],
            "urls": [{"url": "https://example.test", "display_text": "Example"}],
            "mentions": [{"username": "bob", "user_id": "user-2"}],
            "is_quote_post": false,
        })
    );
    assert_eq!(
        serde_json::to_value(page).unwrap(),
        serde_json::json!({
            "items": [{"username": "bob", "user_id": "user-2"}],
            "next": "cursor-1",
        })
    );
    assert_eq!(
        serde_json::to_value(edge).unwrap(),
        serde_json::json!({"from": "post-1", "to": "root-1", "kind": "root"})
    );
    assert_eq!(
        serde_json::to_value(fetch_run).unwrap(),
        serde_json::json!({
            "id": "run-1",
            "provider": "official",
            "started_at": "2026-01-01T00:00:00Z",
            "finished_at": null,
            "posts_fetched": 3,
            "error": null,
        })
    );
    assert_eq!(
        serde_json::to_value(AudienceInsightQuery::FollowerDemographics(
            DemographicDimension::Country
        ))
        .unwrap(),
        serde_json::json!({"follower_demographics": "country"})
    );
    assert_eq!(
        serde_json::to_value(AudienceInsightResult::Demographics(DemographicInsight {
            dimension: DemographicDimension::Country,
            buckets: snapshot.demographics,
        }))
        .unwrap(),
        serde_json::json!({
            "Demographics": {
                "dimension": "country",
                "buckets": [{"dimension": "country", "bucket": "US", "value": 40}],
            },
        })
    );
    assert_eq!(
        serde_json::to_value(EngagementSort::Mentions).unwrap(),
        serde_json::json!("mentions")
    );
    assert_eq!(
        serde_json::to_value(engaged).unwrap(),
        serde_json::json!({
            "user_id": "user-2",
            "username": "bob",
            "replies": 2,
            "mentions": 1,
            "total": 3,
        })
    );
}
