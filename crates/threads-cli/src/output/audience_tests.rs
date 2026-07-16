use chrono::{TimeZone, Utc};
use threads_core::{
    AudienceSnapshot, DemographicBucket, DemographicDimension, EngagedAccount, UserId,
};

use super::{AudienceReport, OutputFormat, render_audience_report, render_engaged_accounts};

fn audience_history() -> Vec<AudienceSnapshot> {
    vec![
        AudienceSnapshot {
            account_id: UserId::new("account-1"),
            observed_at: Utc.with_ymd_and_hms(2026, 7, 14, 9, 0, 0).unwrap(),
            followers_count: 120,
            demographics: vec![
                DemographicBucket {
                    dimension: DemographicDimension::Gender,
                    bucket: "women, \"quoted\"".into(),
                    value: 70,
                },
                DemographicBucket {
                    dimension: DemographicDimension::Country,
                    bucket: "日本".into(),
                    value: 50,
                },
            ],
        },
        AudienceSnapshot {
            account_id: UserId::new("account-1"),
            observed_at: Utc.with_ymd_and_hms(2026, 7, 15, 9, 0, 0).unwrap(),
            followers_count: 115,
            demographics: vec![
                DemographicBucket {
                    dimension: DemographicDimension::Age,
                    bucket: "25-34".into(),
                    value: 80,
                },
                DemographicBucket {
                    dimension: DemographicDimension::City,
                    bucket: "São Paulo".into(),
                    value: 35,
                },
            ],
        },
    ]
}

#[test]
fn report_preserves_history_and_negative_deltas() {
    // Given: chronological snapshots spanning all presentation dimensions.
    let report = AudienceReport::from_snapshots(&audience_history()).unwrap();

    // When: the structured report is converted to flat rows.
    let rows = report.flat_rows();

    // Then: the snapshot delta and tagged demographic rows remain stable.
    assert_eq!(rows.len(), 6);
    assert_eq!(
        serde_json::to_value(&rows[3]).unwrap()["delta"],
        serde_json::json!(-5)
    );
    assert_eq!(
        serde_json::to_value(&rows[1]).unwrap()["dimension"],
        serde_json::json!("country")
    );
    assert_eq!(
        serde_json::to_value(&rows[2]).unwrap()["dimension"],
        serde_json::json!("gender")
    );
}

#[test]
fn report_orders_stale_snapshots_before_latest() {
    // Given: history supplied in unstable newest-first order.
    let mut history = audience_history();
    history.reverse();

    // When: the report normalizes persisted observations.
    let report = AudienceReport::from_snapshots(&history).unwrap();

    // Then: its structured snapshots are chronological and the latest delta is stable.
    assert_eq!(report.snapshots[0].followers_count, 120);
    assert_eq!(report.snapshots[1].followers_count, 115);
    assert_eq!(report.snapshots[1].delta, Some(-5));
}

#[test]
fn report_rejects_mixed_account_history() {
    // Given: snapshots belonging to two different local accounts.
    let mut history = audience_history();
    history[1].account_id = UserId::new("account-2");

    // When: report construction receives the mixed history.
    let error = AudienceReport::from_snapshots(&history).unwrap_err();

    // Then: it returns the typed account-boundary error instead of relabeling rows.
    assert_eq!(error.expected_account_id(), "account-1");
    assert_eq!(error.found_account_id(), "account-2");
}

#[test]
fn audience_renders_all_formats_without_raw_or_identity_list_fields() {
    // Given: snapshots with CSV-sensitive and Unicode demographic buckets.
    let report = AudienceReport::from_snapshots(&audience_history()).unwrap();

    // When: every public format is rendered.
    let formats = [
        OutputFormat::Human,
        OutputFormat::Json,
        OutputFormat::Jsonl,
        OutputFormat::Csv,
    ];

    // Then: each format carries the stable report contract and no provider payload fields.
    for format in formats {
        let mut buffer = Vec::new();
        render_audience_report(&report, format, &mut buffer).unwrap();
        let output = String::from_utf8(buffer).unwrap();
        assert!(output.contains("account-1"));
        assert!(output.contains("-5"));
        assert!(!output.contains("raw"));
        assert!(!output.contains("follower_list"));
    }
}

