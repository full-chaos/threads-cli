use async_trait::async_trait;
use threads_core::publish::{
    ContainerId, ContainerStatus, MediaInput, PublishRequest, PublishingLimits,
};
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, Cursor, Page, Post, PostId, Provider, Result,
    User, UserId,
};
use threads_manifest::Manifest;

use crate::client::HttpClient;

mod audience;
mod paths;
mod posts;
mod publish_params;
mod publishing;
mod reads;

pub struct OfficialProvider {
    pub(crate) http: HttpClient,
    pub(crate) manifest: Manifest,
}

impl OfficialProvider {
    pub fn new(http: HttpClient, manifest: Manifest) -> Self {
        Self { http, manifest }
    }

    pub(super) fn endpoint_fields(&self, key: &str) -> Option<String> {
        paths::endpoint_fields(&self.manifest, key)
    }

    pub(super) fn object_path(&self, key: &str) -> Option<String> {
        paths::object_path(&self.manifest, key)
    }

    pub(super) fn edge_path(&self, key: &str) -> Option<String> {
        paths::edge_path(&self.manifest, key)
    }

    pub(super) fn action_path(&self, key: &str) -> Option<String> {
        paths::action_path(&self.manifest, key)
    }

    pub(super) fn substitute_post_id(path: &str, post_id: &PostId) -> String {
        // Delete actions use either `{post-id}` or `{reply-id}` for the same
        // media-object id slot, so one helper substitutes both placeholders.
        paths::substitute_post_id(path, post_id)
    }

    pub(super) fn substitute_user_id(path: &str, user_id: &UserId) -> String {
        paths::substitute_user_id(path, user_id)
    }

    pub(crate) fn substitute_container_id(path: &str, id: &ContainerId) -> String {
        paths::substitute_container_id(path, id)
    }
}

