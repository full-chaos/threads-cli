use serde::{Deserialize, Deserializer};
use threads_core::{Error, Result};
use url::Url;

use crate::config::Config;

const REFRESH_BASE: &str = "https://graph.threads.net/refresh_access_token";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_id_flex")]
    pub user_id: Option<String>,
}

fn deserialize_id_flex<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IdFormat {
        Str(String),
        I(i64),
        U(u64),
    }
    Ok(
        Option::<IdFormat>::deserialize(deserializer)?.map(|value| match value {
            IdFormat::Str(value) => value,
            IdFormat::I(value) => value.to_string(),
            IdFormat::U(value) => value.to_string(),
        }),
    )
}

pub(crate) async fn exchange_code_at(
    config: &Config,
    code: &str,
    endpoint: &Url,
) -> Result<TokenResponse> {
    let form = [
        ("client_id", config.app_id.as_str()),
        ("client_secret", config.app_secret.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("code", code),
    ];
    let response = oauth_client()?
        .post(endpoint.clone())
        .form(&form)
        .send()
        .await
        .map_err(|error| Error::Network(safe_network_error(error)))?;
    parse_token_response(response).await
}

pub(crate) async fn upgrade_to_long_lived_at(
    config: &Config,
    short_token: &str,
    endpoint: &Url,
) -> Result<TokenResponse> {
    let mut url = endpoint.clone();
    url.query_pairs_mut()
        .append_pair("grant_type", "th_exchange_token")
        .append_pair("client_secret", &config.app_secret)
        .append_pair("access_token", short_token);
    let response = oauth_client()?
        .get(url)
        .send()
        .await
        .map_err(|error| Error::Network(safe_network_error(error)))?;
    parse_token_response(response).await
}

pub async fn refresh_long_lived(_: &Config, token: &str) -> Result<TokenResponse> {
    let mut url = Url::parse(REFRESH_BASE)?;
    url.query_pairs_mut()
        .append_pair("grant_type", "th_refresh_token")
        .append_pair("access_token", token);
    let response = oauth_client()?
        .get(url)
        .send()
        .await
        .map_err(|error| Error::Network(safe_network_error(error)))?;
    parse_token_response(response).await
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenResponse> {
    let status = response.status();
    let raw_body = response
        .text()
        .await
        .map_err(|error| Error::Network(safe_network_error(error)))?;
    let safe_body = crate::redact::redact(&raw_body);
    if !status.is_success() {
        return Err(Error::Auth(format!("token endpoint {status}: {safe_body}")));
    }
    serde_json::from_str(&raw_body)
        .map_err(|error| Error::Parse(format!("token response: {error}; body: {safe_body}")))
}

fn oauth_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| Error::Network(safe_network_error(error)))
}

pub(crate) fn safe_network_error(error: reqwest::Error) -> String {
    error.without_url().to_string()
}
