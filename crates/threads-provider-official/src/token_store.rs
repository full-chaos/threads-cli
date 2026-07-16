use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

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
    /// Meta's token-exchange endpoint does not return the granted scopes; we
    /// record what we requested at login time. This is a heuristic, not a
    /// guarantee — if a request fails with 403/insufficient_scope, re-run
    /// `auth login`.
    #[serde(
        default,
        alias = "granted_scopes",
        skip_serializing_if = "Option::is_none"
    )]
    pub requested_scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub issued_at: DateTime<Utc>,
}

impl Token {
    pub fn new(
        access_token: impl Into<String>,
        expires_in: Option<i64>,
        requested_scopes: Option<Vec<String>>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            expires_in,
            requested_scopes,
            user_id: None,
            issued_at: Utc::now(),
        }
    }

    pub fn with_user_id(mut self, user_id: Option<String>) -> Self {
        self.user_id = user_id;
        self
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_in {
            Some(secs) if secs > 0 => {
                let elapsed = Utc::now()
                    .signed_duration_since(self.issued_at)
                    .num_seconds();
                elapsed >= secs
            }
            _ => false,
        }
    }
}

/// Strict scope check: returns `true` ONLY when the token records a
/// `requested_scopes` list and that list contains `scope`.
///
/// Tokens minted before scope-tracking shipped (`requested_scopes = None`) are
/// treated as missing the scope. This is intentionally strict for write
/// operations like `threads_delete` — those scopes were added in the same
/// release as scope tracking, so a `None` token by definition does not have
/// them. Pre-existing read-only behavior continues to work because read
/// endpoints don't call this helper.
pub fn token_has_scope(token: &Token, scope: &str) -> bool {
    token
        .requested_scopes
        .as_ref()
        .is_some_and(|scopes| scopes.iter().any(|s| s == scope))
}

/// Persists an access [`Token`] across runs.
///
/// Persists atomically so stale keyring entries cannot override replacements.
///
/// The fallback file is located at
/// `~/.config/threads-cli/token.json` with strict permissions (0700 on the
/// parent directory, 0600 on the file itself) on Unix.
pub struct TokenStore {
    fallback_path: PathBuf,
}

impl TokenStore {
    pub fn new() -> Self {
        // Use XDG config home (same logic as threads-cli's CliConfig) so the
        // token lives alongside config.toml at ~/.config/threads-cli/ on every
        // OS, instead of macOS's `~/Library/Application Support`.
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            });
        let fallback_path = config_home.join("threads-cli").join("token.json");
        Self { fallback_path }
    }

    pub fn with_fallback_path(mut self, path: PathBuf) -> Self {
        self.fallback_path = path;
        self
    }

    pub fn save(&self, token: &Token) -> Result<()> {
        let json = serde_json::to_string(token)?;
        if let Some(parent) = self.fallback_path.parent() {
            create_private_dir(parent)?;
        }
        write_private_file(&self.fallback_path, json.as_bytes())?;
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            let _ = entry.set_password(&json);
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Token>> {
        if self.fallback_path.exists() {
            warn_if_world_readable(&self.fallback_path);
            let s = fs::read_to_string(&self.fallback_path)
                .map_err(|e| Error::Config(format!("reading token file: {e}")))?;
            let t: Token = serde_json::from_str(&s)?;
            return Ok(Some(t));
        }
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if let Ok(s) = entry.get_password() {
                let t: Token = serde_json::from_str(&s)?;
                return Ok(Some(t));
            }
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

// --------------- platform-specific private I/O helpers ---------------

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if path.exists() {
        // Tighten permissions defensively; a pre-existing world-readable dir
        // would expose our token file even if the file itself is 0600.
        let mut perms = fs::metadata(path)
            .map_err(|e| Error::Config(format!("stat token dir: {e}")))?
            .permissions();
        if perms.mode() & 0o077 != 0 {
            perms.set_mode(0o700);
            fs::set_permissions(path, perms)
                .map_err(|e| Error::Config(format!("chmod token dir: {e}")))?;
        }
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| Error::Config(format!("creating token dir: {e}")))
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    // On Windows, file permissions rely on the NTFS ACL inherited from the
    // user profile; keyring is the primary store there anyway.
    fs::create_dir_all(path).map_err(|e| Error::Config(format!("creating token dir: {e}")))
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_private_file(path, bytes, |temporary_path, destination_path| {
        fs::rename(temporary_path, destination_path)
    })
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_private_file(path, bytes, |temporary_path, destination_path| {
        fs::rename(temporary_path, destination_path)
    })
}

fn atomic_write_private_file<F>(path: &Path, bytes: &[u8], persist: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let (mut file, temporary_path) = open_private_temporary_file(path)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|e| Error::Config(format!("writing temporary token file: {e}")))?;
        file.sync_all()
            .map_err(|e| Error::Config(format!("syncing temporary token file: {e}")))?;
        drop(file);
        persist(&temporary_path, path)
            .map_err(|e| Error::Config(format!("replacing token file atomically: {e}")))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn open_private_temporary_file(path: &Path) -> Result<(fs::File, PathBuf)> {
    const MAX_ATTEMPTS: u8 = 16;
    static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| Error::Config(format!("token path {} has no parent", path.display())))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::Config(format!("token path {} has no file name", path.display())))?
        .to_string_lossy();
    for _ in 0..MAX_ATTEMPTS {
        let sequence = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match open_private_new_file(&temporary_path) {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(Error::Config(format!(
                    "creating temporary token file {}: {error}",
                    temporary_path.display()
                )));
            }
        }
    }
    Err(Error::Config(format!(
        "could not allocate a temporary token file beside {}",
        path.display()
    )))
}

