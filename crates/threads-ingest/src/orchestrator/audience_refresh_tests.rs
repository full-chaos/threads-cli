use std::sync::Arc;

use async_trait::async_trait;
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, Error, Page, PermissionRequirement, Result, User,
    UserId,
};
use threads_store::Store;

use super::*;
use crate::OfficialNormalizer;

#[derive(Clone, Copy)]
enum Failure {
    MeForbidden,
    FollowersForbidden,
    DemographicsForbidden,
    MentionsForbidden,
    MentionsNetwork,
}

struct AudienceProvider {
    failure: Failure,
}

impl AudienceProvider {
    fn permission_denied() -> Error {
        Error::PermissionDenied("403 Forbidden".into())
    }

    fn account() -> User {
        User {
            id: UserId::new("account"),
            username: Some("account".into()),
            name: None,
            biography: None,
            profile_picture_url: None,
        }
    }
}

#[async_trait]
impl threads_core::Provider for AudienceProvider {
    fn name(&self) -> &'static str {
        "audience-test"
    }

    async fn fetch_me(&self) -> Result<User> {
        match self.failure {
            Failure::MeForbidden => Err(Self::permission_denied()),
            _ => Ok(Self::account()),
        }
    }

    async fn fetch_my_threads(
        &self,
        _cursor: Option<threads_core::Cursor>,
    ) -> Result<Page<threads_core::Post>> {
        Ok(Page::empty())
    }

    async fn fetch_replies(
        &self,
        _post_id: &threads_core::PostId,
        _cursor: Option<threads_core::Cursor>,
    ) -> Result<Page<threads_core::Post>> {
        Ok(Page::empty())
    }

    async fn fetch_thread(
        &self,
        _root_id: &threads_core::PostId,
    ) -> Result<Vec<threads_core::Post>> {
        Ok(Vec::new())
    }

    async fn fetch_audience_insight(
        &self,
        _user_id: &UserId,
        query: AudienceInsightQuery,
    ) -> Result<AudienceInsightResult> {
        match (self.failure, query) {
            (Failure::FollowersForbidden, AudienceInsightQuery::FollowersCount)
            | (Failure::DemographicsForbidden, AudienceInsightQuery::FollowerDemographics(_)) => {
                Err(Self::permission_denied())
            }
            (_, AudienceInsightQuery::FollowersCount) => Ok(AudienceInsightResult::FollowersCount(
                u64::from(matches!(self.failure, Failure::DemographicsForbidden)) * 100 + 99,
            )),
            (_, AudienceInsightQuery::FollowerDemographics(dimension)) => Ok(
                AudienceInsightResult::Demographics(threads_core::DemographicInsight {
                    dimension,
                    buckets: Vec::new(),
                }),
            ),
        }
    }

    async fn fetch_mentions(
        &self,
        _user_id: &UserId,
        _cursor: Option<threads_core::Cursor>,
        _limit: usize,
    ) -> Result<Page<threads_core::Post>> {
        match self.failure {
            Failure::MentionsForbidden => Err(Self::permission_denied()),
            Failure::MentionsNetwork => Err(Error::Network("offline".into())),
            _ => Ok(Page::empty()),
        }
    }
}

fn refresh(failure: Failure) -> Ingestor<AudienceProvider, Store> {
    Ingestor::new(
        Arc::new(AudienceProvider { failure }),
        Box::new(OfficialNormalizer),
        Arc::new(Store::open_in_memory().expect("in-memory store")),
    )
}

#[tokio::test]
async fn refresh_maps_me_forbidden_to_threads_basic_requirement() {
    let result = refresh(Failure::MeForbidden).refresh_audience().await;

    assert!(matches!(
        result,
        Err(Error::MissingPermission {
            requirement: PermissionRequirement::AuthenticatedAccount,
            ..
        })
    ));
}

#[tokio::test]
async fn refresh_maps_follower_and_demographic_forbidden_to_insights_requirement() {
    for failure in [Failure::FollowersForbidden, Failure::DemographicsForbidden] {
        let result = refresh(failure).refresh_audience().await;

        assert!(matches!(
            result,
            Err(Error::MissingPermission {
                requirement: PermissionRequirement::AudienceInsights,
                ..
            })
        ));
    }
}

#[tokio::test]
async fn refresh_downgrades_mentions_forbidden_to_mentions_scope_warning() {
    let summary = refresh(Failure::MentionsForbidden)
        .refresh_audience()
        .await
        .expect("mentions permission failure becomes a warning");

    assert!(matches!(
        summary.mention_warning,
        Some(MentionIngestWarning::PermissionDenied(scope)) if scope == "threads_manage_mentions"
    ));
}

#[tokio::test]
async fn refresh_preserves_non_permission_failures() {
    let result = refresh(Failure::MentionsNetwork).refresh_audience().await;

    assert!(matches!(result, Err(Error::Network(message)) if message == "offline"));
}

#[test]
fn permission_mapping_preserves_non_permission_errors() {
    let error = permission_requirement(
        Error::Parse("invalid insight payload".into()),
        PermissionRequirement::AudienceInsights,
    );

    assert!(matches!(error, Error::Parse(message) if message == "invalid insight payload"));
}
