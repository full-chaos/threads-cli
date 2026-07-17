use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, Cursor, DemographicDimension, DemographicInsight,
    Error, Provider, UserId,
};
use threads_manifest::Manifest;
use threads_provider_official::{OfficialProvider, client::HttpClient};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, timeout},
};

struct Reply {
    status: &'static str,
    body: &'static str,
}

async fn server(reply: Reply) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let address = listener.local_addr().expect("local address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("accept timeout")
            .expect("accept connection");
        let mut request = vec![0_u8; 8192];
        let count = timeout(Duration::from_secs(2), stream.read(&mut request))
            .await
            .expect("read timeout")
            .expect("read request");
        let response = format!(
            "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            reply.status,
            reply.body.len(),
            reply.body
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write response");
        String::from_utf8(request[..count].to_vec()).expect("UTF-8 request")
    });
    (format!("http://{address}"), task)
}

fn provider(base: &str) -> OfficialProvider {
    OfficialProvider::new(
        HttpClient::new(base, "test-token").expect("test client"),
        Manifest::from_path(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        ))
        .expect("manifest"),
    )
}

fn request_url(request: &str) -> url::Url {
    url::Url::parse(&format!(
        "http://local{}",
        request.split_whitespace().nth(1).expect("request target")
    ))
    .expect("request URL")
}

#[tokio::test]
async fn official_audience_requests_use_only_documented_paths_queries_and_local_tcp() {
    // Given: local TCP responders for the official Insights and Mentions fixture payloads.
    let account = UserId::new("17841400000000000");
    let (insights_base, insights_server) = server(Reply {
        status: "200 OK",
        body: include_str!("../../threads-ingest/tests/fixtures/audience_followers_count.json"),
    })
    .await;
    let (mentions_base, mentions_server) = server(Reply {
        status: "200 OK",
        body: include_str!("../../threads-ingest/tests/fixtures/mentions_page.json"),
    })
    .await;

    // When: the official provider requests a count and a cursor-bearing mentions page.
    let count = provider(&insights_base)
        .fetch_audience_insight(&account, AudienceInsightQuery::FollowersCount)
        .await
        .expect("count response");
    let mentions = provider(&mentions_base)
        .fetch_mentions(&account, Some(Cursor("cursor-1".into())), 1)
        .await
        .expect("mentions response");
    let insights_url = request_url(&insights_server.await.expect("insights server"));
    let mentions_url = request_url(&mentions_server.await.expect("mentions server"));

    // Then: no Meta endpoint is contacted and each request is exactly contract-shaped.
    assert!(matches!(
        count,
        threads_core::AudienceInsightResult::FollowersCount(1234)
    ));
    assert_eq!(
        insights_url.path(),
        "/v1.0/17841400000000000/threads_insights"
    );
    assert_eq!(
        insights_url
            .query_pairs()
            .find(|(key, _)| key == "metric")
            .map(|(_, value)| value),
        Some("followers_count".into())
    );
    assert_eq!(mentions_url.path(), "/v1.0/17841400000000000/mentions");
    let query = mentions_url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(query.get("limit").map(|value| value.as_ref()), Some("100"));
    assert_eq!(
        query.get("after").map(|value| value.as_ref()),
        Some("cursor-1")
    );
    assert_eq!(mentions.items.len(), 1);
}

#[tokio::test]
async fn official_audience_errors_map_from_local_http_without_real_network_access() {
    // Given: local malformed, unauthorized, and forbidden official response fixtures.
    let cases = [
        (
            "401 Unauthorized",
            include_str!("../../threads-ingest/tests/fixtures/audience_401.json"),
            "auth",
        ),
        (
            "403 Forbidden",
            include_str!("../../threads-ingest/tests/fixtures/audience_403.json"),
            "permission",
        ),
        ("200 OK", "{malformed", "parse"),
    ];

    // When: each response is requested through the real HTTP client/provider boundary.
    for (status, body, expected) in cases {
        let (base, local_server) = server(Reply { status, body }).await;
        let result = provider(&base)
            .fetch_audience_insight(
                &UserId::new("account"),
                AudienceInsightQuery::FollowersCount,
            )
            .await;
        let _request = local_server.await.expect("local server");

        // Then: documented failures are typed and stay inside the local test server.
        match (expected, result) {
            ("auth", Err(Error::Auth(_)))
            | ("permission", Err(Error::PermissionDenied(_)))
            | ("parse", Err(Error::Parse(_))) => {}
            (_, other) => panic!("unexpected result: {other:?}"),
        }
    }
}

#[tokio::test]
async fn official_demographics_return_only_the_requested_partial_buckets() {
    // Given: an age breakdown whose known buckets total 1030, not the authoritative 1234 followers.
    let account = UserId::new("17841400000000000");
    let (base, local_server) = server(Reply {
        status: "200 OK",
        body: include_str!("../../threads-ingest/tests/fixtures/audience_demographics_age.json"),
    })
    .await;

    // When: the provider fetches the official demographic insight.
    let result = provider(&base)
        .fetch_audience_insight(
            &account,
            AudienceInsightQuery::FollowerDemographics(DemographicDimension::Age),
        )
        .await;
    let request = local_server.await.expect("local server");

    // Then: the result has just its dimension and buckets; it cannot claim a follower total.
    assert!(matches!(
        result,
        Ok(AudienceInsightResult::Demographics(DemographicInsight {
            dimension: DemographicDimension::Age,
            buckets,
        })) if buckets.iter().map(|bucket| bucket.value).sum::<u64>() == 1030
    ));
    let url = request_url(&request);
    assert_eq!(
        url.query_pairs()
            .collect::<std::collections::HashMap<_, _>>()
            .get("breakdown"),
        Some(&"age".into())
    );
}

#[tokio::test]
async fn official_provider_rejects_posts_without_an_owner_or_username() {
    // Given: a documented envelope whose post omits both supported author identities.
    let (base, local_server) = server(Reply {
        status: "200 OK",
        body: r#"{"data":[{"id":"post-without-author"}]}"#,
    })
    .await;

    // When: the provider converts the response at its public boundary.
    let result = provider(&base)
        .fetch_mentions(&UserId::new("account"), None, 1)
        .await;
    let _request = local_server.await.expect("local server");

    // Then: no empty synthetic user ID can enter the core model.
    assert!(matches!(result, Err(Error::Parse(_))));
}

#[tokio::test]
async fn official_provider_encodes_hostile_user_ids_in_exact_versioned_paths() {
    // Given: a local official envelope and a hostile identifier.
    let (base, local_server) = server(Reply {
        status: "200 OK",
        body: include_str!("../../threads-ingest/tests/fixtures/mentions_terminal_page.json"),
    })
    .await;

    // When: the provider requests the documented mentions edge.
    let result = provider(&base)
        .fetch_mentions(&UserId::new("a b+c%/✓"), None, 1)
        .await;
    let request = local_server.await.expect("local server");

    // Then: the endpoint version remains fixed and the identifier stays one path segment.
    assert!(result.is_ok());
    assert!(request.starts_with("GET /v1.0/a%20b%2Bc%25%2F%E2%9C%93/mentions?"));
}