#[test]
fn audience_csv_escapes_unicode_and_quotes() {
    // Given: a demographic bucket requiring CSV quoting.
    let report = AudienceReport::from_snapshots(&audience_history()).unwrap();
    let mut buffer = Vec::new();

    // When: CSV output is parsed through the CSV reader.
    render_audience_report(&report, OutputFormat::Csv, &mut buffer).unwrap();
    let records: Vec<csv::StringRecord> = csv::Reader::from_reader(buffer.as_slice())
        .records()
        .collect::<Result<_, _>>()
        .unwrap();

    // Then: the original Unicode and quotes survive without column shifts.
    assert_eq!(records[1].get(6), Some("日本"));
    assert_eq!(records[2].get(6), Some("women, \"quoted\""));
}

#[test]
fn engaged_rows_keep_input_rank_and_optional_username() {
    // Given: the store's deterministic ranking, including a missing username.
    let accounts = vec![
        EngagedAccount {
            user_id: UserId::new("stable-b"),
            username: None,
            replies: 3,
            mentions: 2,
            total: 5,
        },
        EngagedAccount {
            user_id: UserId::new("stable-a"),
            username: Some("café".into()),
            replies: 2,
            mentions: 1,
            total: 3,
        },
    ];

    // When: engagement output is rendered as JSONL.
    let mut buffer = Vec::new();
    render_engaged_accounts(&accounts, OutputFormat::Jsonl, &mut buffer).unwrap();

    // Then: rank follows input order and optional usernames remain explicit.
    let rows: Vec<serde_json::Value> = String::from_utf8(buffer)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["rank"], serde_json::json!(1));
    assert_eq!(rows[0]["user_id"], serde_json::json!("stable-b"));
    assert_eq!(rows[0]["username"], serde_json::Value::Null);
    assert_eq!(rows[1]["rank"], serde_json::json!(2));
    assert_eq!(rows[1]["username"], serde_json::json!("café"));
}

#[test]
fn engaged_renders_all_formats_without_audience_labels() {
    // Given: one observed account with both reply and mention activity.
    let accounts = [EngagedAccount {
        user_id: UserId::new("stable-1"),
        username: Some("café".into()),
        replies: 3,
        mentions: 2,
        total: 5,
    }];

    // When: every public format is rendered.
    for format in [
        OutputFormat::Human,
        OutputFormat::Json,
        OutputFormat::Jsonl,
        OutputFormat::Csv,
    ] {
        let mut buffer = Vec::new();
        render_engaged_accounts(&accounts, format, &mut buffer).unwrap();
        let output = String::from_utf8(buffer).unwrap();

        // Then: stable identity and component counts remain visible without audience claims.
        assert!(output.contains("stable-1"));
        assert!(output.contains("café"));
        assert!(output.contains("mentions"));
        assert!(!output.contains("follower"));
    }
}

#[test]
fn human_engagement_sanitizes_terminal_control_characters() {
    // Given: an identity and username containing terminal row-control characters.
    let accounts = [EngagedAccount {
        user_id: UserId::new("stable\nidentity\tvalue\rnext"),
        username: Some("name\nwith\tcontrols\r".into()),
        replies: 3,
        mentions: 2,
        total: 5,
    }];

    // When: the human renderer writes the engagement table.
    let mut buffer = Vec::new();
    render_engaged_accounts(&accounts, OutputFormat::Human, &mut buffer).unwrap();
    let output = String::from_utf8(buffer).unwrap();

    // Then: it remains one data row and contains no terminal control characters.
    assert_eq!(output.lines().count(), 2);
    assert!(!output.lines().nth(1).unwrap().contains(['\r', '\t']));
}

#[test]
fn empty_audience_and_engagement_are_successful_and_clear() {
    // Given: no persisted snapshots or observed engagement.
    let report = AudienceReport::from_snapshots(&[]).unwrap();

    // When: the empty human reports are rendered.
    let mut audience = Vec::new();
    let mut engaged = Vec::new();
    render_audience_report(&report, OutputFormat::Human, &mut audience).unwrap();
    render_engaged_accounts(&[], OutputFormat::Human, &mut engaged).unwrap();

    // Then: both outputs communicate successful absence of local observations.
    assert_eq!(
        String::from_utf8(audience).unwrap(),
        "No audience observations found.\n"
    );
    assert_eq!(
        String::from_utf8(engaged).unwrap(),
        "No observed engagement found.\n"
    );
}
