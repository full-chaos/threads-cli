use std::{
    fs,
    io::{self, Write as _},
    path::Path,
};

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

    // Meta blocks http:// redirects on the Threads product ("Insecure Login
    // Blocked", error 1349187). We pick a flow based on the configured URI:
    //
    //   - http://127.0.0.1 or http://localhost -> local listener (works for
    //     other OAuth2 providers and for future-proofing if Meta ever relaxes)
    //   - anything else (e.g. the user's registered https:// URI)
    //     -> manual paste mode
    let is_loopback_http = provider_cfg.redirect_uri.starts_with("http://127.0.0.1")
        || provider_cfg.redirect_uri.starts_with("http://localhost");

    let state = random_state();

    if is_loopback_http {
        login_local_listener(&mut provider_cfg, &state).await
    } else {
        login_manual_paste(&provider_cfg, &state).await
    }
}

async fn login_local_listener(
    provider_cfg: &mut threads_provider_official::Config,
    state: &str,
) -> Result<()> {
    // Bind to the EXACT host+port of the configured URI so it byte-matches
    // what was registered in the app dashboard. Meta rejects any mismatch.
    // If the URI lacks a port, fall back to OS-assigned — but warn, since
    // the provider must have whitelisted that generated URI somehow.
    let has_port = url::Url::parse(&provider_cfg.redirect_uri)
        .ok()
        .and_then(|u| u.port())
        .is_some();
    let server = if has_port {
        CallbackServer::bind_to_uri(&provider_cfg.redirect_uri)
            .await
            .map_err(|e| anyhow!("bind local callback: {e}"))?
    } else {
        eprintln!(
            "warning: redirect_uri {} has no port; binding to an OS-assigned port.\n\
             This will only work if the provider treats the loopback URI as \
             port-agnostic (most do, Meta does not).",
            provider_cfg.redirect_uri
        );
        let s = CallbackServer::bind("/callback")
            .await
            .map_err(|e| anyhow!("bind local callback: {e}"))?;
        provider_cfg.redirect_uri = s.redirect_uri.clone();
        s
    };
    info!(uri = %server.redirect_uri, "OAuth callback listener ready");

    let url = auth::authorize_url(provider_cfg, DEFAULT_SCOPES, state)
        .map_err(|e| anyhow!("build authorize URL: {e}"))?;

    println!("Opening browser to authorize threads-cli...");
    println!("If it does not open, visit this URL manually:");
    println!("  {url}");
    let _ = std::process::Command::new("open").arg(url.as_str()).status();

    let code = server
        .accept_code(state)
        .await
        .map_err(|e| anyhow!("oauth callback: {e}"))?;

    finish_login(provider_cfg, &code).await
}

async fn login_manual_paste(
    provider_cfg: &threads_provider_official::Config,
    state: &str,
) -> Result<()> {
    let url = auth::authorize_url(provider_cfg, DEFAULT_SCOPES, state)
        .map_err(|e| anyhow!("build authorize URL: {e}"))?;

    println!("1. Open this URL in your browser and approve the request:");
    println!("   {url}\n");
    println!(
        "2. After approval, Meta will redirect you to:\n   {}\n",
        provider_cfg.redirect_uri
    );
    println!(
        "3. Copy the resulting URL (or just the `code=...` parameter) from the browser's\n\
         address bar and paste it here. (State to match: {state})\n"
    );

    let _ = std::process::Command::new("open").arg(url.as_str()).status();

    print!("Paste URL or code: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let (code, returned_state) = parse_code_from_input(input.trim())?;
    if let Some(rs) = returned_state {
        if rs != state {
            return Err(anyhow!(
                "OAuth state mismatch: got {rs:?}, expected {state:?} — aborting"
            ));
        }
    }
    finish_login(provider_cfg, &code).await
}

async fn finish_login(
    provider_cfg: &threads_provider_official::Config,
    code: &str,
) -> Result<()> {
    let short = auth::exchange_code(provider_cfg, code)
        .await
        .map_err(|e| anyhow!("exchange code: {e}"))?;
    let long = auth::upgrade_to_long_lived(provider_cfg, &short.access_token)
        .await
        .map_err(|e| anyhow!("upgrade to long-lived: {e}"))?;

    let token = Token::new(long.access_token, long.expires_in);
    TokenStore::new()
        .save(&token)
        .map_err(|e| anyhow!("save token: {e}"))?;

    println!("Authentication complete; token stored.");
    Ok(())
}

/// Accept either a bare code (`AQxxxxx...`) or a full URL with
/// `?code=...&state=...` query params. Returns `(code, Option<state>)`.
fn parse_code_from_input(input: &str) -> Result<(String, Option<String>)> {
    if let Ok(url) = url::Url::parse(input) {
        let mut code = None;
        let mut state = None;
        for (k, v) in url.query_pairs() {
            if k == "code" {
                code = Some(v.into_owned());
            } else if k == "state" {
                state = Some(v.into_owned());
            }
        }
        let code = code.ok_or_else(|| anyhow!("URL has no `code=...` parameter"))?;
        return Ok((code, state));
    }
    if input.is_empty() {
        return Err(anyhow!("empty input"));
    }
    Ok((input.to_string(), None))
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

/// Short random-ish state string for CSRF protection.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_redirect_url() {
        let (code, state) = parse_code_from_input(
            "https://example.com/cb?code=AQx123&state=abc&extra=1",
        )
        .unwrap();
        assert_eq!(code, "AQx123");
        assert_eq!(state.as_deref(), Some("abc"));
    }

    #[test]
    fn parses_bare_code() {
        let (code, state) = parse_code_from_input("AQx123").unwrap();
        assert_eq!(code, "AQx123");
        assert!(state.is_none());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_code_from_input("").is_err());
    }

    #[test]
    fn rejects_url_without_code() {
        assert!(parse_code_from_input("https://example.com/cb?foo=bar").is_err());
    }
}
