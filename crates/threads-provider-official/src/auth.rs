use std::collections::HashMap;

use serde::Deserialize;
use threads_core::{Error, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use crate::config::Config;

const AUTHORIZE_BASE: &str = "https://threads.net/oauth/authorize";
const TOKEN_EXCHANGE_BASE: &str = "https://graph.threads.net/oauth/access_token";
const ACCESS_TOKEN_BASE: &str = "https://graph.threads.net/access_token";
const REFRESH_BASE: &str = "https://graph.threads.net/refresh_access_token";

/// Default OAuth scopes covering read-only v1 MVP behavior.
pub const DEFAULT_SCOPES: &[&str] = &[
    "threads_basic",
    "threads_read_replies",
];

/// Build the URL the user must visit to grant authorization.
pub fn authorize_url(cfg: &Config, scopes: &[&str], state: &str) -> Result<Url> {
    let mut url = Url::parse(AUTHORIZE_BASE)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.app_id)
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("scope", &scopes.join(","))
        .append_pair("state", state);
    Ok(url)
}

#[derive(Clone, Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub user_id: Option<String>,
}

/// Exchange an authorization code for a short-lived access token.
pub async fn exchange_code(cfg: &Config, code: &str) -> Result<TokenResponse> {
    let client = reqwest::Client::new();
    let form = [
        ("client_id", cfg.app_id.as_str()),
        ("client_secret", cfg.app_secret.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("code", code),
    ];
    let resp = client
        .post(TOKEN_EXCHANGE_BASE)
        .form(&form)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    parse_token_response(resp).await
}

/// Upgrade a short-lived token to a long-lived one (60d).
pub async fn upgrade_to_long_lived(cfg: &Config, short_token: &str) -> Result<TokenResponse> {
    let mut url = Url::parse(ACCESS_TOKEN_BASE)?;
    url.query_pairs_mut()
        .append_pair("grant_type", "th_exchange_token")
        .append_pair("client_secret", &cfg.app_secret)
        .append_pair("access_token", short_token);
    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    parse_token_response(resp).await
}

/// Refresh a long-lived token (extends expiry by up to 60d).
pub async fn refresh_long_lived(_cfg: &Config, token: &str) -> Result<TokenResponse> {
    let mut url = Url::parse(REFRESH_BASE)?;
    url.query_pairs_mut()
        .append_pair("grant_type", "th_refresh_token")
        .append_pair("access_token", token);
    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    parse_token_response(resp).await
}

async fn parse_token_response(resp: reqwest::Response) -> Result<TokenResponse> {
    let status = resp.status();
    let body = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(Error::Auth(format!("token endpoint {status}: {body}")));
    }
    serde_json::from_str(&body)
        .map_err(|e| Error::Parse(format!("token response: {e}; body: {body}")))
}

/// Run a one-shot local HTTP server that receives Meta's redirect after the
/// user approves the authorization request.
///
/// - Binds to `127.0.0.1:0` (OS-assigned port) so multiple runs don't collide.
/// - Updates `cfg.redirect_uri` with the bound port; callers MUST pass that
///   updated value to [`authorize_url`] so state matches on redirect.
/// - Reads the first GET request, parses `?code=...&state=...`, validates
///   `state`, returns the code.
///
/// Returns the updated redirect URI (caller uses for authorize_url) together
/// with a future that resolves to the code on redirect.
pub struct CallbackServer {
    pub listener: TcpListener,
    pub redirect_uri: String,
}

impl CallbackServer {
    /// Bind to `127.0.0.1:0`, using the given `path` for the redirect URI
    /// (e.g., "/callback").
    pub async fn bind(path: &str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| Error::Network(format!("bind callback listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Network(format!("local_addr: {e}")))?
            .port();
        let p = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let redirect_uri = format!("http://127.0.0.1:{port}{p}");
        Ok(Self { listener, redirect_uri })
    }

    /// Accept a single HTTP request, parse the OAuth code and state.
    ///
    /// Validates that the received `state` matches `expected_state`; returns
    /// [`Error::Auth`] otherwise.
    pub async fn accept_code(self, expected_state: &str) -> Result<String> {
        let (mut sock, _) = self
            .listener
            .accept()
            .await
            .map_err(|e| Error::Network(format!("accept: {e}")))?;
        let mut buf = vec![0u8; 8192];
        let n = sock
            .read(&mut buf)
            .await
            .map_err(|e| Error::Network(format!("read: {e}")))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let (code, state) = parse_oauth_request(&req)?;
        if state != expected_state {
            let _ = respond_html(
                &mut sock,
                400,
                "<h1>State mismatch</h1><p>Authentication aborted.</p>",
            )
            .await;
            return Err(Error::Auth(format!(
                "OAuth state mismatch: got {state:?}, expected {expected_state:?}"
            )));
        }
        respond_html(
            &mut sock,
            200,
            "<h1>Authentication complete</h1><p>You may close this window.</p>",
        )
        .await
        .map_err(|e| Error::Network(format!("write response: {e}")))?;
        Ok(code)
    }
}

fn parse_oauth_request(raw: &str) -> Result<(String, String)> {
    let first = raw.lines().next().unwrap_or_default();
    // "GET /callback?code=...&state=... HTTP/1.1"
    let mut parts = first.split_whitespace();
    let _method = parts.next();
    let path_and_query = parts
        .next()
        .ok_or_else(|| Error::Parse("malformed oauth request line".into()))?;
    let (_path, query) = path_and_query
        .split_once('?')
        .ok_or_else(|| Error::Parse("missing query string".into()))?;
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .collect();
    let code = params
        .get("code")
        .ok_or_else(|| Error::Auth("missing `code` in redirect".into()))?
        .to_string();
    let state = params
        .get("state")
        .ok_or_else(|| Error::Auth("missing `state` in redirect".into()))?
        .to_string();
    Ok((percent_decode(&code), percent_decode(&state)))
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                16,
            ) {
                out.push(b as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

async fn respond_html(sock: &mut tokio::net::TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Unknown",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    );
    sock.write_all(resp.as_bytes()).await?;
    sock.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_contains_required_params() {
        let cfg = Config {
            app_id: "APP".into(),
            app_secret: "SEC".into(),
            redirect_uri: "https://localhost/cb".into(),
            access_token: None,
        };
        let url = authorize_url(&cfg, &["threads_basic", "threads_read_replies"], "xyz").unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "APP");
        assert_eq!(q["redirect_uri"], "https://localhost/cb");
        assert_eq!(q["scope"], "threads_basic,threads_read_replies");
        assert_eq!(q["state"], "xyz");
    }

    #[test]
    fn parse_oauth_request_extracts_code_and_state() {
        let req = "GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let (code, state) = parse_oauth_request(req).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz");
    }

    #[test]
    fn parse_oauth_request_rejects_missing_code() {
        let req = "GET /callback?state=xyz HTTP/1.1\r\n\r\n";
        let err = parse_oauth_request(req).unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a%2Bb"), "a+b");
        assert_eq!(percent_decode("x+y"), "x y");
    }

    #[tokio::test]
    async fn callback_server_binds_and_reports_uri() {
        let srv = CallbackServer::bind("/callback").await.unwrap();
        assert!(srv.redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(srv.redirect_uri.ends_with("/callback"));
    }
}
