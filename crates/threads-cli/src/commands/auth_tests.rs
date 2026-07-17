use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};

use super::*;

fn test_provider_config() -> threads_provider_official::Config {
    threads_provider_official::Config {
        app_id: "test-app".into(),
        app_secret: "test-secret".into(),
        redirect_uri: "https://example.test/callback".into(),
        access_token: None,
    }
}

async fn oauth_server_that_fails_on(
    failure_step: usize,
) -> (auth::OAuthEndpoints, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local OAuth test server");
    let address = listener
        .local_addr()
        .expect("read local OAuth test server address");
    let endpoints = auth::OAuthEndpoints::new(
        format!("http://{address}/exchange"),
        format!("http://{address}/upgrade"),
    )
    .expect("construct local OAuth endpoints");
    let server = tokio::spawn(async move {
        for step in 1..=failure_step {
            let (mut socket, _) = listener.accept().await.expect("accept OAuth request");
            let mut request = [0_u8; 4096];
            let bytes_read = socket.read(&mut request).await.expect("read OAuth request");
            assert!(bytes_read > 0, "OAuth client must send a request");
            let response = if step == failure_step {
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\nConnection: close\r\n\r\nfail"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 30\r\nConnection: close\r\n\r\n{\"access_token\":\"short-lived\"}"
            };
            socket
                .write_all(response.as_bytes())
                .await
                .expect("respond to OAuth request");
        }
    });
    (endpoints, server)
}

async fn assert_failed_completion_preserves_original_token_bytes(failure_step: usize) {
    let temporary = tempfile::TempDir::new().expect("create token fixture directory");
    let token_path = temporary.path().join("token.json");
    let original =
        b"{\n  \"access_token\": \"old-token\",\n  \"padding\": \"preserve these bytes\"\n}\n";
    std::fs::write(&token_path, original).expect("seed original token file");
    let store = TokenStore::new()
        .with_fallback_path(token_path.clone())
        .file_only_for_tests();
    let (endpoints, server) = oauth_server_that_fails_on(failure_step).await;

    let result = completion::finish_login_with(completion::LoginCompletion {
        provider_cfg: &test_provider_config(),
        code: "authorization-code",
        token_store: &store,
        endpoints: &endpoints,
    })
    .await;

    assert!(result.is_err(), "the simulated OAuth request must fail");
    server.await.expect("local OAuth server finishes");
    assert_eq!(
        std::fs::read(token_path).expect("read original token file"),
        original,
        "a failed reauthorization must not alter an existing token file"
    );
}

#[tokio::test]
async fn short_lived_exchange_failure_preserves_original_token_bytes_without_keychain() {
    assert_failed_completion_preserves_original_token_bytes(1).await;
}

#[tokio::test]
async fn long_lived_upgrade_failure_preserves_original_token_bytes_without_keychain() {
    assert_failed_completion_preserves_original_token_bytes(2).await;
}

#[test]
fn parses_full_redirect_url() {
    let code = parse_code_from_input(
        "https://example.com/cb?code=AQx123&state=abc&extra=1",
        "abc",
    )
    .unwrap();
    assert_eq!(code, "AQx123");
}

#[test]
fn rejects_bare_code() {
    assert!(parse_code_from_input("AQx123", "state").is_err());
}

#[test]
fn rejects_empty_input() {
    assert!(parse_code_from_input("", "state").is_err());
}

#[test]
fn rejects_url_without_code() {
    assert!(parse_code_from_input("https://example.com/cb?foo=bar", "state").is_err());
}

#[test]
fn rejects_redirect_without_state() {
    assert!(parse_code_from_input("https://example.com/cb?code=AQx123", "state").is_err());
}

#[test]
fn rejects_redirect_with_a_mismatched_state() {
    assert!(
        parse_code_from_input("https://example.com/cb?code=AQx123&state=wrong", "state").is_err()
    );
}

#[test]
fn consent_text_names_every_requested_scope_and_audience_purpose() {
    let consent = requested_scope_consent_text();

    for scope in DEFAULT_SCOPES {
        assert!(consent.contains(scope), "missing {scope} from consent text");
    }
    assert!(consent.contains("audience insights"));
    assert!(consent.contains("mentions"));
    assert!(consent.contains("requested scopes"));
}

#[test]
fn oauth_state_uses_full_entropy_and_is_not_reused() {
    let first = random_state().unwrap();
    let second = random_state().unwrap();

    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
    assert_ne!(first, second);
}
