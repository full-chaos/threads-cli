use chrono::{Duration, TimeZone};
use tempfile::TempDir;
use threads_core::{AudienceSnapshot, UserId};
use threads_ingest::AudienceRefreshSummary;
use threads_provider_official::token_store::Token;

use super::*;

fn snapshot(
    account_id: &str,
    observed_at: chrono::DateTime<Utc>,
    audience_count: u64,
) -> AudienceSnapshot {
    AudienceSnapshot {
        account_id: UserId::new(account_id),
        observed_at,
        followers_count: audience_count,
        demographics: Vec::new(),
    }
}

#[test]
fn parse_before_accepts_rfc3339_and_bare_dates() {
    assert!(parse_before("2026-01-15").is_ok());
    assert!(parse_before("2026-01-15T12:30:00Z").is_ok());
}

#[test]
fn parse_before_rejects_malformed_values() {
    assert!(parse_before("not-a-date").is_err());
    assert!(parse_before("2026-13-15").is_err());
}

#[test]
fn validated_before_rejects_future_cutoffs_before_opening_local_state() {
    let future = (Utc::now() + Duration::days(1)).to_rfc3339();

    let error = validated_before(&future).expect_err("future cutoff must be rejected");

    assert!(error.to_string().contains("must not be in the future"));
}

#[test]
fn required_token_rejects_missing_token_before_any_provider_can_be_constructed() {
    let error = required_token(None).expect_err("missing token must fail preflight");

    assert!(error.to_string().contains("auth login"));
}

#[test]
fn audience_refresh_preflight_requires_both_recorded_scopes() {
    let token = Token::new("token", None, Some(vec!["threads_manage_insights".into()]));

    let error = crate::commands::require_recorded_scopes(&token, &AUDIENCE_SCOPES)
        .expect_err("missing mentions scope must block refresh before network setup");

    assert!(error.to_string().contains("threads_manage_mentions"));
}