#[cfg(unix)]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
fn write_private_file_with_failure(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_private_file(path, bytes, |_, _| {
        Err(std::io::Error::other("simulated replacement interruption"))
    })
}

#[cfg(unix)]
fn warn_if_world_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = fs::metadata(path) {
        let mode = md.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            tracing::warn!(
                path = %path.display(),
                mode = format!("{mode:o}"),
                "token file is group- or world-readable; run `chmod 0600 <path>` to tighten it"
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_world_readable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_via_file_fallback() {
        let tmp = TempDir::new().unwrap();
        let store = TokenStore::new().with_fallback_path(tmp.path().join("token.json"));
        let t = Token::new("abcd", Some(3600), Some(vec!["threads_basic".into()]));
        store.save(&t).unwrap();
        let loaded = store.load().unwrap().expect("token should load");
        assert_eq!(loaded.access_token, "abcd");
        assert_eq!(
            loaded.requested_scopes.as_deref(),
            Some(&["threads_basic".to_string()][..])
        );
        store.clear().unwrap();
    }

    #[test]
    fn expiry_detection() {
        let mut t = Token::new("x", Some(1), None);
        t.issued_at = Utc::now() - chrono::Duration::seconds(10);
        assert!(t.is_expired());
        let t2 = Token::new("y", Some(3600), None);
        assert!(!t2.is_expired());
    }

    #[test]
    fn legacy_token_without_scopes_lacks_new_scopes() {
        // A token saved before scope tracking shipped has `granted_scopes = None`.
        // For new write scopes like `threads_delete`, that MUST read as missing
        // so the CLI can guide the user to re-run `auth login`.
        let t: Token =
            serde_json::from_str(r#"{"access_token":"t","issued_at":"2026-01-01T00:00:00Z"}"#)
                .unwrap();

        assert!(t.requested_scopes.is_none());
        assert!(!token_has_scope(&t, "threads_delete"));
    }

    #[test]
    fn legacy_token_json_deserializes_without_a_user_or_requested_scopes() {
        let token: Token =
            serde_json::from_str(r#"{"access_token":"t","issued_at":"2026-01-01T00:00:00Z"}"#)
                .unwrap();

        assert!(token.user_id.is_none());
        assert!(token.requested_scopes.is_none());
    }

    #[test]
    fn legacy_granted_scopes_json_deserializes_as_requested_scopes() {
        let token: Token = serde_json::from_str(
            r#"{"access_token":"t","granted_scopes":["threads_basic"],"issued_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        assert_eq!(
            token.requested_scopes.as_deref(),
            Some(&["threads_basic".to_owned()][..])
        );
    }

    #[test]
    fn token_has_scope_checks_recorded_scopes() {
        let t = Token::new(
            "t",
            None,
            Some(vec!["threads_basic".into(), "threads_delete".into()]),
        );

        assert!(token_has_scope(&t, "threads_delete"));
        assert!(!token_has_scope(&t, "threads_publish"));
    }

    #[cfg(unix)]
    #[test]
    fn file_fallback_writes_0600_file_and_0700_dir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("threads-cli");
        let file = dir.join("token.json");
        let store = TokenStore::new().with_fallback_path(file.clone());

        // Pretend keyring is unavailable by choosing a service name that
        // would fail — but keyring may still succeed on dev machines. To
        // guarantee the file write path runs, call the helpers directly.
        create_private_dir(&dir).unwrap();
        write_private_file(
            &file,
            b"{\"access_token\":\"t\",\"issued_at\":\"2026-01-01T00:00:00Z\"}",
        )
        .unwrap();

        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "parent dir should be 0700, got {dir_mode:o}"
        );

        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "token file should be 0600, got {file_mode:o}"
        );

        // Keep the store struct alive so `with_fallback_path` isn't dead-code.
        let _ = store;
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_loose_dir_is_tightened() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("loose");
        fs::create_dir_all(&dir).unwrap();
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dir, perms).unwrap();

        create_private_dir(&dir).unwrap();

        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "loose dir should have been tightened to 0700, got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_loose_file_is_tightened() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("token.json");
        fs::write(&file, b"{}").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&file, perms).unwrap();

        write_private_file(&file, b"{}").unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "loose file should have been tightened to 0600, got {mode:o}"
        );
    }

    #[test]
    fn atomic_fallback_save_keeps_previous_bytes_when_rename_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("token.json");
        fs::write(&path, b"old token bytes").unwrap();

        let result = write_private_file_with_failure(&path, b"new token bytes");

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old token bytes");
    }
}