#[async_trait]
impl Provider for OfficialProvider {
    fn name(&self) -> &'static str {
        "official"
    }

    async fn fetch_me(&self) -> Result<User> {
        reads::fetch_me(self).await
    }

    async fn fetch_my_threads(&self, cursor: Option<Cursor>) -> Result<Page<Post>> {
        reads::fetch_my_threads(self, cursor).await
    }

    async fn fetch_my_replies(&self, cursor: Option<Cursor>) -> Result<Page<Post>> {
        reads::fetch_my_replies(self, cursor).await
    }

    async fn fetch_audience_insight(
        &self,
        user_id: &UserId,
        query: AudienceInsightQuery,
    ) -> Result<AudienceInsightResult> {
        audience::fetch_insight(self, user_id, query).await
    }

    async fn fetch_mentions(
        &self,
        user_id: &UserId,
        cursor: Option<Cursor>,
        _limit: usize,
    ) -> Result<Page<Post>> {
        reads::fetch_mentions(self, user_id, cursor).await
    }

    async fn fetch_replies(&self, post_id: &PostId, cursor: Option<Cursor>) -> Result<Page<Post>> {
        reads::fetch_replies(self, post_id, cursor).await
    }

    async fn fetch_thread(&self, root_id: &PostId) -> Result<Vec<Post>> {
        reads::fetch_thread(self, root_id).await
    }

    async fn delete_post(&self, post_id: &PostId) -> Result<()> {
        publishing::delete_post(self, post_id).await
    }

    async fn delete_reply(&self, reply_id: &PostId) -> Result<()> {
        publishing::delete_reply(self, reply_id).await
    }

    async fn create_container(&self, req: &PublishRequest) -> threads_core::Result<ContainerId> {
        publishing::create_container(self, req).await
    }

    async fn publish_container(
        &self,
        id: &ContainerId,
    ) -> threads_core::Result<threads_core::PostId> {
        publishing::publish_container(self, id).await
    }

    async fn container_status(&self, id: &ContainerId) -> threads_core::Result<ContainerStatus> {
        publishing::container_status(self, id).await
    }

    async fn publishing_limits(&self) -> threads_core::Result<PublishingLimits> {
        publishing::publishing_limits(self).await
    }

    async fn fetch_post(
        &self,
        id: &threads_core::PostId,
    ) -> threads_core::Result<threads_core::Post> {
        reads::fetch_post(self, id).await
    }

    async fn create_carousel_item(&self, item: &MediaInput) -> threads_core::Result<ContainerId> {
        publishing::create_carousel_item(self, item).await
    }

    async fn create_carousel_container(
        &self,
        req: &PublishRequest,
        children: &[ContainerId],
    ) -> threads_core::Result<ContainerId> {
        publishing::create_carousel_container(self, req, children).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        client::HttpClient,
        dto::{Envelope, InsightsEnvelope, PostDto},
    };
    use threads_core::{DemographicDimension, Error};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{Duration, timeout},
    };

    struct ServerReply {
        status: &'static str,
        body: &'static str,
    }

    async fn one_shot_server(reply: ServerReply) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = vec![0_u8; 8_192];
            let bytes = timeout(Duration::from_secs(2), stream.read(&mut request))
                .await
                .unwrap()
                .unwrap();
            let response = format!(
                "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                reply.status,
                reply.body.len(),
                reply.body,
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request[..bytes].to_vec()).unwrap()
        });
        (format!("http://{address}"), handle)
    }

    fn audience_provider(base_url: &str) -> OfficialProvider {
        let manifest_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        OfficialProvider::new(
            HttpClient::new(base_url, "test-token").unwrap(),
            Manifest::from_path(manifest_path).unwrap(),
        )
    }

    fn request_url(request: &str) -> url::Url {
        let target = request.split_whitespace().nth(1).unwrap();
        url::Url::parse(&format!("http://local{target}")).unwrap()
    }

    // ---- publish param building ----

    #[test]
    fn create_container_text_params_include_media_type_and_text() {
        use threads_core::publish::{PublishMediaType, PublishRequest};
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("Hello Threads!".into()),
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let params = publish_params::create(&req);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("TEXT"));
        assert_eq!(map.get("text").copied(), Some("Hello Threads!"));
        assert!(!params.iter().any(|(k, _)| *k == "reply_to_id"));
    }

    #[test]
    fn create_container_reply_params_include_reply_to_id() {
        use threads_core::{
            PostId,
            publish::{PublishMediaType, PublishRequest},
        };
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("a reply".into()),
            reply_to_id: Some(PostId::new("parent_post_99")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let params = publish_params::create(&req);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("reply_to_id").copied(), Some("parent_post_99"));
    }

    #[test]
    fn create_container_image_params_include_image_url() {
        use threads_core::publish::{MediaInput, MediaInputKind, PublishMediaType, PublishRequest};
        let req = PublishRequest {
            media_type: PublishMediaType::Image,
            text: None,
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![MediaInput {
                kind: MediaInputKind::Image,
                url: "https://example.com/photo.jpg".into(),
            }],
        };
        let params = publish_params::create(&req);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("IMAGE"));
        assert_eq!(
            map.get("image_url").copied(),
            Some("https://example.com/photo.jpg")
        );
    }

    #[test]
    fn substitute_container_id_replaces_placeholder() {
        let path = "/v1.0/{container-id}";
        use threads_core::publish::ContainerId;
        let cid = ContainerId::new("ctr_42");
        let result = OfficialProvider::substitute_container_id(path, &cid);
        assert_eq!(result, "/v1.0/ctr_42");
    }

    #[test]
    fn carousel_item_params_set_is_carousel_item() {
        use threads_core::publish::{MediaInput, MediaInputKind};
        let item = MediaInput {
            kind: MediaInputKind::Image,
            url: "https://example.com/img.jpg".into(),
        };
        let params = publish_params::carousel_item(&item);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("IMAGE"));
        assert_eq!(
            map.get("image_url").copied(),
            Some("https://example.com/img.jpg")
        );
        assert_eq!(map.get("is_carousel_item").copied(), Some("true"));
    }

    #[test]
    fn carousel_parent_params_set_children_csv() {
        use threads_core::{
            PostId,
            publish::{ContainerId, PublishMediaType, PublishRequest},
        };
        let req = PublishRequest {
            media_type: PublishMediaType::Carousel,
            text: Some("carousel caption".into()),
            reply_to_id: Some(PostId::new("parent_post_7")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let children = vec![ContainerId::new("ctr_1"), ContainerId::new("ctr_2")];
        let params = publish_params::carousel_parent(&req, &children);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("CAROUSEL"));
        assert_eq!(map.get("children").copied(), Some("ctr_1,ctr_2"));
        assert_eq!(map.get("text").copied(), Some("carousel caption"));
        assert_eq!(map.get("reply_to_id").copied(), Some("parent_post_7"));
    }

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
        let post = posts::dto_to_post(dto, None).expect("username is a valid sparse author");
        assert_eq!(post.id, PostId::new("p1"));
        assert_eq!(post.author, UserId::new("@alice"));
        assert_eq!(post.author_username.as_deref(), Some("alice"));
    }

    #[test]
    fn dto_to_post_propagates_root_hint_when_missing() {
        let dto = PostDto {
            id: "r1".into(),
            username: Some("b".into()),
            text: None,
            timestamp: None,
            permalink: None,
            media_type: None,
            media_url: None,
            thumbnail_url: None,
            is_quote_post: false,
            owner: None,
            children: None,
            replied_to: Some(crate::dto::PostRefDto {
                id: "parent".into(),
            }),
            root_post: None,
            is_reply: Some(true),
            shortcode: None,
        };
        let post = posts::dto_to_post(dto, Some(&PostId::new("root-x")))
            .expect("username is a valid sparse author");
        assert_eq!(post.parent_id, Some(PostId::new("parent")));
        assert_eq!(post.root_id, Some(PostId::new("root-x")));
    }

    #[test]
    fn parse_timestamp_accepts_meta_format() {
        // Meta returns `+0000` (no colon), which is valid ISO 8601 but not
        // RFC 3339. chrono's strict RFC 3339 parser rejects it.
        let ts = posts::parse_timestamp("2026-04-24T18:15:44+0000").unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-04-24T18:15:44+00:00");
    }

    #[test]
    fn parse_timestamp_accepts_rfc3339() {
        let ts = posts::parse_timestamp("2026-04-24T18:15:44+00:00").unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-04-24T18:15:44+00:00");
    }

    #[test]
    fn parse_timestamp_rejects_garbage() {
        assert!(posts::parse_timestamp("not a date").is_none());
        assert!(posts::parse_timestamp("").is_none());
    }

    #[test]
    fn envelope_to_page_extracts_after_cursor() {
        let env: Envelope<PostDto> = serde_json::from_str(
            r#"{"data":[{"id":"1","username":"u"}],"paging":{"cursors":{"after":"NXT"}}}"#,
        )
        .unwrap();
        let page = posts::envelope_to_page(env, None).expect("fixture has a post author");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next.as_ref().map(|c| c.0.as_str()), Some("NXT"));
    }

    #[test]
    fn envelope_to_page_extracts_after_cursor_from_next_url() {
        // Given: an official pagination response that provides only its next URL.
        let env: Envelope<PostDto> = serde_json::from_str(
            r#"{"data":[{"id":"1","username":"u"}],"paging":{"next":"https://graph.threads.net/v1.0/me/threads?after=NXT"}}"#,
        )
        .unwrap();

        // When: the response is converted to the core page.
        let page = posts::envelope_to_page(env, None).expect("fixture has a post author");

        // Then: the documented continuation cursor is preserved.
        assert_eq!(page.next, Some(Cursor("NXT".into())));
    }

    #[test]
    fn substitutes_threads_user_id_in_versioned_audience_path() {
        let user_id = UserId::new("17841400000000000");
        let path = OfficialProvider::substitute_user_id(
            "/v1.0/{threads-user-id}/threads_insights",
            &user_id,
        );
        assert_eq!(path, "/v1.0/17841400000000000/threads_insights");
    }

    #[test]
    fn substitutes_identifiers_as_percent_encoded_path_segments() {
        // Given: identifiers containing path and query delimiters.
        let post_id = PostId::new("post/one?two#three%");
        let user_id = UserId::new("user/one?two#three%");
        let container_id = ContainerId::new("container/one?two#three%");

        // When: each identifier is substituted into its manifest path.
        let post_path = OfficialProvider::substitute_post_id("/v1.0/{post-id}/replies", &post_id);
        let reply_path = OfficialProvider::substitute_post_id("/v1.0/{reply-id}", &post_id);
        let user_path =
            OfficialProvider::substitute_user_id("/v1.0/{threads-user-id}/mentions", &user_id);
        let container_path =
            OfficialProvider::substitute_container_id("/v1.0/{container-id}", &container_id);

        // Then: every substituted value remains one path segment.
        assert_eq!(post_path, "/v1.0/post%2Fone%3Ftwo%23three%25/replies");
        assert_eq!(reply_path, "/v1.0/post%2Fone%3Ftwo%23three%25");
        assert_eq!(user_path, "/v1.0/user%2Fone%3Ftwo%23three%25/mentions");
        assert_eq!(container_path, "/v1.0/container%2Fone%3Ftwo%23three%25");
    }

    #[test]
    fn substitutes_spaces_plus_percent_and_unicode_as_path_segment_bytes() {
        // Given: an identifier with characters whose form encoding differs from a URL path.
        let user_id = UserId::new("a b+c%/✓");

        // When: it fills the documented versioned path.
        let path =
            OfficialProvider::substitute_user_id("/v1.0/{threads-user-id}/mentions", &user_id);

        // Then: every non-unreserved byte is percent encoded, never form encoded.
        assert_eq!(path, "/v1.0/a%20b%2Bc%25%2F%E2%9C%93/mentions");
    }

    #[test]
    fn builds_only_documented_audience_queries() {
        let count = audience::insight_params(&AudienceInsightQuery::FollowersCount);
        assert_eq!(count, [("metric", "followers_count".to_string())]);

        for (dimension, wire) in [
            (DemographicDimension::Country, "country"),
            (DemographicDimension::City, "city"),
            (DemographicDimension::Age, "age"),
            (DemographicDimension::Gender, "gender"),
        ] {
            let demographics =
                audience::insight_params(&AudienceInsightQuery::FollowerDemographics(dimension));
            assert_eq!(
                demographics,
                [
                    ("metric", "follower_demographics".to_string()),
                    ("breakdown", wire.to_string()),
                ]
            );
        }
    }

    #[tokio::test]
    async fn fetches_followers_count_over_the_versioned_insights_path() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_followers_count.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_audience_insight(
                &UserId::new("17841400000000000"),
                AudienceInsightQuery::FollowersCount,
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);

        assert_eq!(result, AudienceInsightResult::FollowersCount(1234));
        assert_eq!(
            request_url.path(),
            "/v1.0/17841400000000000/threads_insights"
        );
        assert_eq!(
            request_url
                .query_pairs()
                .collect::<std::collections::HashMap<_, _>>(),
            std::collections::HashMap::from([
                ("metric".into(), "followers_count".into()),
                ("access_token".into(), "test-token".into()),
            ])
        );
    }

    #[tokio::test]
    async fn fetches_one_demographic_breakdown_without_date_ranges() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_demographics_country.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_audience_insight(
                &UserId::new("17841400000000000"),
                AudienceInsightQuery::FollowerDemographics(DemographicDimension::Country),
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);
        let query = request_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert!(
            matches!(result, AudienceInsightResult::Demographics(insight) if insight.dimension == DemographicDimension::Country && insight.buckets.len() == 2)
        );
        assert_eq!(
            query.get("metric").map(|value| value.as_ref()),
            Some("follower_demographics")
        );
        assert_eq!(
            query.get("breakdown").map(|value| value.as_ref()),
            Some("country")
        );
        assert!(!query.contains_key("since"));
        assert!(!query.contains_key("until"));
    }

    #[tokio::test]
    async fn fetches_mentions_with_a_fixed_limit_and_cursor() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/mentions_page.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let page = provider
            .fetch_mentions(
                &UserId::new("17841400000000000"),
                Some(Cursor("after-cursor".into())),
                12,
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);
        let query = request_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(request_url.path(), "/v1.0/17841400000000000/mentions");
        assert_eq!(query.get("limit").map(|value| value.as_ref()), Some("100"));
        assert_eq!(
            query.get("after").map(|value| value.as_ref()),
            Some("after-cursor")
        );
        assert_eq!(page.next, Some(Cursor("QVFIUnhR".into())));
        assert_eq!(page.items[0].author_username.as_deref(), Some("author"));
    }

    #[test]
    fn parses_terminal_mentions_fixture_without_a_cursor() {
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/mentions_terminal_page.json"
        ));
        let mentions: Envelope<PostDto> = serde_json::from_str(fixture).unwrap();
        let page = posts::envelope_to_page(mentions, None).expect("fixture has no posts");

        assert!(page.items.is_empty());
        assert!(page.next.is_none());
    }

    #[test]
    fn rejects_empty_insight_data_during_conversion() {
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/audience_empty_data.json"
        ));
        let insights: InsightsEnvelope = serde_json::from_str(fixture).unwrap();
        let result = audience::into_result(insights, AudienceInsightQuery::FollowersCount);

        assert!(matches!(result, Err(Error::Parse(_))));
    }

    #[tokio::test]
    async fn fetch_mentions_maps_forbidden_responses_to_permission_denied() {
        let reply = ServerReply {
            status: "403 Forbidden",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_403.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_mentions(&UserId::new("17841400000000000"), None, 100)
            .await;
        let request = server.await.unwrap();

        assert!(request.starts_with("GET /v1.0/17841400000000000/mentions?"));
        assert!(matches!(result, Err(Error::PermissionDenied(_))));
    }
}
