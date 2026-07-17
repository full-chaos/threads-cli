use serde::{Deserialize, Serialize};

/// Shape of `/me` from graph.threads.net.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeDto {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub threads_biography: Option<String>,
    #[serde(default)]
    pub threads_profile_picture_url: Option<String>,
}

/// Shape of a post returned by /me/threads and /{post-id}.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PostDto {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub permalink: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub media_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub is_quote_post: bool,
    #[serde(default)]
    pub owner: Option<OwnerRefDto>,
    #[serde(default)]
    pub children: Option<ChildrenDto>,
    #[serde(default)]
    pub replied_to: Option<PostRefDto>,
    #[serde(default)]
    pub root_post: Option<PostRefDto>,
    #[serde(default)]
    pub is_reply: Option<bool>,
    #[serde(default)]
    pub shortcode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OwnerRefDto {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PostRefDto {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChildrenDto {
    #[serde(default)]
    pub data: Vec<PostDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InsightsEnvelope {
    pub data: Vec<InsightDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InsightDto {
    pub name: String,
    pub total_value: TotalValueDto,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TotalValueDto {
    #[serde(default)]
    pub value: Option<u64>,
    #[serde(default)]
    pub breakdowns: Vec<InsightBreakdownDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InsightBreakdownDto {
    #[serde(default)]
    pub dimension_keys: Vec<String>,
    #[serde(default)]
    pub results: Vec<InsightBreakdownResultDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InsightBreakdownResultDto {
    #[serde(default)]
    pub dimension_values: Vec<String>,
    pub value: u64,
}

/// Pagination envelope: `{ data: [...], paging: { cursors: { before, after } } }`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Envelope<T> {
    pub data: Vec<T>,
    #[serde(default)]
    pub paging: Option<Paging>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Paging {
    #[serde(default)]
    pub cursors: Option<Cursors>,
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Cursors {
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

/// Response from `POST /v1.0/me/threads` (create container).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateContainerResp {
    pub id: String,
}

/// Response from `POST /v1.0/me/threads_publish`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishResp {
    pub id: String,
}

/// Response from `GET /{container-id}?fields=status`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContainerStatusResp {
    pub status: String,
}

/// One element from `GET /me/threads_publishing_limit`.
/// The API wraps this in `{ "data": [ { ... } ] }` when field-projected;
/// the provider extracts `data[0]` before deserializing into this struct.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishingLimitResp {
    #[serde(default)]
    pub quota_usage: u32,
    pub config: PublishingLimitConfig,
    #[serde(default)]
    pub reply_quota_usage: u32,
    pub reply_config: PublishingLimitConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishingLimitConfig {
    pub quota_total: u32,
    pub quota_duration: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_me_minimal() {
        let v = r#"{"id":"123","username":"me"}"#;
        let m: MeDto = serde_json::from_str(v).unwrap();
        assert_eq!(m.id, "123");
        assert_eq!(m.username.as_deref(), Some("me"));
        assert!(m.name.is_none());
    }

    #[test]
    fn parses_envelope_with_paging() {
        let v = r#"{
            "data": [{"id":"a"},{"id":"b"}],
            "paging": {"cursors":{"after":"CURSOR"}}
        }"#;
        let e: Envelope<PostDto> = serde_json::from_str(v).unwrap();
        assert_eq!(e.data.len(), 2);
        assert_eq!(
            e.paging.unwrap().cursors.unwrap().after.as_deref(),
            Some("CURSOR")
        );
    }

    #[test]
    fn rejects_post_envelope_without_required_data() {
        // Given: a response that omits the documented collection payload.
        let missing_data = r#"{"paging":{"cursors":{"after":"CURSOR"}}}"#;

        // When: it crosses the post-envelope boundary.
        let result = serde_json::from_str::<Envelope<PostDto>>(missing_data);

        // Then: it cannot become a misleading empty page.
        assert!(result.is_err());
    }

    #[test]
    fn parses_reply_post_with_root_and_replied_to() {
        let v = r#"{
            "id":"r1","text":"hi",
            "replied_to":{"id":"p1"},
            "root_post":{"id":"root1"},
            "is_reply":true
        }"#;
        let p: PostDto = serde_json::from_str(v).unwrap();
        assert_eq!(p.replied_to.unwrap().id, "p1");
        assert_eq!(p.root_post.unwrap().id, "root1");
        assert_eq!(p.is_reply, Some(true));
    }

    #[test]
    fn parses_create_container_resp() {
        let v = r#"{"id":"container_abc123"}"#;
        let r: CreateContainerResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.id, "container_abc123");
    }

    #[test]
    fn parses_publish_resp() {
        let v = r#"{"id":"post_xyz999"}"#;
        let r: PublishResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.id, "post_xyz999");
    }

    #[test]
    fn parses_container_status_resp() {
        let v = r#"{"status":"FINISHED"}"#;
        let r: ContainerStatusResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.status, "FINISHED");
    }

    #[test]
    fn parses_publishing_limit_resp_full() {
        let v = r#"{
            "quota_usage": 3,
            "config": { "quota_total": 250, "quota_duration": 86400 },
            "reply_quota_usage": 12,
            "reply_config": { "quota_total": 1000, "quota_duration": 86400 }
        }"#;
        let r: PublishingLimitResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.quota_usage, 3);
        assert_eq!(r.config.quota_total, 250);
        assert_eq!(r.reply_quota_usage, 12);
        assert_eq!(r.reply_config.quota_total, 1000);
    }

    #[test]
    fn parses_publishing_limit_resp_wrapped_in_data_array() {
        // The API returns this as `{ "data": [{ ... }] }` when using fields=.
        // We parse the inner element only (provider unwraps data[0]).
        let inner = r#"{
            "quota_usage": 0,
            "config": { "quota_total": 250, "quota_duration": 86400 },
            "reply_quota_usage": 0,
            "reply_config": { "quota_total": 1000, "quota_duration": 86400 }
        }"#;
        let r: PublishingLimitResp = serde_json::from_str(inner).unwrap();
        assert_eq!(r.quota_usage, 0);
        assert_eq!(r.config.quota_total, 250);
    }

    #[test]
    fn parses_followers_count_fixture() {
        // Given: the official followers-count response fixture.
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/audience_followers_count.json"
        ));

        // When: the response crosses the Insights DTO boundary.
        let insights: InsightsEnvelope = serde_json::from_str(fixture).unwrap();

        // Then: the typed value remains available without raw JSON traversal.
        assert_eq!(insights.data[0].name, "followers_count");
        assert_eq!(insights.data[0].total_value.value, Some(1234));
    }

    #[test]
    fn parses_every_demographic_fixture() {
        // Given: one official fixture for each supported breakdown dimension.
        let fixtures = [
            ("country", "audience_demographics_country.json"),
            ("city", "audience_demographics_city.json"),
            ("age", "audience_demographics_age.json"),
            ("gender", "audience_demographics_gender.json"),
        ];

        // When: each fixture crosses the Insights DTO boundary.
        for (dimension, file_name) in fixtures {
            let fixture_path = format!(
                "{}/../threads-ingest/tests/fixtures/{file_name}",
                env!("CARGO_MANIFEST_DIR")
            );
            let fixture = std::fs::read_to_string(fixture_path).unwrap();
            let insights: InsightsEnvelope = serde_json::from_str(&fixture).unwrap();

            // Then: its breakdown is typed with the documented dimension.
            assert_eq!(
                insights.data[0].total_value.breakdowns[0].dimension_keys,
                vec![dimension.to_string()]
            );
        }
    }

    #[test]
    fn accepts_total_value_followers_count_and_rejects_legacy_values() {
        // Given: the documented Total Value envelope and the obsolete time-series shape.
        let documented = r#"{
            "data":[{
                "name":"followers_count",
                "total_value":{"value":1234}
            }]
        }"#;
        let obsolete = r#"{
            "data":[{
                "name":"followers_count",
                "values":[{"value":1234}]
            }]
        }"#;

        // When: each payload crosses the insight DTO boundary.
        let documented_result = serde_json::from_str::<InsightsEnvelope>(documented);
        let obsolete_result = serde_json::from_str::<InsightsEnvelope>(obsolete);

        // Then: only the documented Total Value envelope is accepted.
        assert!(documented_result.is_ok());
        assert!(obsolete_result.is_err());
    }

    #[test]
    fn accepts_total_value_demographics_and_rejects_root_breakdowns() {
        // Given: documented nested Total Value breakdowns and the obsolete root breakdown shape.
        let documented = r#"{
            "data":[{
                "name":"follower_demographics",
                "total_value":{"breakdowns":[{
                    "dimension_keys":["country"],
                    "results":[{"dimension_values":["US"],"value":900}]
                }]}
            }]
        }"#;
        let obsolete = r#"{
            "data":[{
                "name":"follower_demographics",
                "total_value":900,
                "breakdowns":[{
                    "dimension_keys":["country"],
                    "results":[{"dimension_values":["US"],"value":900}]
                }]
            }]
        }"#;

        // When: each payload crosses the insight DTO boundary.
        let documented_result = serde_json::from_str::<InsightsEnvelope>(documented);
        let obsolete_result = serde_json::from_str::<InsightsEnvelope>(obsolete);

        // Then: only the documented nested Total Value shape is accepted.
        assert!(documented_result.is_ok());
        assert!(obsolete_result.is_err());
    }

    #[test]
    fn rejects_missing_or_noninteger_insight_data() {
        // Given: malformed and noninteger Insights responses.
        let malformed = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/audience_malformed.json"
        ));
        let noninteger = r#"{"data":[{"name":"followers_count","total_value":{"value":"many"}}]}"#;

        // When: each response crosses the DTO boundary.
        let missing_data = serde_json::from_str::<InsightsEnvelope>(malformed);
        let invalid_value = serde_json::from_str::<InsightsEnvelope>(noninteger);

        // Then: both fail at that boundary.
        assert!(missing_data.is_err());
        assert!(invalid_value.is_err());
    }
}
