use chrono::{DateTime, Utc};
use serde::Serialize;
use threads_core::{AudienceSnapshot, DemographicDimension};

#[path = "audience/render.rs"]
mod render;
pub use render::{render_audience_report, render_engaged_accounts};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudienceReportError {
    MixedAccountIds {
        expected_account_id: String,
        found_account_id: String,
    },
}

impl AudienceReportError {
    pub fn expected_account_id(&self) -> &str {
        match self {
            Self::MixedAccountIds {
                expected_account_id,
                found_account_id: _,
            } => expected_account_id,
        }
    }

    pub fn found_account_id(&self) -> &str {
        match self {
            Self::MixedAccountIds {
                expected_account_id: _,
                found_account_id,
            } => found_account_id,
        }
    }
}

impl std::fmt::Display for AudienceReportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MixedAccountIds {
                expected_account_id,
                found_account_id,
            } => write!(
                formatter,
                "audience report mixes account IDs {expected_account_id:?} and {found_account_id:?}"
            ),
        }
    }
}

impl std::error::Error for AudienceReportError {}

#[derive(Clone, Debug, Serialize)]
pub struct AudienceReport {
    pub account_id: Option<String>,
    pub snapshots: Vec<AudienceSnapshotOutput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudienceSnapshotOutput {
    pub observed_at: DateTime<Utc>,
    pub followers_count: u64,
    pub delta: Option<i64>,
    pub demographics: Vec<DemographicOutput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DemographicOutput {
    pub dimension: DemographicDimension,
    pub bucket: String,
    pub value: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudienceOutputRow {
    Snapshot {
        account_id: String,
        observed_at: DateTime<Utc>,
        followers_count: u64,
        delta: Option<i64>,
    },
    Demographic {
        account_id: String,
        observed_at: DateTime<Utc>,
        followers_count: u64,
        delta: Option<i64>,
        dimension: DemographicDimension,
        bucket: String,
        value: u64,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct EngagedOutputRow {
    pub rank: usize,
    pub user_id: String,
    pub username: Option<String>,
    pub replies: u64,
    pub mentions: u64,
    pub total: u64,
}

impl AudienceReport {
    pub fn from_snapshots(
        snapshots: &[AudienceSnapshot],
    ) -> std::result::Result<Self, AudienceReportError> {
        let Some(first) = snapshots.first() else {
            return Ok(Self {
                account_id: None,
                snapshots: Vec::new(),
            });
        };
        let expected_account_id = first.account_id.as_str();
        if let Some(snapshot) = snapshots
            .iter()
            .find(|snapshot| snapshot.account_id.as_str() != expected_account_id)
        {
            return Err(AudienceReportError::MixedAccountIds {
                expected_account_id: expected_account_id.to_owned(),
                found_account_id: snapshot.account_id.to_string(),
            });
        }
        let mut ordered: Vec<&AudienceSnapshot> = snapshots.iter().collect();
        ordered.sort_by_key(|snapshot| snapshot.observed_at);

        let account_id = ordered
            .first()
            .map(|snapshot| snapshot.account_id.to_string());
        let mut previous_count = None;
        let snapshots = ordered
            .into_iter()
            .map(|snapshot| {
                let delta = previous_count
                    .and_then(|previous| follower_delta(previous, snapshot.followers_count));
                previous_count = Some(snapshot.followers_count);
                let mut demographics: Vec<DemographicOutput> = snapshot
                    .demographics
                    .iter()
                    .map(|bucket| DemographicOutput {
                        dimension: bucket.dimension,
                        bucket: bucket.bucket.clone(),
                        value: bucket.value,
                    })
                    .collect();
                demographics.sort_by(|left, right| {
                    dimension_key(left.dimension)
                        .cmp(dimension_key(right.dimension))
                        .then_with(|| left.bucket.cmp(&right.bucket))
                });
                AudienceSnapshotOutput {
                    observed_at: snapshot.observed_at,
                    followers_count: snapshot.followers_count,
                    delta,
                    demographics,
                }
            })
            .collect();
        Ok(Self {
            account_id,
            snapshots,
        })
    }

    pub fn flat_rows(&self) -> Vec<AudienceOutputRow> {
        let Some(account_id) = self.account_id.as_deref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for snapshot in &self.snapshots {
            rows.push(AudienceOutputRow::Snapshot {
                account_id: account_id.to_owned(),
                observed_at: snapshot.observed_at,
                followers_count: snapshot.followers_count,
                delta: snapshot.delta,
            });
            rows.extend(snapshot.demographics.iter().map(|demographic| {
                AudienceOutputRow::Demographic {
                    account_id: account_id.to_owned(),
                    observed_at: snapshot.observed_at,
                    followers_count: snapshot.followers_count,
                    delta: snapshot.delta,
                    dimension: demographic.dimension,
                    bucket: demographic.bucket.clone(),
                    value: demographic.value,
                }
            }));
        }
        rows
    }
}

fn follower_delta(previous: u64, current: u64) -> Option<i64> {
    let previous = i64::try_from(previous).ok()?;
    let current = i64::try_from(current).ok()?;
    current.checked_sub(previous)
}

pub(super) const fn dimension_key(dimension: DemographicDimension) -> &'static str {
    match dimension {
        DemographicDimension::Country => "country",
        DemographicDimension::City => "city",
        DemographicDimension::Age => "age",
        DemographicDimension::Gender => "gender",
    }
}
