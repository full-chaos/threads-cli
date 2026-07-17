use std::{
    io::{self, Write as _},
    path::Path,
};

use anyhow::{Result, anyhow};
use rand::{TryRng, rngs::SysRng};
use threads_provider_official::{
    auth::{self, CallbackServer, DEFAULT_SCOPES},
    token_store::{Token, TokenStore},
};
use tracing::info;

use crate::{cli::AuthCommand, config::CliConfig};

const REQUESTED_SCOPE_PURPOSES: &[(&str, &str)] = &[
    ("threads_basic", "read your Threads account basics"),
    ("threads_read_replies", "read replies to your Threads posts"),
    ("threads_delete", "delete your Threads posts and replies"),
    ("threads_content_publish", "publish Threads posts"),
    (
        "threads_manage_insights",
        "read aggregate audience insights",
    ),
    (
        "threads_manage_mentions",
        "read posts that mention your account",
    ),
];

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

    let state = random_state()?;

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

    print_requested_scope_consent();
    println!("Opening browser to authorize threads-cli...");
    println!("If it does not open, visit this URL manually:");
    println!("  {url}");
    if let Err(error) = super::browser::open(url.as_str()) {
        eprintln!("could not open browser ({error}); visit the URL above manually");
    }

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

    print_requested_scope_consent();
    println!("1. Open this URL in your browser and approve the request:");
    println!("   {url}\n");
    println!(
        "2. After approval, Meta will redirect you to:\n   {}\n",
        provider_cfg.redirect_uri
    );
    println!(
        "3. Copy the full resulting redirect URL from the browser address bar and paste it here.\n"
    );

    if let Err(error) = super::browser::open(url.as_str()) {
        eprintln!("could not open browser ({error}); visit the URL above manually");
    }

    print!("Paste redirect URL: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let code = parse_code_from_input(input.trim(), state)?;
    finish_login(provider_cfg, &code).await
}

async fn finish_login(provider_cfg: &threads_provider_official::Config, code: &str) -> Result<()> {
    let short = auth::exchange_code(provider_cfg, code)
        .await
        .map_err(|e| anyhow!("exchange code: {e}"))?;
    let long = auth::upgrade_to_long_lived(provider_cfg, &short.access_token)
        .await
        .map_err(|e| anyhow!("upgrade to long-lived: {e}"))?;

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
    TokenStore::new()
        .save(&token)
        .map_err(|e| anyhow!("save token: {e}"))?;

    println!("Authentication complete; token stored with requested scope metadata.");
    Ok(())
}

fn parse_code_from_input(input: &str, expected_state: &str) -> Result<String> {
    let url = url::Url::parse(input).map_err(|_| anyhow!("paste the full redirect URL"))?;
    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }
    let code = code.ok_or_else(|| anyhow!("URL has no `code=...` parameter"))?;
    let state = state.ok_or_else(|| anyhow!("URL has no `state=...` parameter"))?;
    if state != expected_state {
        return Err(anyhow!(
            "OAuth state mismatch: got {state:?}, expected {expected_state:?} — aborting"
        ));
    }
    Ok(code)
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
            if let Some(user_id) = t.user_id.as_deref() {
                println!("user_id:     {user_id}");
            }
            if let Some(scopes) = t.requested_scopes.as_ref() {
                println!(
                    "requested_scopes (not confirmed by Meta): {}",
                    scopes.join(",")
                );
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
    println!("token cleared");
    Ok(())
}

/// Short random-ish state string for CSRF protection.
fn random_state() -> Result<String> {
    let mut bytes = [0_u8; 32];
    let mut rng = SysRng;
    rng.try_fill_bytes(&mut bytes)
        .map_err(|error| anyhow!("read operating-system entropy for OAuth state: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn requested_scope_consent_text() -> String {
    let scopes = REQUESTED_SCOPE_PURPOSES
        .iter()
        .map(|(scope, purpose)| format!("  - {scope}: {purpose}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "threads-cli will request these requested scopes (Meta does not return confirmed grants):\n{scopes}"
    )
}

fn print_requested_scope_consent() {
    println!("{}", requested_scope_consent_text());
}

#[cfg(test)]
mod tests {
    use super::*;

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
            parse_code_from_input("https://example.com/cb?code=AQx123&state=wrong", "state")
                .is_err()
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
}
