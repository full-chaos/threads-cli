mod credential_backend;
mod file_store;
mod token_model;

use std::{path::PathBuf, sync::Arc};

#[cfg(feature = "test-support")]
use credential_backend::FileOnlyBackend;
use credential_backend::{CredentialBackend, KeyringBackend};
use threads_core::{Error, Result};

pub use token_model::{Token, token_has_scope};

pub struct TokenStore {
    fallback_path: PathBuf,
    backend: Arc<dyn CredentialBackend>,
}

impl TokenStore {
    pub fn new() -> Self {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            });
        Self {
            fallback_path: config_home.join("threads-cli").join("token.json"),
            backend: Arc::new(KeyringBackend),
        }
    }

    pub fn with_fallback_path(mut self, path: PathBuf) -> Self {
        self.fallback_path = path;
        self
    }

    #[cfg(feature = "test-support")]
    pub fn file_only_for_tests(self) -> Self {
        self.with_backend(Arc::new(FileOnlyBackend))
    }

    pub fn save(&self, token: &Token) -> Result<()> {
        let json = serde_json::to_string(token)?;
        file_store::write_private_file(&self.fallback_path, json.as_bytes())?;
        let _ = self.backend.save(&json);
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Token>> {
        if let Some(json) = file_store::read_private_file(&self.fallback_path)? {
            return serde_json::from_str(&json).map(Some).map_err(Error::from);
        }
        match self.backend.load() {
            Ok(Some(json)) => serde_json::from_str(&json).map(Some).map_err(Error::from),
            Ok(None) | Err(_) => Ok(None),
        }
    }

    pub fn clear(&self) -> Result<()> {
        let backend_result = self
            .backend
            .clear()
            .map_err(|error| Error::Config(format!("clearing keyring token: {error}")));
        let fallback_result = file_store::remove_private_file(&self.fallback_path);

        match (backend_result, fallback_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn with_backend(mut self, backend: Arc<dyn CredentialBackend>) -> Self {
        self.backend = backend;
        self
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::token_store::credential_backend::CredentialResult;
    use tempfile::TempDir;

    struct FailingBackend;

    impl CredentialBackend for FailingBackend {
        fn save(&self, _: &str) -> CredentialResult<()> {
            Err("unavailable".into())
        }

        fn load(&self) -> CredentialResult<Option<String>> {
            Err("unavailable".into())
        }

        fn clear(&self) -> CredentialResult<()> {
            Err("unavailable".into())
        }
    }

    struct StaleBackend;

    impl CredentialBackend for StaleBackend {
        fn save(&self, _: &str) -> CredentialResult<()> {
            Ok(())
        }

        fn load(&self) -> CredentialResult<Option<String>> {
            Ok(Some(
                r#"{"access_token":"stale","issued_at":"2026-01-01T00:00:00Z"}"#.into(),
            ))
        }

        fn clear(&self) -> CredentialResult<()> {
            Err("unavailable".into())
        }
    }

    struct InMemoryBackend(Mutex<Option<String>>);

    impl CredentialBackend for InMemoryBackend {
        fn save(&self, secret: &str) -> CredentialResult<()> {
            *self.0.lock().map_err(|error| error.to_string())? = Some(secret.into());
            Ok(())
        }

        fn load(&self) -> CredentialResult<Option<String>> {
            Ok(self.0.lock().map_err(|error| error.to_string())?.clone())
        }

        fn clear(&self) -> CredentialResult<()> {
            self.0.lock().map_err(|error| error.to_string())?.take();
            Ok(())
        }
    }

    struct AccessTrackingBackend(AtomicUsize);

    impl CredentialBackend for AccessTrackingBackend {
        fn save(&self, _: &str) -> CredentialResult<()> {
            Ok(())
        }

        fn load(&self) -> CredentialResult<Option<String>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }

        fn clear(&self) -> CredentialResult<()> {
            Ok(())
        }
    }

    #[test]
    fn save_keeps_file_fallback_when_keyring_write_fails() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("token.json");
        let store = TokenStore::new()
            .with_fallback_path(path.clone())
            .with_backend(Arc::new(FailingBackend));

        store.save(&Token::new("current", None, None)).unwrap();

        assert!(path.exists());
        assert_eq!(store.load().unwrap().unwrap().access_token, "current");
    }

    #[test]
    fn load_returns_none_when_keyring_read_fails_without_a_file() {
        let temp = TempDir::new().unwrap();
        let store = TokenStore::new()
            .with_fallback_path(temp.path().join("token.json"))
            .with_backend(Arc::new(FailingBackend));

        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn clear_removes_fallback_when_credential_backend_delete_fails() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("token.json");
        let store = TokenStore::new()
            .with_fallback_path(path.clone())
            .with_backend(Arc::new(StaleBackend));
        store.save(&Token::new("current", None, None)).unwrap();

        let error = store.clear().expect_err("backend failure must be reported");
        assert!(
            matches!(error, Error::Config(message) if message.contains("clearing keyring token: unavailable"))
        );
        assert!(
            !path.exists(),
            "fallback token must be removed despite backend failure"
        );
    }

    #[test]
    fn clear_prefers_credential_backend_error_when_both_cleanup_steps_fail() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("token.json");
        std::fs::create_dir(&path).unwrap();
        let store = TokenStore::new()
            .with_fallback_path(path.clone())
            .with_backend(Arc::new(FailingBackend));

        let error = store
            .clear()
            .expect_err("both cleanup failures must be reported");

        assert!(
            error
                .to_string()
                .contains("clearing keyring token: unavailable")
        );
        assert!(path.is_dir());
    }

    #[test]
    fn roundtrip_via_file_fallback() {
        let temp = TempDir::new().unwrap();
        let store = TokenStore::new()
            .with_fallback_path(temp.path().join("token.json"))
            .with_backend(Arc::new(InMemoryBackend(Mutex::new(None))));
        let token = Token::new("abcd", Some(3600), Some(vec!["threads_basic".into()]));
        store.save(&token).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.access_token, "abcd");
        store.clear().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_unsafe_file_without_accessing_credential_backend() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let victim = temp.path().join("victim.json");
        let path = temp.path().join("token.json");
        std::fs::write(&victim, r#"{"access_token":"victim"}"#).unwrap();
        symlink(&victim, &path).unwrap();
        let backend = Arc::new(AccessTrackingBackend(AtomicUsize::new(0)));
        let store = TokenStore::new()
            .with_fallback_path(path)
            .with_backend(backend.clone());

        let error = store.load().expect_err("unsafe token file must fail load");

        assert!(error.to_string().contains("unsafe token file"));
        assert_eq!(backend.0.load(Ordering::Relaxed), 0);
    }
}
