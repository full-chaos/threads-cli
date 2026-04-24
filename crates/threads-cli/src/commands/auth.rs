use std::{fs, path::Path};

use anyhow::{anyhow, Context, Result};
use threads_provider_official::{
    auth::{self, CallbackServer, DEFAULT_SCOPES},
    token_store::{Token, TokenStore},
};
use tracing::info;

use crate::{cli::AuthCommand, config::CliConfig};

pub async fn run(cmd: AuthCommand, config_override: Option<&Path>) -> Result<()> {
    match cmd {
        AuthCommand::Login => login(config_override).await,
        AuthCommand::Status => status(),
        AuthCommand::Logout => logout(),
    }
}

async fn login(config_override: Option<&Path>) -> Result<()> {
    let cli_cfg = CliConfig::load(config_override)?;
    let mut provider_cfg = super::provider_config(&cli_cfg)?;

    // Bind a local callback listener and use its URI as the redirect.
    let server = CallbackServer::bind("/callback")
        .await
        .map_err(|e| anyhow!("bind local callback: {e}"))?;
    provider_cfg.redirect_uri = server.redirect_uri.clone();
    info!(uri = %server.redirect_uri, "OAuth callback listener ready");

    // Random state for CSRF protection.
    let state = random_state();
    let url = auth::authorize_url(&provider_cfg, DEFAULT_SCOPES, &state)
        .map_err(|e| anyhow!("build authorize URL: {e}"))?;

    println!("Opening browser to authorize threads-cli...");
    println!("If it does not open, visit this URL manually:");
    println!("  {url}");
    let _ = std::process::Command::new("open").arg(url.as_str()).status();

    let code = server
        .accept_code(&state)
        .await
        .map_err(|e| anyhow!("oauth callback: {e}"))?;

    let short = auth::exchange_code(&provider_cfg, &code)
        .await
        .map_err(|e| anyhow!("exchange code: {e}"))?;
    let long = auth::upgrade_to_long_lived(&provider_cfg, &short.access_token)
        .await
        .map_err(|e| anyhow!("upgrade to long-lived: {e}"))?;

    let token = Token::new(long.access_token, long.expires_in);
    TokenStore::new()
        .save(&token)
        .map_err(|e| anyhow!("save token: {e}"))?;

    println!("Authentication complete; token stored.");
    Ok(())
}

fn status() -> Result<()> {
    let token_path = CliConfig::token_path();
    let stored = TokenStore::new()
        .load()
        .map_err(|e| anyhow!("load token: {e}"))?;
    match stored {
        Some(t) => {
            println!("token stored (keyring or {})", token_path.display());
            println!("issued_at:   {}", t.issued_at);
            if let Some(exp) = t.expires_in {
                println!("expires_in:  {exp}s");
            }
            println!("expired:     {}", t.is_expired());
        }
        None => println!("no token; run `threads-cli auth login`"),
    }
    Ok(())
}

fn logout() -> Result<()> {
    TokenStore::new()
        .clear()
        .map_err(|e| anyhow!("clear token: {e}"))?;
    let path = CliConfig::token_path();
    if path.exists() {
        fs::remove_file(&path).context("removing token file")?;
    }
    println!("token cleared");
    Ok(())
}

/// 16-char base36 random-ish state from process time + counter. Not
/// cryptographically strong but adequate for OAuth state verification since
/// Meta re-posts it back to 127.0.0.1 on our socket.
fn random_state() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = n ^ c.wrapping_mul(0x9E3779B97F4A7C15);
    format!("{mixed:x}")
}