#[test]
fn failed_refresh_does_not_backfill_legacy_token_metadata() {
    let temporary = TempDir::new().expect("temporary token directory");
    let path = temporary.path().join("token.json");
    let token_store = TokenStore::new().with_fallback_path(path.clone());
    let token = Token::new("token", None, Some(vec!["threads_manage_insights".into()]));

    let result = finish_refresh(
        &token_store,
        token,
        Err(anyhow::anyhow!("remote refresh failed")),
    );

    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn successful_refresh_backfills_a_legacy_token_account_id() {
    let temporary = TempDir::new().expect("temporary token directory");
    let path = temporary.path().join("token.json");
    let token_store = TokenStore::new().with_fallback_path(path.clone());
    let token = Token::new("token", None, Some(vec!["threads_manage_insights".into()]));
    let summary = AudienceRefreshSummary {
        account_id: UserId::new("account-a"),
        followers_count: 10,
        demographics_count: 0,
        mentions_ingested: 0,
        mention_warning: None,
    };

    finish_refresh(&token_store, token, Ok(summary)).expect("successful refresh finishes");

    let saved: Token = serde_json::from_slice(&std::fs::read(path).expect("saved fallback token"))
        .expect("valid saved token");
    assert_eq!(saved.user_id.as_deref(), Some("account-a"));
}

#[test]
fn bound_account_requires_a_refresh_or_relogin_for_legacy_tokens() {
    let token = Token::new("token", None, None);

    let error =
        bound_account(&token).expect_err("legacy token must not access local audience data");

    assert!(error.to_string().contains("audience refresh"));
}

#[test]
fn show_renders_only_the_token_bound_account() {
    let store = Store::open_in_memory().expect("in-memory store");
    let first = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("timestamp");
    store
        .upsert_audience_snapshot(&snapshot("account-a", first, 10), None)
        .expect("seed account a");
    store
        .upsert_audience_snapshot(&snapshot("account-b", first + Duration::days(1), 99), None)
        .expect("seed account b");
    let mut output = Vec::new();

    render_show(
        &store,
        &UserId::new("account-a"),
        10,
        OutputFormat::Json,
        &mut output,
    )
    .expect("render token-bound history");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(value["account_id"], "account-a");
    assert_eq!(value["snapshots"].as_array().map(Vec::len), Some(1));
}

#[test]
fn show_routes_every_supported_format_without_network_access() {
    let store = Store::open_in_memory().expect("in-memory store");
    let observed_at = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("timestamp");
    store
        .upsert_audience_snapshot(&snapshot("account-a", observed_at, 10), None)
        .expect("seed account");

    for format in [
        OutputFormat::Human,
        OutputFormat::Json,
        OutputFormat::Jsonl,
        OutputFormat::Csv,
    ] {
        let mut output = Vec::new();
        render_show(&store, &UserId::new("account-a"), 10, format, &mut output)
            .expect("local format render succeeds");
        assert!(!output.is_empty());
    }
}

#[test]
fn show_and_engaged_render_empty_local_results_successfully() {
    let store = Store::open_in_memory().expect("in-memory store");
    let account_id = UserId::new("account-a");
    let mut show_output = Vec::new();
    let mut engaged_output = Vec::new();

    render_show(
        &store,
        &account_id,
        10,
        OutputFormat::Human,
        &mut show_output,
    )
    .expect("empty show succeeds");
    render_engaged(
        &store,
        &account_id,
        20,
        AudienceSortArg::Total,
        OutputFormat::Human,
        &mut engaged_output,
    )
    .expect("empty engaged succeeds");

    assert!(
        String::from_utf8(show_output)
            .expect("utf-8")
            .contains("No audience observations")
    );
    assert!(
        String::from_utf8(engaged_output)
            .expect("utf-8")
            .contains("No observed engagement")
    );
}

#[test]
fn purge_is_dry_by_default_and_scoped_to_the_token_account() {
    let store = Store::open_in_memory().expect("in-memory store");
    let cutoff = Utc
        .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
        .single()
        .expect("timestamp");
    store
        .upsert_audience_snapshot(&snapshot("account-a", cutoff - Duration::days(1), 10), None)
        .expect("seed account a");
    store
        .upsert_audience_snapshot(&snapshot("account-b", cutoff - Duration::days(1), 20), None)
        .expect("seed account b");
    let mut dry_output = Vec::new();

    purge_before(
        &store,
        &UserId::new("account-a"),
        cutoff,
        false,
        &mut dry_output,
    )
    .expect("dry run succeeds");

    assert!(
        String::from_utf8(dry_output)
            .expect("utf-8")
            .contains("would purge 1")
    );
    assert_eq!(
        store
            .audience_history(&UserId::new("account-a"), 10)
            .expect("history")
            .len(),
        1
    );
    assert_eq!(
        store
            .audience_history(&UserId::new("account-b"), 10)
            .expect("history")
            .len(),
        1
    );
}

#[test]
fn purge_apply_removes_only_matching_token_bound_observations() {
    let store = Store::open_in_memory().expect("in-memory store");
    let cutoff = Utc
        .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
        .single()
        .expect("timestamp");
    store
        .upsert_audience_snapshot(&snapshot("account-a", cutoff - Duration::days(1), 10), None)
        .expect("seed account a");
    store
        .upsert_audience_snapshot(&snapshot("account-b", cutoff - Duration::days(1), 20), None)
        .expect("seed account b");
    let mut output = Vec::new();

    purge_before(&store, &UserId::new("account-a"), cutoff, true, &mut output)
        .expect("apply succeeds");

    assert!(
        String::from_utf8(output)
            .expect("utf-8")
            .contains("purged: 1")
    );
    assert!(
        store
            .audience_history(&UserId::new("account-a"), 10)
            .expect("history")
            .is_empty()
    );
    assert_eq!(
        store
            .audience_history(&UserId::new("account-b"), 10)
            .expect("history")
            .len(),
        1
    );
}
