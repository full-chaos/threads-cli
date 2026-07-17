use std::time::Duration;

use serde::de::DeserializeOwned;
use threads_core::{Error, Result};
use url::Url;

mod retry;
mod transport;

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    base: Url,
    token: String,
}

impl HttpClient {
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            inner: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| Error::Network(error.without_url().to_string()))?,
            base: Url::parse(base_url)?,
            token: token.into(),
        })
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        serde_json::from_value(self.get_json_value(path, query).await?).map_err(Error::from)
    }

    pub async fn get_json_value(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        self.execute(path, query, transport::Method::Get, false)
            .await
    }

    pub async fn delete_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        self.execute(path, query, transport::Method::Delete, true)
            .await
    }

    pub async fn post_json(&self, path: &str, query: &[(&str, &str)]) -> Result<serde_json::Value> {
        self.execute(path, query, transport::Method::Post, true)
            .await
    }

    async fn execute(
        &self,
        path: &str,
        query: &[(&str, &str)],
        method: transport::Method,
        empty_is_null: bool,
    ) -> Result<serde_json::Value> {
        let mut url = self.resolve_url(path)?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
            pairs.append_pair("access_token", &self.token);
        }
        transport::execute(&self.inner, url, method, empty_is_null).await
    }

    fn resolve_url(&self, path: &str) -> Result<Url> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            Url::parse(path)?
        } else {
            self.base.join(path)?
        };
        if url.origin() != self.base.origin() {
            return Err(Error::Config(
                "refusing to send an access token off-origin".into(),
            ));
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::retry::{backoff, is_near_limit, retry_after_delay};

    #[test]
    fn near_limit_detects_high_percentage() {
        assert!(is_near_limit(r#"{"call_count":95.0}"#));
    }

    #[test]
    fn near_limit_false_when_low() {
        assert!(!is_near_limit(r#"{"call_count":10.0}"#));
    }

    #[test]
    fn backoff_is_bounded() {
        let delay = backoff(250);
        assert!(delay >= Duration::from_millis(250));
        assert!(delay < Duration::from_millis(500));
    }

    #[test]
    fn retry_after_is_capped_to_the_documented_maximum() {
        assert_eq!(
            retry_after_delay(Some("999999")),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn rejects_off_origin_absolute_urls_before_attaching_a_token() {
        assert!(matches!(
            HttpClient::new("https://graph.threads.net", "secret")
                .unwrap()
                .resolve_url("https://example.com/redirect"),
            Err(Error::Config(_))
        ));
    }

    #[tokio::test]
    async fn post_json_sends_expected_method_query_and_access_token() {
        use tokio::{
            io::{AsyncReadExt as _, AsyncWriteExt as _},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 2048];
            let read = socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").await.unwrap();
            String::from_utf8(request[..read].to_vec()).unwrap()
        });
        let response = HttpClient::new(&format!("http://{address}"), "token")
            .unwrap()
            .post_json("/v1.0/me/threads", &[("text", "hello world")])
            .await
            .unwrap();
        assert_eq!(response["ok"], true);
        assert!(
            server
                .await
                .unwrap()
                .starts_with("POST /v1.0/me/threads?text=hello+world&access_token=token HTTP/1.1")
        );
    }

    async fn counted_error_server(status: u16) -> (String, tokio::task::JoinHandle<usize>) {
        use tokio::{
            io::{AsyncReadExt as _, AsyncWriteExt as _},
            net::TcpListener,
            time::timeout,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut count = 0;
            while let Ok(Ok((mut socket, _))) =
                timeout(Duration::from_millis(100), listener.accept()).await
            {
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).await.unwrap();
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} Error\r\nRetry-After: 0\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                count += 1;
            }
            count
        });
        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn get_retries_429_within_the_retry_bound() {
        let (base, server) = counted_error_server(429).await;

        let error = HttpClient::new(&base, "token")
            .unwrap()
            .get_json_value("/", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, Error::RateLimit { .. }));
        assert_eq!(server.await.unwrap(), 6);
    }

    #[tokio::test]
    async fn get_retries_5xx_within_the_retry_bound() {
        let (base, server) = counted_error_server(500).await;

        let error = HttpClient::new(&base, "token")
            .unwrap()
            .get_json_value("/", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, Error::Network(_)));
        assert_eq!(server.await.unwrap(), 6);
    }

    #[tokio::test]
    async fn delete_retries_429_within_the_retry_bound() {
        let (base, server) = counted_error_server(429).await;

        let error = HttpClient::new(&base, "token")
            .unwrap()
            .delete_json("/", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, Error::RateLimit { .. }));
        assert_eq!(server.await.unwrap(), 6);
    }

    #[tokio::test]
    async fn post_429_returns_rate_limit_after_one_request() {
        let (base, server) = counted_error_server(429).await;

        let error = HttpClient::new(&base, "token")
            .unwrap()
            .post_json("/", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, Error::RateLimit { .. }));
        assert_eq!(server.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn post_5xx_returns_network_error_after_one_request() {
        let (base, server) = counted_error_server(500).await;

        let error = HttpClient::new(&base, "token")
            .unwrap()
            .post_json("/", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, Error::Network(_)));
        assert_eq!(server.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn get_maps_401_to_auth_and_403_to_permission_denied() {
        async fn error_base(status: u16) -> (String, tokio::task::JoinHandle<()>) {
            use tokio::{io::AsyncWriteExt as _, net::TcpListener};

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                socket
                    .write_all(
                        format!("HTTP/1.1 {status} Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
            });
            (format!("http://{address}"), server)
        }

        let (base, server) = error_base(401).await;
        let error = HttpClient::new(&base, "token")
            .unwrap()
            .get_json_value("/", &[])
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(error, Error::Auth(_)));

        let (base, server) = error_base(403).await;
        let error = HttpClient::new(&base, "token")
            .unwrap()
            .get_json_value("/", &[])
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(error, Error::PermissionDenied(_)));
    }
}
