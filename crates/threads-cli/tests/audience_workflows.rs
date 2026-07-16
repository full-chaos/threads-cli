use std::process::Command;

use tempfile::TempDir;
use threads_core::UserId;
use threads_store::Store;

mod support;

use support::{Harness, assert_success};

#[test]
fn audience_local_workflows_render_only_the_token_bound_account_in_every_format() {
    // Given: an isolated database with two accounts, decreasing snapshots, nested replies, and mention-target edges.
    let harness = Harness::new();

    // When: every local audience surface is invoked through the compiled CLI binary.
    for format in ["human", "json", "jsonl", "csv"] {
        let show = harness.run(&[
            "--db",
            harness.db.to_str().expect("db path"),
            "--format",
            format,
            "audience",
            "show",
        ]);
        assert_success(&show);
        let output = String::from_utf8(show.stdout).expect("UTF-8 show output");
        assert!(output.contains("account-a"));
        assert!(!output.contains("account-b"));
        let engaged = harness.run(&[
            "--db",
            harness.db.to_str().expect("db path"),
            "--format",
            format,
            "audience",
            "engaged",
        ]);
        assert_success(&engaged);
        let engaged_output = String::from_utf8(engaged.stdout).expect("UTF-8 engaged output");
        assert!(engaged_output.contains("engaged-a"));
        assert!(
            !engaged_output.contains("outsider"),
            "account-b identity leaked into account-a {format} engagement output"
        );
    }
    let export = harness.run(&[
        "--db",
        harness.db.to_str().expect("db path"),
        "--format",
        "json",
        "export",
    ]);

    // Then: account isolation holds and post export never gains audience fields.
    assert_success(&export);
    let exported: serde_json::Value = serde_json::from_slice(&export.stdout).expect("JSON export");
    assert!(exported.to_string().contains("root-a"));
    assert!(!exported.to_string().contains("followers_count"));
}

#[test]
fn audience_purge_is_dry_by_default_then_applies_only_to_the_bound_account() {
    // Given: the isolated two-account fixture and an old cutoff.
    let harness = Harness::new();
    let args = [
        "--db",
        harness.db.to_str().expect("db path"),
        "audience",
        "purge",
        "--before",
        "2025-02-01",
    ];

    // When: dry-run, apply, and an interrupted-style repeat are executed as separate processes.
    let dry_run = harness.run(&args);
    assert_success(&dry_run);
    assert!(
        String::from_utf8(dry_run.stdout)
            .expect("UTF-8 dry output")
            .contains("would purge 1")
    );
    let applied = harness.run(&[
        "--db",
        harness.db.to_str().expect("db path"),
        "audience",
        "purge",
        "--before",
        "2025-02-01",
        "--apply",
    ]);
    assert_success(&applied);
    let repeated = harness.run(&[
        "--db",
        harness.db.to_str().expect("db path"),
        "audience",
        "purge",
        "--before",
        "2025-02-01",
        "--apply",
    ]);

    // Then: the first apply removes one bound row, repeat is harmless, and account-b remains intact.
    assert_success(&repeated);
    assert!(
        String::from_utf8(repeated.stdout)
            .expect("UTF-8 repeat output")
            .contains("purged: 0")
    );
    let store = Store::open(&harness.db).expect("reopen store");
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
fn audience_preflight_failures_exit_nonzero_without_contacting_a_provider() {
    // Given: isolated fallback-token states that cannot authorize local or remote audience operations.
    let harness = Harness::new();
    harness.write_token(
        None,
        &["threads_manage_insights", "threads_manage_mentions"],
    );

    // When: the local command lacks a bound account and refresh lacks a recorded scope.
    let no_account = harness.run(&[
        "--db",
        harness.db.to_str().expect("db path"),
        "audience",
        "show",
    ]);
    harness.write_token(Some("account-a"), &["threads_manage_insights"]);
    let missing_scope = harness.run(&[
        "--db",
        harness.db.to_str().expect("db path"),
        "audience",
        "refresh",
    ]);

    // Then: both commands fail before an official endpoint can be constructed.
    assert!(!no_account.status.success());
    assert!(String::from_utf8_lossy(&no_account.stderr).contains("no recorded account ID"));
    assert!(!missing_scope.status.success());
    assert!(String::from_utf8_lossy(&missing_scope.stderr).contains("threads_manage_mentions"));
}

#[test]
fn follow_without_opening_a_browser_requires_no_local_credentials() {
    // Given: a fresh state directory with no config, database, or token.
    let state = TempDir::new().expect("fresh state");

    // When: follow is run with browser opening disabled.
    let output = Command::new(env!("CARGO_BIN_EXE_threads-cli"))
        .args(["follow", "@threads", "--no-open"])
        .env("HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
        .output()
        .expect("run follow");

    // Then: it succeeds locally and emits the official intent URL without network access.
    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 URL"),
        "https://www.threads.com/intent/follow?username=threads\n"
    );
}
