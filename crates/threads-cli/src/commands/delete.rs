use std::{
    io::{self, IsTerminal},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, Utc};
use threads_core::{Error as CoreError, Post, Provider};
use threads_provider_official::{TokenStore, token_store::token_has_scope};
use threads_store::PostKind;

use crate::cli::{DeleteArgs, DeleteCommand};

pub async fn run(
    cmd: DeleteCommand,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<()> {
    match cmd {
        DeleteCommand::Posts(args) => {
            run_kind(args, config_override, db_override, PostKind::Post).await
        }
        DeleteCommand::Replies(args) => {
            run_kind(args, config_override, db_override, PostKind::Reply).await
        }
    }
}

async fn run_kind(
    args: DeleteArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    kind: PostKind,
) -> Result<()> {
    if args.before.is_none() && args.after.is_none() {
        bail!("refusing to delete without a time window: pass --before, --after, or both");
    }

    let before = args.before.as_deref().map(parse_time).transpose()?;
    let after = args.after.as_deref().map(parse_time).transpose()?;
    let before_str = window_bound(before, "+∞");
    let after_str = window_bound(after, "-∞");

    let token = TokenStore::new()
        .load()
        .map_err(|e| anyhow!("read token: {e}"))?;
    match token {
        Some(token) if token_has_scope(&token, "threads_delete") => {}
        Some(_) => bail!("stored token lacks `threads_delete` scope; run `threads-cli auth login`"),
        None => bail!("no stored token; run `threads-cli auth login`"),
    }

    let cli_cfg = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;
    let provider = crate::commands::open_provider(&cli_cfg).await?;
    let me = provider
        .fetch_me()
        .await
        .map_err(|e| anyhow!("fetch authenticated user: {e}"))?;

    let candidate_limit = args.limit.unwrap_or(usize::MAX);
    let candidates = store
        .posts_in_window(&me.id, after, before, kind, candidate_limit)
        .map_err(|e| anyhow!("query delete candidates: {e}"))?;

    let noun = noun(kind);
    if candidates.is_empty() {
        println!("no candidates in window {after_str} → {before_str}");
        return Ok(());
    }

    print_dry_run(&candidates, noun, &after_str, &before_str);
    if !args.apply {
        return Ok(());
    }

    if kind == PostKind::Reply && !args.yes_undocumented {
        confirm_undocumented_replies()?;
    }

    let already = store
        .deletions_in_last_24h()
        .map_err(|e| anyhow!("count recent deletions: {e}"))?;
    if already >= 100 {
        let resets_at = store
            .oldest_deletion_in_last_24h()
            .map_err(|e| anyhow!("read oldest recent deletion: {e}"))?
            .map(|dt| (dt + chrono::Duration::hours(24)).to_rfc3339())
            .unwrap_or_else(|| "<unknown>".to_string());
        bail!(
            "rate limit reached: {already}/100 successful deletions in the last 24h. Quota resets at {resets_at}"
        );
    }

    let remaining_quota = 100 - already;
    let total = candidates
        .len()
        .min(candidate_limit)
        .min(remaining_quota as usize);
    let mut deleted_count = 0usize;
    let mut failed_count = 0usize;

    for (idx, post) in candidates.iter().take(total).enumerate() {
        if idx > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let result = match kind {
            PostKind::Post => provider.delete_post(&post.id).await,
            PostKind::Reply => provider.delete_reply(&post.id).await,
        };

        match result {
            Ok(()) => {
                store
                    .delete_post(&post.id)
                    .map_err(|e| anyhow!("delete local post {}: {e}", post.id))?;
                store
                    .record_deletion(&post.id, kind, true, None)
                    .map_err(|e| anyhow!("record deletion {}: {e}", post.id))?;
                deleted_count += 1;
            }
            Err(CoreError::RateLimit { .. }) => {
                eprintln!(
                    "rate limited by Threads API; stopping batch (deleted {deleted_count}/{total} so far)"
                );
                break;
            }
            Err(err) => {
                eprintln!("failed to delete {}: {err}", post.id);
                store
                    .record_deletion(&post.id, kind, false, Some(&err.to_string()))
                    .map_err(|e| anyhow!("record deletion failure {}: {e}", post.id))?;
                failed_count += 1;
            }
        }
    }

    println!("deleted: {deleted_count}");
    println!("failed:  {failed_count}");
    println!(
        "remaining_quota_24h: {}",
        remaining_quota.saturating_sub(deleted_count as u64)
    );
    Ok(())
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }

    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").with_context(|| {
        format!("invalid time window value `{value}`; expected RFC 3339 or YYYY-MM-DD")
    })?;
    let naive = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
        anyhow!("invalid time window value `{value}`; expected RFC 3339 or YYYY-MM-DD")
    })?;
    Ok(DateTime::from_naive_utc_and_offset(naive, Utc))
}

fn window_bound(value: Option<DateTime<Utc>>, fallback: &str) -> String {
    // Open-ended windows are displayed explicitly so the dry-run remains auditable.
    value
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| fallback.to_string())
}

fn noun(kind: PostKind) -> &'static str {
    match kind {
        PostKind::Post => "posts",
        PostKind::Reply => "replies",
    }
}

fn print_dry_run(candidates: &[Post], noun: &str, after: &str, before: &str) {
    println!(
        "DRY RUN — would delete {} {noun} authored between {after} and {before}:",
        candidates.len()
    );
    for post in candidates.iter().take(10) {
        let created_at = post
            .created_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "<unknown>".to_string());
        println!(
            "  {}  {}  {}",
            post.id,
            created_at,
            snippet(post.text.as_deref())
        );
    }
    if candidates.len() > 10 {
        println!("  ... and {} more", candidates.len() - 10);
    }
    println!("Run with --apply to actually delete.");
    println!("Note: Threads API enforces a hard cap of 100 deletions per 24h.");
}

fn snippet(text: Option<&str>) -> String {
    text.unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(60)
        .collect()
}

fn confirm_undocumented_replies() -> Result<()> {
    println!(
        "This endpoint is not officially documented for replies. Verify on a single test reply before deleting in bulk."
    );
    if !io::stdin().is_terminal() {
        bail!("--yes-undocumented required when not on a TTY");
    }

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => bail!("delete replies confirmation declined"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_accepts_rfc3339_and_bare_dates() {
        assert!(parse_time("2025-01-15").is_ok());
        assert!(parse_time("2025-01-15T12:30:00Z").is_ok());
        assert!(parse_time("2025-01-15T12:30:00+00:00").is_ok());
    }

    #[test]
    fn parse_time_rejects_invalid_values() {
        assert!(parse_time("garbage").is_err());
        assert!(parse_time("2025-13-99").is_err());
        assert!(parse_time("").is_err());
    }
}
