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
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if entry.set_password(&json).is_ok() {
                return Ok(());
            }
        }
        // Fallback: file on disk, restricted to the current user.
        if let Some(parent) = self.fallback_path.parent() {
            create_private_dir(parent)?;
        }
        write_private_file(&self.fallback_path, json.as_bytes())?;
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
            warn_if_world_readable(&self.fallback_path);
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

// --------------- platform-specific private I/O helpers ---------------

#[cfg(unix)]
fn create_private_dir(path: &std::path::Path) -> Result<()> {
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
fn create_private_dir(path: &std::path::Path) -> Result<()> {
    // On Windows, file permissions rely on the NTFS ACL inherited from the
    // user profile; keyring is the primary store there anyway.
    fs::create_dir_all(path).map_err(|e| Error::Config(format!("creating token dir: {e}")))
}

#[cfg(unix)]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| Error::Config(format!("opening token file: {e}")))?;
    f.write_all(bytes)
        .map_err(|e| Error::Config(format!("writing token file: {e}")))?;
    // Re-assert mode in case the file pre-existed with looser perms.
    let mut perms = f
        .metadata()
        .map_err(|e| Error::Config(format!("stat token file: {e}")))?
        .permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
        .map_err(|e| Error::Config(format!("chmod token file: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|e| Error::Config(format!("writing token file: {e}")))
}

#[cfg(unix)]
fn warn_if_world_readable(path: &std::path::Path) {
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
fn warn_if_world_readable(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_via_file_fallback() {
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
        write_private_file(&file, b"{\"access_token\":\"t\",\"issued_at\":\"2026-01-01T00:00:00Z\"}").unwrap();

        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "parent dir should be 0700, got {dir_mode:o}");

        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "token file should be 0600, got {file_mode:o}");

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
        assert_eq!(mode, 0o700, "loose dir should have been tightened to 0700, got {mode:o}");
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
        assert_eq!(mode, 0o600, "loose file should have been tightened to 0600, got {mode:o}");
    }
}
