use std::collections::HashSet;

use chrono::Utc;
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, AudienceSnapshot, DemographicBucket,
    DemographicDimension, Error, Mention, PermissionRequirement, Result, User, UserId,
};

use super::{AudienceRefreshSummary, Ingestor, MentionIngestWarning};
use crate::store_shim::StoreWrite;

const MENTION_PAGE_SIZE: usize = 100;
const DEMOGRAPHIC_DIMENSIONS: [DemographicDimension; 4] = [
    DemographicDimension::Country,
    DemographicDimension::City,
    DemographicDimension::Age,
    DemographicDimension::Gender,
];

impl<P: threads_core::Provider + 'static, S: StoreWrite + 'static> Ingestor<P, S> {
    pub async fn refresh_audience(&self) -> Result<AudienceRefreshSummary> {
        self.refresh_audience_for_account(None).await
    }

    pub async fn refresh_audience_for_account(
        &self,
        expected_account_id: Option<&UserId>,
    ) -> Result<AudienceRefreshSummary> {
        let account = self.provider.fetch_me().await.map_err(|error| {
            permission_requirement(error, PermissionRequirement::AuthenticatedAccount)
        })?;
        if let Some(expected_account_id) = expected_account_id {
            if account.id != *expected_account_id {
                return Err(Error::Auth(format!(
                    "stored token is bound to account {expected_account_id}, but Threads authenticated account is {}; run `threads-cli auth login`",
                    account.id
                )));
            }
        }
        self.store.upsert_user(&account)?;
        if let Some(username) = &account.username {
            self.store.resolve_author(username, &account.id)?;
        }

        let followers_count = self.fetch_followers_count(&account.id).await?;
        let demographics = self
            .fetch_demographics(&account.id, followers_count)
            .await?;
        let snapshot = AudienceSnapshot {
            account_id: account.id.clone(),
            observed_at: Utc::now(),
            followers_count,
            demographics,
        };
        self.store.upsert_audience_snapshot(&snapshot, None)?;

        let (mentions_ingested, mention_warning) = self.ingest_mentions(&account).await?;
        Ok(AudienceRefreshSummary {
            account_id: account.id,
            followers_count,
            demographics_count: snapshot.demographics.len(),
            mentions_ingested,
            mention_warning,
        })
    }

    async fn fetch_followers_count(&self, account_id: &UserId) -> Result<u64> {
        match self
            .provider
            .fetch_audience_insight(account_id, AudienceInsightQuery::FollowersCount)
            .await
            .map_err(|error| {
                permission_requirement(error, PermissionRequirement::AudienceInsights)
            })? {
            AudienceInsightResult::FollowersCount(value) => Ok(value),
            AudienceInsightResult::Demographics(_) => Err(Error::Parse(
                "followers_count request returned demographics".into(),
            )),
        }
    }

    async fn fetch_demographics(
        &self,
        account_id: &UserId,
        followers_count: u64,
    ) -> Result<Vec<DemographicBucket>> {
        if followers_count < 100 {
            return Ok(vec![]);
        }

        let mut demographics = Vec::new();
        for dimension in DEMOGRAPHIC_DIMENSIONS {
            match self
                .provider
                .fetch_audience_insight(
                    account_id,
                    AudienceInsightQuery::FollowerDemographics(dimension),
                )
                .await
                .map_err(|error| {
                    permission_requirement(error, PermissionRequirement::AudienceInsights)
                })? {
                AudienceInsightResult::Demographics(insight) => {
                    if insight.dimension != dimension {
                        return Err(Error::Parse(format!(
                            "{dimension:?} demographics request returned {:?} demographics",
                            insight.dimension
                        )));
                    }
                    demographics.extend(insight.buckets);
                }
                AudienceInsightResult::FollowersCount(_) => {
                    return Err(Error::Parse(format!(
                        "{dimension:?} demographics request returned followers_count"
                    )));
                }
            }
        }
        Ok(demographics)
    }

    async fn ingest_mentions(&self, account: &User) -> Result<(u64, Option<MentionIngestWarning>)> {
        let Some(username) = account.username.as_deref() else {
            return Ok((0, Some(MentionIngestWarning::MissingAuthenticatedUsername)));
        };

        let mention = Mention {
            username: username.into(),
            user_id: Some(account.id.clone()),
        };
        let mut seen_posts = HashSet::new();
        let mut seen_cursors = HashSet::new();
        let mut cursor = None;
        let mut mentions_ingested = 0;

        loop {
            let page = match self
                .provider
                .fetch_mentions(&account.id, cursor, MENTION_PAGE_SIZE)
                .await
            {
                Ok(page) => page,
                Err(Error::PermissionDenied(_)) => {
                    return Ok((
                        mentions_ingested,
                        Some(MentionIngestWarning::PermissionDenied(
                            PermissionRequirement::Mentions.scope().into(),
                        )),
                    ));
                }
                Err(error) => return Err(error),
            };

            let mut posts = Vec::new();
            for mut post in page.items {
                if seen_posts.insert(post.id.clone()) {
                    if !post.mentions.iter().any(|existing| existing == &mention) {
                        post.mentions.push(mention.clone());
                    }
                    posts.push(post);
                }
            }
            if !posts.is_empty() {
                mentions_ingested += self.store.upsert_posts(&posts, None)? as u64;
            }

            let Some(next) = page.next else {
                return Ok((mentions_ingested, None));
            };
            if !seen_cursors.insert(next.0.clone()) {
                return Ok((mentions_ingested, None));
            }
            cursor = Some(next);
        }
    }
}

fn permission_requirement(error: Error, requirement: PermissionRequirement) -> Error {
    match error {
        Error::PermissionDenied(detail) => Error::MissingPermission {
            requirement,
            detail,
        },
        other => other,
    }
}

#[cfg(test)]
#[path = "audience_refresh_tests.rs"]
mod tests;
