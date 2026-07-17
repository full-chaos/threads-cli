use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::UserId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemographicDimension {
    Country,
    City,
    Age,
    Gender,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemographicBucket {
    pub dimension: DemographicDimension,
    pub bucket: String,
    pub value: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemographicInsight {
    pub dimension: DemographicDimension,
    pub buckets: Vec<DemographicBucket>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudienceSnapshot {
    pub account_id: UserId,
    pub observed_at: DateTime<Utc>,
    pub followers_count: u64,
    pub demographics: Vec<DemographicBucket>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceInsightQuery {
    FollowersCount,
    FollowerDemographics(DemographicDimension),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AudienceInsightResult {
    FollowersCount(u64),
    Demographics(DemographicInsight),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngagementSort {
    Total,
    Replies,
    Mentions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagedAccount {
    pub user_id: UserId,
    pub username: Option<String>,
    pub replies: u64,
    pub mentions: u64,
    pub total: u64,
}
