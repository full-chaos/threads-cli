//! # threads-manifest
//!
//! Parses versioned local API manifests describing endpoints, fields, edges,
//! and required OAuth permissions for the Threads Graph API.
//!
//! Per the PRD, we do NOT use GraphQL introspection — `graph.threads.net` is a
//! REST-like Graph API. A static TOML manifest gives us a compile-time contract
//! we can diff in PRs, generate typed request builders from, and validate
//! against recorded fixtures.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("invalid manifest: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ManifestError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub api: ApiSection,
    #[serde(default)]
    pub objects: Vec<ObjectDef>,
    #[serde(default)]
    pub edges: Vec<EdgeDef>,
    /// Write operations exposed by the provider manifest.
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiSection {
    pub base_url: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectDef {
    pub name: String,
    pub path: String,
    #[serde(default = "default_get")]
    pub method: String,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub permission: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeDef {
    pub name: String,
    pub path: String,
    #[serde(default = "default_get")]
    pub method: String,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub paginated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    /// Stable action key, e.g. `post/delete`.
    pub name: String,
    /// Request path, absolute or relative to the API base URL.
    pub path: String,
    /// HTTP method for the write operation.
    pub method: String,
    /// OAuth permission required for the action.
    pub permission: String,
    /// Whether Meta officially documents the action.
    #[serde(default = "default_documented")]
    pub documented: bool,
    /// Optional provider-enforced daily action limit.
    pub rate_limit_per_day: Option<u32>,
}

fn default_get() -> String {
    "GET".to_string()
}

fn default_documented() -> bool {
    true
}

impl Manifest {
    #[allow(clippy::should_implement_trait)] // Result type differs from FromStr's
    pub fn from_str(s: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(s)?;
        m.validate()?;
        Ok(m)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let s = fs::read_to_string(path.as_ref())?;
        Self::from_str(&s)
    }

    pub fn object(&self, name: &str) -> Option<&ObjectDef> {
        self.objects.iter().find(|o| o.name == name)
    }

    pub fn edge(&self, name: &str) -> Option<&EdgeDef> {
        self.edges.iter().find(|e| e.name == name)
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    fn validate(&self) -> Result<()> {
        if self.api.base_url.is_empty() {
            return Err(ManifestError::Invalid("api.base_url is empty".into()));
        }
        if self.api.version.is_empty() {
            return Err(ManifestError::Invalid("api.version is empty".into()));
        }
        for o in &self.objects {
            if o.name.is_empty() || o.path.is_empty() {
                return Err(ManifestError::Invalid(format!(
                    "object has empty name or path: {o:?}"
                )));
            }
        }
        for e in &self.edges {
            if e.name.is_empty() || e.path.is_empty() {
                return Err(ManifestError::Invalid(format!(
                    "edge has empty name or path: {e:?}"
                )));
            }
        }
        for a in &self.actions {
            if a.name.is_empty() || a.path.is_empty() {
                return Err(ManifestError::Invalid(format!(
                    "action has empty name or path: {a:?}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[api]
base_url = "https://graph.threads.net"
version = "v1.0"

[[objects]]
name = "me"
path = "/v1.0/me"
method = "GET"
fields = ["id", "username", "name"]

[[edges]]
name = "me/threads"
path = "/v1.0/me/threads"
method = "GET"
permission = "threads_basic"
paginated = true
"#;

    #[test]
    fn parses_sample() {
        let m = Manifest::from_str(SAMPLE).unwrap();
        assert_eq!(m.api.base_url, "https://graph.threads.net");
        assert_eq!(m.api.version, "v1.0");
        assert!(m.object("me").is_some());
        assert!(m.edge("me/threads").is_some());
        assert!(m.edge("me/threads").unwrap().paginated);
    }

    #[test]
    fn rejects_empty_base_url() {
        let s = r#"
[api]
base_url = ""
version = "v1.0"
"#;
        let err = Manifest::from_str(s).unwrap_err();
        assert!(matches!(err, ManifestError::Invalid(_)));
    }

    #[test]
    fn parses_official_v1_manifest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        let m = Manifest::from_path(path).expect("manifest should parse");
        assert!(m.object("me").is_some(), "manifest must define `me` object");
        assert!(
            m.edge("me/threads").is_some(),
            "manifest must define `me/threads` edge"
        );
    }

    #[test]
    fn official_manifest_contains_no_follower_or_follow_actions() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        let manifest = Manifest::from_path(path).expect("manifest should parse");

        assert!(
            manifest
                .actions
                .iter()
                .all(|action| !action.name.contains("follower") && !action.name.contains("follow")),
            "official manifest must not define follower or follow actions"
        );
    }

    #[test]
    fn official_manifest_defines_audience_insights_and_mentions_contracts() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        let manifest = Manifest::from_path(path).expect("manifest should parse");

        let insights = manifest
            .object("user/insights")
            .expect("user/insights object missing");
        assert_eq!(insights.method, "GET");
        assert_eq!(insights.path, "/{threads-user-id}/threads_insights");
        assert_eq!(
            insights.permission.as_deref(),
            Some("threads_manage_insights")
        );
        assert_eq!(
            insights.fields,
            ["followers_count", "follower_demographics"]
        );

        let mentions = manifest
            .edge("user/mentions")
            .expect("user/mentions edge missing");
        assert_eq!(mentions.method, "GET");
        assert_eq!(mentions.path, "/{threads-user-id}/mentions");
        assert_eq!(
            mentions.permission.as_deref(),
            Some("threads_manage_mentions")
        );
        assert!(mentions.paginated);
        println!(
            "insights: name={}, path={}, permission={}; mentions: name={}, path={}, permission={}, paginated={}",
            insights.name,
            insights.path,
            insights.permission.as_deref().unwrap_or("none"),
            mentions.name,
            mentions.path,
            mentions.permission.as_deref().unwrap_or("none"),
            mentions.paginated,
        );
        assert_eq!(
            mentions.fields,
            [
                "id",
                "media_product_type",
                "media_type",
                "media_url",
                "permalink",
                "owner",
                "username",
                "text",
                "timestamp",
                "shortcode",
                "thumbnail_url",
                "children",
                "is_quote_post",
            ]
        );
    }

    #[test]
    fn audience_fixtures_cover_required_envelopes_and_pagination() {
        const FIXTURES: [(&str, &str); 8] = [
            ("audience_followers_count.json", "\"followers_count\""),
            ("audience_demographics_country.json", "\"country\""),
            ("audience_demographics_city.json", "\"city\""),
            ("audience_demographics_age.json", "\"age\""),
            ("audience_demographics_gender.json", "\"gender\""),
            ("audience_empty_data.json", "\"data\": []"),
            ("mentions_page.json", "\"after\": \"QVFIUnhR\""),
            ("mentions_terminal_page.json", "\"after\": null"),
        ];
        let fixtures_directory = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures"
        );

        for (file_name, required_fragment) in FIXTURES {
            let fixture = fs::read_to_string(format!("{fixtures_directory}/{file_name}"))
                .expect("audience fixture should exist");
            assert!(
                fixture.contains("\"data\""),
                "{file_name} must envelope data"
            );
            assert!(
                fixture.contains(required_fragment),
                "{file_name} missing required contract fragment"
            );
        }

        for file_name in ["audience_401.json", "audience_403.json"] {
            let fixture = fs::read_to_string(format!("{fixtures_directory}/{file_name}"))
                .expect("audience error fixture should exist");
            assert!(
                fixture.contains("\"error\""),
                "{file_name} must envelope error"
            );
        }

        let malformed = fs::read_to_string(format!("{fixtures_directory}/audience_malformed.json"))
            .expect("malformed audience fixture should exist");
        assert!(
            !malformed.contains("\"data\""),
            "malformed fixture must omit the required data envelope"
        );
    }

    #[test]
    fn official_manifest_has_publish_actions_and_objects() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        let m = Manifest::from_path(path).expect("manifest should parse");

        let create = m.action("post/create").expect("post/create action missing");
        assert_eq!(create.method, "POST");
        assert_eq!(create.path, "/v1.0/me/threads");
        assert_eq!(create.permission, "threads_content_publish");
        assert_eq!(create.rate_limit_per_day, Some(250));

        let publish = m
            .action("post/publish")
            .expect("post/publish action missing");
        assert_eq!(publish.method, "POST");
        assert_eq!(publish.path, "/v1.0/me/threads_publish");
        assert_eq!(publish.permission, "threads_content_publish");

        let container = m.object("container").expect("container object missing");
        assert_eq!(container.path, "/v1.0/{container-id}");
        assert!(container.fields.contains(&"status".to_string()));

        let limits = m
            .object("publishing_limit")
            .expect("publishing_limit object missing");
        assert_eq!(limits.path, "/v1.0/me/threads_publishing_limit");
        assert!(limits.fields.iter().any(|f| f == "quota_usage"));
        assert!(limits.fields.iter().any(|f| f == "reply_quota_usage"));
    }

    #[test]
    fn action_round_trips_with_default_documented() {
        let s = r#"
[api]
base_url = "https://graph.threads.net"
version = "v1.0"

[[actions]]
name = "post/delete"
path = "/v1.0/{post-id}"
method = "DELETE"
permission = "threads_delete"
rate_limit_per_day = 100
"#;
        let m = Manifest::from_str(s).unwrap();
        assert_eq!(m.actions.len(), 1);
        assert!(m.actions[0].documented);
        assert_eq!(m.actions[0].rate_limit_per_day, Some(100));

        let toml = toml::to_string(&m).unwrap();
        let reparsed = Manifest::from_str(&toml).unwrap();
        assert_eq!(reparsed.actions[0].name, "post/delete");
        assert!(reparsed.actions[0].documented);
    }
}
