use anyhow::{Result, anyhow};
use threads_provider_official::{
    auth::{self, DEFAULT_SCOPES},
    token_store::{Token, TokenStore},
};

#[cfg(test)]
pub(super) struct LoginCompletion<'a> {
    pub(super) provider_cfg: &'a threads_provider_official::Config,
    pub(super) code: &'a str,
    pub(super) token_store: &'a TokenStore,
    pub(super) endpoints: &'a auth::OAuthEndpoints,
}

pub(super) async fn finish_login(
    provider_cfg: &threads_provider_official::Config,
    code: &str,
) -> Result<()> {
    let token_store = TokenStore::new();
    let short = auth::exchange_code(provider_cfg, code)
        .await
        .map_err(|error| anyhow!("exchange code: {error}"))?;
    let long = auth::upgrade_to_long_lived(provider_cfg, &short.access_token)
        .await
        .map_err(|error| anyhow!("upgrade to long-lived: {error}"))?;
    save_token(&token_store, short, long)
}

#[cfg(test)]
pub(super) async fn finish_login_with(completion: LoginCompletion<'_>) -> Result<()> {
    let short = auth::exchange_code_with_endpoints(
        completion.provider_cfg,
        completion.code,
        completion.endpoints,
    )
    .await
    .map_err(|error| anyhow!("exchange code: {error}"))?;
    let long = auth::upgrade_to_long_lived_with_endpoints(
        completion.provider_cfg,
        &short.access_token,
        completion.endpoints,
    )
    .await
    .map_err(|error| anyhow!("upgrade to long-lived: {error}"))?;
    save_token(completion.token_store, short, long)
}

fn save_token(
    token_store: &TokenStore,
    short: auth::TokenResponse,
    long: auth::TokenResponse,
) -> Result<()> {
    let token = Token::new(
        long.access_token,
        long.expires_in,
        Some(
            DEFAULT_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
        ),
    )
    .with_user_id(long.user_id.or(short.user_id));
    token_store
        .save(&token)
        .map_err(|error| anyhow!("save token: {error}"))?;
    println!("Authentication complete; token stored with requested scope metadata.");
    Ok(())
}
