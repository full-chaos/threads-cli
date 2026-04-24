use std::time::Duration;

use serde::de::DeserializeOwned;
use threads_core::{Error, Result};
use tracing::{debug, warn};
use url::Url;

/// Low-level HTTP client for `https://graph.threads.net`.
///
/// - Automatically appends `access_token` on every request.
/// - Retries on 429 with exponential backoff + jitter (cap 30s, max 5
///   attempts). Also backs off preemptively when `x-app-usage` reports >= 90%.
/// - Maps HTTP status codes to [`threads_core::Error`] variants.
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    base: Url,
    token: String,
}

impl HttpClient {
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<Self> {
        let base = Url::parse(base_url)?;
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| Error::Network(format!("reqwest client: {e}")))?;
        Ok(Self { inner, base, token: token.into() })
    }

    /// GET `path` (absolute or relative to `base`) returning `T`.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let value = self.get_json_value(path, query).await?;
        serde_json::from_value(value).map_err(Error::from)
    }

    /// GET returning the raw JSON `Value` (useful for normalizer retention).
    pub async fn get_json_value(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let mut url = if path.starts_with("http://") || path.starts_with("https://") {
            Url::parse(path)?
        } else {
            self.base.join(path)?
        };
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
            q.append_pair("access_token", &self.token);
        }

        let mut attempt = 0u32;
        let mut delay_ms = 250u64;
        loop {
            attempt += 1;
            let resp = self
                .inner
                .get(url.clone())
                .send()
                .await
                .map_err(|e| Error::Network(e.to_string()))?;
            let status = resp.status();
            let app_usage = resp
                .headers()
                .get("x-app-usage")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());

            if status.is_success() {
                let body = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
                if let Some(usage) = app_usage.as_deref() {
                    if is_near_limit(usage) {
                        warn!(usage, "threads API near rate limit; client-side backoff");
                    }
                }
                return serde_json::from_str(&body).map_err(Error::from);
            }

            let body = resp.text().await.unwrap_or_default();
            match status.as_u16() {
                401 | 403 => return Err(Error::Auth(format!("{status}: {body}"))),
                404 => return Err(Error::NotFound(body)),
                429 => {
                    if attempt > 5 {
                        return Err(Error::RateLimit {
                            retry_after: retry_after.map(Duration::from_secs),
                        });
                    }
                    let wait = retry_after
                        .map(Duration::from_secs)
                        .unwrap_or_else(|| backoff(delay_ms));
                    debug!(?wait, attempt, "rate limited, backing off");
                    tokio::time::sleep(wait).await;
                    delay_ms = (delay_ms * 2).min(30_000);
                }
                s if (500..600).contains(&s) => {
                    if attempt > 5 {
                        return Err(Error::Network(format!("{status}: {body}")));
                    }
                    tokio::time::sleep(backoff(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(30_000);
                }
                _ => return Err(Error::Other(format!("{status}: {body}"))),
            }
        }
    }
}

fn is_near_limit(x_app_usage: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(x_app_usage) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let Some(obj) = v.as_object() else { return false };
    obj.values()
        .filter_map(|n| n.as_f64())
        .any(|n| n >= 90.0)
}

fn backoff(base_ms: u64) -> Duration {
    let jitter = fastrand_like_jitter(base_ms);
    Duration::from_millis(base_ms + jitter)
}

// Cheap per-process jitter: xorshift on a process-local seed. Avoids adding
// the `rand` crate for just this one thing.
fn fastrand_like_jitter(base_ms: u64) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);
    let mut x = SEED.load(Ordering::Relaxed);
    if x == 0 {
        x = 0xDEADBEEFCAFEBABE;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    x % base_ms.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_limit_detects_high_percentage() {
        let json = r#"{"call_count":95.0,"total_time":12.3}"#;
        assert!(is_near_limit(json));
    }

    #[test]
    fn near_limit_false_when_low() {
        let json = r#"{"call_count":10.0,"total_time":2.0}"#;
        assert!(!is_near_limit(json));
    }

    #[test]
    fn backoff_is_bounded() {
        let d = backoff(250);
        assert!(d >= Duration::from_millis(250));
        assert!(d < Duration::from_millis(500));
    }
}
