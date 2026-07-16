use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use chrono::{TimeZone, Utc};
use serde_json::json;
use tempfile::TempDir;
use threads_core::{
    AudienceSnapshot, DemographicBucket, DemographicDimension, Mention, Post, PostId, UserId,
};
use threads_store::Store;

pub(crate) struct Harness {
    pub(crate) state: TempDir,
    pub(crate) db: PathBuf,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let state = TempDir::new().expect("temporary state");
        let db = state.path().join("store.db");
        let harness = Self { state, db };
        harness.write_token(
            Some("account-a"),
            &["threads_manage_insights", "threads_manage_mentions"],
        );
        harness.seed();
        harness
    }

    fn token_path(&self) -> PathBuf {
        self.state.path().join("config/threads-cli/token.json")
    }

    pub(crate) fn write_token(&self, account_id: Option<&str>, scopes: &[&str]) {
        let path = self.token_path();
        fs::create_dir_all(path.parent().expect("token parent")).expect("create token directory");
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "access_token": "test-only-token",
                "requested_scopes": scopes,
                "user_id": account_id,
                "issued_at": "2026-01-01T00:00:00Z"
            }))
            .expect("serialize test token"),
        )
        .expect("write test token");
    }

    pub(crate) fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_threads-cli"))
            .args(arguments)
            .env("HOME", self.state.path())
            .env("XDG_CONFIG_HOME", self.state.path().join("config"))
            .env("XDG_DATA_HOME", self.state.path().join("data"))
            .output()
            .expect("run threads-cli")
    }

    fn seed(&self) {
        let store = Store::open(&self.db).expect("open isolated store");
        let first = Utc
            .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .single()
            .expect("timestamp");
        let second = Utc
            .with_ymd_and_hms(2025, 2, 1, 0, 0, 0)
            .single()
            .expect("timestamp");
        let demographics = vec![
            DemographicBucket {
                dimension: DemographicDimension::Country,
                bucket: "US".into(),
                value: 75,
            },
            DemographicBucket {
                dimension: DemographicDimension::City,
                bucket: "Austin".into(),
                value: 30,
            },
            DemographicBucket {
                dimension: DemographicDimension::Age,
                bucket: "25-34".into(),
                value: 45,
            },
            DemographicBucket {
                dimension: DemographicDimension::Gender,
                bucket: "female".into(),
                value: 52,
            },
        ];
        for snapshot in [
            AudienceSnapshot {
                account_id: UserId::new("account-a"),
                observed_at: first,
                followers_count: 120,
                demographics: demographics.clone(),
            },
            AudienceSnapshot {
                account_id: UserId::new("account-a"),
                observed_at: second,
                followers_count: 110,
                demographics,
            },
            AudienceSnapshot {
                account_id: UserId::new("account-b"),
                observed_at: first,
                followers_count: 999,
                demographics: Vec::new(),
            },
        ] {
            store
                .upsert_audience_snapshot(&snapshot, None)
                .expect("seed snapshot");
        }
        let mut root = post("root-a", "account-a");
        root.author_username = Some("owner".into());
        let mut direct = post("direct-reply", "engaged-a");
        direct.parent_id = Some(PostId::new("root-a"));
        direct.author_username = Some("direct".into());
        let mut nested = post("nested-reply", "engaged-b");
        nested.parent_id = Some(PostId::new("direct-reply"));
        let mut mentioned = post("mention-post", "engaged-a");
        mentioned.mentions.push(Mention {
            username: "owner".into(),
            user_id: Some(UserId::new("account-a")),
        });
        let mut other_root = post("root-b", "account-b");
        other_root.author_username = Some("other".into());
        let mut other_reply = post("other-reply", "outsider");
        other_reply.parent_id = Some(PostId::new("root-b"));
        store
            .upsert_posts(
                &[root, direct, nested, mentioned, other_root, other_reply],
                None,
            )
            .expect("seed graph");
    }
}

fn post(id: &str, author: &str) -> Post {
    Post {
        id: PostId::new(id),
        author: UserId::new(author),
        author_username: None,
        text: Some(id.into()),
        created_at: None,
        parent_id: None,
        root_id: None,
        permalink: None,
        media: Vec::new(),
        urls: Vec::new(),
        mentions: Vec::new(),
        is_quote_post: false,
        raw: None,
    }
}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
