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

fn default_get() -> String {
    "GET".to_string()
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
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../manifests/official_v1.toml");
        let m = Manifest::from_path(path).expect("manifest should parse");
        assert!(m.object("me").is_some(), "manifest must define `me` object");
        assert!(m.edge("me/threads").is_some(), "manifest must define `me/threads` edge");
    }
}
