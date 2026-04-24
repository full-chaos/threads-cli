use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use threads_core::{Error, Result};

/// Client/app configuration for the official Threads provider.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub app_id: String,
    pub app_secret: String,
    pub redirect_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
}

impl Config {
    /// Build a [`Config`] from env vars.
    ///
    /// Reads `THREADS_APP_ID`, `THREADS_APP_SECRET`, `THREADS_REDIRECT_URI`,
    /// and optionally `THREADS_ACCESS_TOKEN`. Returns [`Error::Config`] if
    /// any required var is absent.
    pub fn from_env() -> Result<Self> {
        let app_id = std::env::var("THREADS_APP_ID")
            .map_err(|_| Error::Config("THREADS_APP_ID not set".into()))?;
        let app_secret = std::env::var("THREADS_APP_SECRET")
            .map_err(|_| Error::Config("THREADS_APP_SECRET not set".into()))?;
        let redirect_uri = std::env::var("THREADS_REDIRECT_URI")
            .map_err(|_| Error::Config("THREADS_REDIRECT_URI not set".into()))?;
        Ok(Self {
            app_id,
            app_secret,
            redirect_uri,
            access_token: std::env::var("THREADS_ACCESS_TOKEN").ok(),
        })
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let s = fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("reading {}: {e}", path.display())))?;
        toml::from_str(&s).map_err(|e| Error::Config(format!("parsing {}: {e}", path.display())))
    }

    pub fn with_access_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_file_parses_toml() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "app_id = \"aid\"\napp_secret = \"sec\"\nredirect_uri = \"https://x/cb\"\n",
        )
        .unwrap();
        let cfg = Config::from_file(tmp.path()).unwrap();
        assert_eq!(cfg.app_id, "aid");
        assert_eq!(cfg.redirect_uri, "https://x/cb");
        assert!(cfg.access_token.is_none());
    }
}
