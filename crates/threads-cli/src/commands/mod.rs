use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use threads_manifest::Manifest;
use threads_provider_official::{
    Config as ProviderConfig, OfficialProvider, TokenStore,
    client::HttpClient,
    token_store::{Token, token_has_scope},
};
use threads_store::Store;

use crate::{
    cli::{Cli, Command},
    config::CliConfig,
};

pub mod audience;
pub mod auth;
pub mod browser;
pub mod delete;
pub mod export;
pub mod follow;
pub mod ingest;
pub mod init;
pub mod post;
pub mod search;
pub mod show;

/// Embedded manifest — v1 contract is compiled in so the binary doesn't need a
/// runtime file lookup. Bumping the Threads API contract is a PR that touches
/// this file.
const OFFICIAL_MANIFEST_TOML: &str = include_str!("../../../../manifests/official_v1.toml");

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init(args) => init::run(args, cli.config.as_deref()),
        Command::Auth(cmd) => auth::run(cmd, cli.config.as_deref()).await,
        Command::Follow(args) => follow::run(args),
        Command::Audience(cmd) => {
            audience::run(
                cmd,
                cli.config.as_deref(),
                cli.db.as_deref(),
                cli.format.into(),
            )
            .await
        }
        Command::Ingest(cmd) => ingest::run(cmd, cli.config.as_deref(), cli.db.as_deref()).await,
        Command::Show(args) => show::run(
            args,
            cli.config.as_deref(),
            cli.db.as_deref(),
            cli.format.into(),
        ),
        Command::Search(args) => search::run(
            args,
            cli.config.as_deref(),
            cli.db.as_deref(),
            cli.format.into(),
        ),
        Command::Export(args) => export::run(
            args,
            cli.config.as_deref(),
            cli.db.as_deref(),
            cli.format.into(),
        ),
        Command::Delete(cmd) => delete::run(cmd, cli.config.as_deref(), cli.db.as_deref()).await,
        Command::Post(cmd) => post::run_post(cmd, cli.config.as_deref(), cli.db.as_deref()).await,
    }
}

// ------------------------ shared helpers ------------------------

pub fn load_config(cli_override: Option<&std::path::Path>) -> Result<CliConfig> {
    CliConfig::load(cli_override).context("loading CLI config")
}

pub fn load_manifest() -> Result<Manifest> {
    Manifest::from_str(OFFICIAL_MANIFEST_TOML).map_err(|e| anyhow!("parse embedded manifest: {e}"))
}

pub fn open_store(cfg: &CliConfig, cli_override: Option<&std::path::Path>) -> Result<Arc<Store>> {
    let path = cli_override
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| cfg.db_path());
    let store = Store::open(&path).map_err(|e| anyhow!("open store at {}: {e}", path.display()))?;
    Ok(Arc::new(store))
}

pub async fn open_provider(_cfg: &CliConfig) -> Result<OfficialProvider> {
    let token = TokenStore::new()
        .load()
        .map_err(|e| anyhow!("read token: {e}"))?
        .ok_or_else(|| anyhow!("no stored access token; run `threads-cli auth login`"))?;
    let manifest = load_manifest()?;
    let base = manifest.api.base_url.clone();
    let http = HttpClient::new(&base, token.access_token)
        .map_err(|e| anyhow!("build http client: {e}"))?;
    Ok(OfficialProvider::new(http, manifest))
}

pub fn require_recorded_scopes(token: &Token, required_scopes: &[&str]) -> Result<()> {
    let missing_scopes = required_scopes
        .iter()
        .copied()
        .filter(|scope| !token_has_scope(token, scope))
        .collect::<Vec<_>>();
    if missing_scopes.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "stored token is missing recorded scope(s): {}; run `threads-cli auth login`",
        missing_scopes.join(", ")
    ))
}

pub fn provider_config(cfg: &CliConfig) -> Result<ProviderConfig> {
    Ok(ProviderConfig {
        app_id: cfg
            .app_id
            .clone()
            .ok_or_else(|| anyhow!("app_id not set — run `threads-cli init`"))?,
        app_secret: cfg
            .app_secret
            .clone()
            .ok_or_else(|| anyhow!("app_secret not set"))?,
        redirect_uri: cfg
            .redirect_uri
            .clone()
            .unwrap_or_else(|| "http://127.0.0.1:0/callback".to_string()),
        access_token: None,
    })
}

#[cfg(test)]
mod tests {
    use crate::cli::{FollowArgs, OutputFormatArg};

    use super::*;

    #[tokio::test]
    async fn follow_dispatch_requires_no_config_token_provider_or_store() {
        let cli = Cli {
            config: None,
            db: None,
            verbose: 0,
            format: OutputFormatArg::Human,
            command: Command::Follow(FollowArgs {
                username: "threads".to_string(),
                no_open: true,
            }),
        };

        assert!(dispatch(cli).await.is_ok());
    }

    #[test]
    fn scope_preflight_rejects_a_missing_recorded_scope_before_provider_creation() {
        let token = threads_provider_official::token_store::Token::new(
            "access-token",
            None,
            Some(vec!["threads_basic".to_string()]),
        );

        let error = require_recorded_scopes(&token, &["threads_manage_insights"]).unwrap_err();

        assert!(error.to_string().contains("threads_manage_insights"));
        assert!(error.to_string().contains("threads-cli auth login"));
    }
}
