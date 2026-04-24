use std::{fs, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use threads_core::{Error, Result};

const KEYRING_SERVICE: &str = "threads-cli";
const KEYRING_USER: &str = "default";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    pub issued_at: DateTime<Utc>,
}

impl Token {
    pub fn new(access_token: impl Into<String>, expires_in: Option<i64>) -> Self {
        Self {
            access_token: access_token.into(),
            expires_in,
            issued_at: Utc::now(),
        }
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_in {
            Some(secs) if secs > 0 => {
                let elapsed = Utc::now().signed_duration_since(self.issued_at).num_seconds();
                elapsed >= secs
            }
            _ => false,
        }
    }
}

/// Persists an access [`Token`] across runs.
///
/// Prefers the OS keyring (`keyring` crate, service = "threads-cli"). When
/// keyring access fails, falls back to a JSON file at
/// `~/.config/threads-cli/token.json`.
pub struct TokenStore {
    fallback_path: PathBuf,
}

impl TokenStore {
    pub fn new() -> Self {
        let fallback_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("threads-cli")
            .join("token.json");
        Self { fallback_path }
    }

    pub fn with_fallback_path(mut self, path: PathBuf) -> Self {
        self.fallback_path = path;
        self
    }

    pub fn save(&self, token: &Token) -> Result<()> {
        let json = serde_json::to_string(token)?;
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if entry.set_password(&json).is_ok() {
                return Ok(());
            }
        }
        // Fallback: file on disk.
        if let Some(parent) = self.fallback_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Config(format!("creating token dir: {e}")))?;
        }
        fs::write(&self.fallback_path, json)
            .map_err(|e| Error::Config(format!("writing token file: {e}")))?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Token>> {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if let Ok(s) = entry.get_password() {
                let t: Token = serde_json::from_str(&s)?;
                return Ok(Some(t));
            }
        }
        if self.fallback_path.exists() {
            let s = fs::read_to_string(&self.fallback_path)
                .map_err(|e| Error::Config(format!("reading token file: {e}")))?;
            let t: Token = serde_json::from_str(&s)?;
            return Ok(Some(t));
        }
        Ok(None)
    }

    pub fn clear(&self) -> Result<()> {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            let _ = entry.delete_credential();
        }
        if self.fallback_path.exists() {
            fs::remove_file(&self.fallback_path)
                .map_err(|e| Error::Config(format!("removing token file: {e}")))?;
        }
        Ok(())
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_via_file_fallback() {
        // Force file fallback by pointing at a tempdir. Keyring may succeed
        // too; this still covers the Ok path.
        let tmp = TempDir::new().unwrap();
        let store = TokenStore::new().with_fallback_path(tmp.path().join("token.json"));
        let t = Token::new("abcd", Some(3600));
        store.save(&t).unwrap();
        let loaded = store.load().unwrap().expect("token should load");
        assert_eq!(loaded.access_token, "abcd");
        store.clear().unwrap();
    }

    #[test]
    fn expiry_detection() {
        let mut t = Token::new("x", Some(1));
        t.issued_at = Utc::now() - chrono::Duration::seconds(10);
        assert!(t.is_expired());
        let t2 = Token::new("y", Some(3600));
        assert!(!t2.is_expired());
    }
}
