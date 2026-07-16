use std::{io, path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, Utc};
use threads_core::{EngagementSort, UserId};
use threads_ingest::{Ingestor, OfficialNormalizer};
use threads_provider_official::TokenStore;
use threads_store::Store;

use crate::{
    cli::{
        AudienceCommand, AudienceEngagedArgs, AudiencePurgeArgs, AudienceShowArgs, AudienceSortArg,
    },
    output::{AudienceReport, OutputFormat, render_audience_report, render_engaged_accounts},
};

const AUDIENCE_SCOPES: [&str; 2] = ["threads_manage_insights", "threads_manage_mentions"];

pub async fn run(
    command: AudienceCommand,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    format: OutputFormat,
) -> Result<()> {
    match command {
        AudienceCommand::Refresh => refresh(config_override, db_override).await,
        AudienceCommand::Show(args) => show(args, config_override, db_override, format),
        AudienceCommand::Engaged(args) => engaged(args, config_override, db_override, format),
        AudienceCommand::Purge(args) => purge(args, config_override, db_override),
    }
}

async fn refresh(config_override: Option<&Path>, db_override: Option<&Path>) -> Result<()> {
    let token_store = TokenStore::new();
    let token = load_token(&token_store)?;
    crate::commands::require_recorded_scopes(&token, &AUDIENCE_SCOPES)?;

    let config = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&config, db_override)?;
    let provider = crate::commands::open_provider(&config).await?;
    let expected_account = token.user_id.as_deref().map(UserId::new);
    let summary = finish_refresh(
        &token_store,
        token,
        Ingestor::new(Arc::new(provider), Box::new(OfficialNormalizer), store)
            .refresh_audience_for_account(expected_account.as_ref())
            .await
            .map_err(|error| anyhow!("refresh audience: {error}")),
    )?;
    println!(
        "audience refreshed: account_id={} audience_count={} demographics={} mentions_ingested={}",
        summary.account_id,
        summary.followers_count,
        summary.demographics_count,
        summary.mentions_ingested
    );
    if let Some(warning) = summary.mention_warning {
        eprintln!("mentions warning: {warning:?}");
    }
    Ok(())
}

fn show(
    args: AudienceShowArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    format: OutputFormat,
) -> Result<()> {
    let token_store = TokenStore::new();
    let account_id = bound_account(&load_token(&token_store)?)?;
    let config = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&config, db_override)?;
    let mut writer = io::stdout().lock();
    render_show(&store, &account_id, args.history.get(), format, &mut writer)
}

fn engaged(
    args: AudienceEngagedArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    format: OutputFormat,
) -> Result<()> {
    let token_store = TokenStore::new();
    let account_id = bound_account(&load_token(&token_store)?)?;
    let config = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&config, db_override)?;
    let mut writer = io::stdout().lock();
    render_engaged(
        &store,
        &account_id,
        args.limit.get(),
        args.sort,
        format,
        &mut writer,
    )
}

fn purge(
    args: AudiencePurgeArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<()> {
    let cutoff = validated_before(&args.before)?;
    let token_store = TokenStore::new();
    let account_id = bound_account(&load_token(&token_store)?)?;
    let config = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&config, db_override)?;
    let mut writer = io::stdout().lock();
    purge_before(&store, &account_id, cutoff, args.apply, &mut writer)
}

fn load_token(token_store: &TokenStore) -> Result<threads_provider_official::token_store::Token> {
    let token = token_store
        .load()
        .map_err(|error| anyhow!("read token: {error}"))?;
    required_token(token)
}

fn required_token(
    token: Option<threads_provider_official::token_store::Token>,
) -> Result<threads_provider_official::token_store::Token> {
    token.ok_or_else(|| anyhow!("no stored access token; run `threads-cli auth login`"))
}

fn bound_account(token: &threads_provider_official::token_store::Token) -> Result<UserId> {
    token
        .user_id
        .as_deref()
        .map(UserId::new)
        .ok_or_else(|| anyhow!("token has no recorded account ID; run `threads-cli audience refresh` or `threads-cli auth login`"))
}

fn finish_refresh(
    token_store: &TokenStore,
    token: threads_provider_official::token_store::Token,
    refresh: Result<threads_ingest::AudienceRefreshSummary>,
) -> Result<threads_ingest::AudienceRefreshSummary> {
    let summary = refresh?;
    if token.user_id.is_none() {
        token_store
            .save(&token.with_user_id(Some(summary.account_id.to_string())))
            .map_err(|error| anyhow!("backfill authenticated account ID: {error}"))?;
    }
    Ok(summary)
}

fn render_show(
    store: &Store,
    account_id: &UserId,
    history: usize,
    format: OutputFormat,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let snapshots = store
        .audience_history(account_id, history)
        .map_err(|error| anyhow!("load audience history: {error}"))?;
    let report = AudienceReport::from_snapshots(&snapshots).map_err(|error| anyhow!(error))?;
    render_audience_report(&report, format, writer)
}

fn render_engaged(
    store: &Store,
    account_id: &UserId,
    limit: usize,
    sort: AudienceSortArg,
    format: OutputFormat,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let accounts = store
        .rank_engaged_accounts(account_id, limit, engagement_sort(sort))
        .map_err(|error| anyhow!("rank observed engagement: {error}"))?;
    render_engaged_accounts(&accounts, format, writer)
}

fn purge_before(
    store: &Store,
    account_id: &UserId,
    cutoff: DateTime<Utc>,
    apply: bool,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let count = store
        .count_audience_snapshots_before(account_id, cutoff)
        .map_err(|error| anyhow!("count audience observations: {error}"))?;
    if apply {
        let deleted = store
            .delete_audience_snapshots_before(account_id, cutoff)
            .map_err(|error| anyhow!("purge audience observations: {error}"))?;
        writeln!(writer, "purged: {deleted} audience observations")?;
    } else {
        writeln!(
            writer,
            "DRY RUN — would purge {count} audience observations before {}; run with --apply to remove them.",
            cutoff.to_rfc3339()
        )?;
    }
    Ok(())
}

fn engagement_sort(sort: AudienceSortArg) -> EngagementSort {
    match sort {
        AudienceSortArg::Total => EngagementSort::Total,
        AudienceSortArg::Replies => EngagementSort::Replies,
        AudienceSortArg::Mentions => EngagementSort::Mentions,
    }
}

fn parse_before(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").with_context(|| {
        format!("invalid --before value `{value}`; expected RFC 3339 or YYYY-MM-DD")
    })?;
    let timestamp = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
        anyhow!("invalid --before value `{value}`; expected RFC 3339 or YYYY-MM-DD")
    })?;
    Ok(DateTime::from_naive_utc_and_offset(timestamp, Utc))
}

fn validated_before(value: &str) -> Result<DateTime<Utc>> {
    let cutoff = parse_before(value)?;
    if cutoff > Utc::now() {
        bail!("--before must not be in the future");
    }
    Ok(cutoff)
}

#[cfg(test)]
#[path = "audience_tests.rs"]
mod tests;
