use std::time::Duration;

use threads_core::{Error, Result};
use tracing::{debug, warn};
use url::Url;

use super::retry::{backoff, is_near_limit, retry_after_delay};

#[derive(Clone, Copy)]
pub(super) enum Method {
    Get,
    Delete,
    Post,
}

impl Method {
    /// Status retries replay only HTTP methods whose requests are idempotent.
    const fn retries_status_errors(self) -> bool {
        match self {
            Self::Get | Self::Delete => true,
            Self::Post => false,
        }
    }
}

pub(super) async fn execute(
    client: &reqwest::Client,
    url: Url,
    method: Method,
    empty_is_null: bool,
) -> Result<serde_json::Value> {
    let mut attempt = 0_u32;
    let mut delay_ms = 250_u64;
    loop {
        attempt += 1;
        let response = request(client, method, url.clone()).await?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| retry_after_delay(Some(value)));
        let usage = response
            .headers()
            .get("x-app-usage")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if status.is_success() {
            let body = response.text().await.map_err(network_error)?;
            if usage.as_deref().is_some_and(is_near_limit) {
                warn!(usage = ?usage, "threads API near rate limit; client-side backoff");
            }
            if empty_is_null && body.trim().is_empty() {
                return Ok(serde_json::Value::Null);
            }
            return serde_json::from_str(&body).map_err(Error::from);
        }
        let body = crate::redact::redact(&response.text().await.unwrap_or_default());
        match status.as_u16() {
            401 => return Err(Error::Auth(format!("{status}: {body}"))),
            403 => return Err(Error::PermissionDenied(format!("{status}: {body}"))),
            404 => return Err(Error::NotFound(body)),
            429 if method.retries_status_errors() && attempt <= 5 => {
                wait(retry_after.unwrap_or_else(|| backoff(delay_ms)), attempt).await
            }
            429 => return Err(Error::RateLimit { retry_after }),
            status
                if (500..600).contains(&status)
                    && method.retries_status_errors()
                    && attempt <= 5 =>
            {
                tokio::time::sleep(retry_after.unwrap_or_else(|| backoff(delay_ms))).await
            }
            status if (500..600).contains(&status) => {
                return Err(Error::Network(format!("{status}: {body}")));
            }
            _ => return Err(Error::Other(format!("{status}: {body}"))),
        }
        delay_ms = (delay_ms * 2).min(30_000);
    }
}

async fn request(client: &reqwest::Client, method: Method, url: Url) -> Result<reqwest::Response> {
    let request = match method {
        Method::Get => client.get(url),
        Method::Delete => client.delete(url),
        Method::Post => client.post(url),
    };
    request.send().await.map_err(network_error)
}

fn network_error(error: reqwest::Error) -> Error {
    Error::Network(error.without_url().to_string())
}

async fn wait(delay: Duration, attempt: u32) {
    debug!(?delay, attempt, "rate limited, backing off");
    tokio::time::sleep(delay).await;
}
