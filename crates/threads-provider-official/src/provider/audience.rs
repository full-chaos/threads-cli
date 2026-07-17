use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, DemographicBucket, DemographicDimension,
    DemographicInsight, Error, Result, UserId,
};

use super::OfficialProvider;
use crate::dto::{InsightDto, InsightsEnvelope};

pub(super) async fn fetch_insight(
    provider: &OfficialProvider,
    user_id: &UserId,
    query: AudienceInsightQuery,
) -> Result<AudienceInsightResult> {
    let path = provider
        .object_path("user/insights")
        .ok_or_else(|| Error::Manifest("missing object `user/insights`".into()))?;
    let path = OfficialProvider::substitute_user_id(&path, user_id);
    let owned_params = insight_params(&query);
    let params = owned_params
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    let insights: InsightsEnvelope = provider.http.get_json(&path, &params).await?;
    into_result(insights, query)
}

pub(super) fn insight_params(query: &AudienceInsightQuery) -> Vec<(&'static str, String)> {
    match query {
        AudienceInsightQuery::FollowersCount => vec![("metric", "followers_count".into())],
        AudienceInsightQuery::FollowerDemographics(dimension) => vec![
            ("metric", "follower_demographics".into()),
            ("breakdown", dimension_wire(dimension).into()),
        ],
    }
}

pub(super) fn into_result(
    insights: InsightsEnvelope,
    query: AudienceInsightQuery,
) -> Result<AudienceInsightResult> {
    let expected_name = metric(&query);
    let insight = insights
        .data
        .into_iter()
        .find(|insight| insight.name == expected_name)
        .ok_or_else(|| Error::Parse(format!("missing {expected_name} insight data")))?;

    match query {
        AudienceInsightQuery::FollowersCount => followers_count(insight),
        AudienceInsightQuery::FollowerDemographics(dimension) => demographics(insight, dimension),
    }
}

const fn metric(query: &AudienceInsightQuery) -> &'static str {
    match query {
        AudienceInsightQuery::FollowersCount => "followers_count",
        AudienceInsightQuery::FollowerDemographics(_) => "follower_demographics",
    }
}

pub(super) const fn dimension_wire(dimension: &DemographicDimension) -> &'static str {
    match dimension {
        DemographicDimension::Country => "country",
        DemographicDimension::City => "city",
        DemographicDimension::Age => "age",
        DemographicDimension::Gender => "gender",
    }
}

fn followers_count(insight: InsightDto) -> Result<AudienceInsightResult> {
    let value = insight
        .total_value
        .value
        .ok_or_else(|| Error::Parse("followers_count insight is missing a value".into()))?;
    Ok(AudienceInsightResult::FollowersCount(value))
}

fn demographics(
    insight: InsightDto,
    dimension: DemographicDimension,
) -> Result<AudienceInsightResult> {
    let expected_dimension = dimension_wire(&dimension);
    let [breakdown] = insight.total_value.breakdowns.as_slice() else {
        return Err(Error::Parse(
            "follower_demographics insight is missing one breakdown".into(),
        ));
    };
    if breakdown.dimension_keys.as_slice() != [expected_dimension] {
        return Err(Error::Parse(format!(
            "follower_demographics breakdown does not match {expected_dimension}"
        )));
    }
    let buckets = breakdown
        .results
        .iter()
        .map(|result| {
            let bucket = result.dimension_values.first().cloned().ok_or_else(|| {
                Error::Parse("follower_demographics result is missing a dimension value".into())
            })?;
            Ok(DemographicBucket {
                dimension,
                bucket,
                value: result.value,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(AudienceInsightResult::Demographics(DemographicInsight {
        dimension,
        buckets,
    }))
}
