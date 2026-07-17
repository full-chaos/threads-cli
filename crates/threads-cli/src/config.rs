use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

mod config_persistence;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CliConfig {
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub redirect_uri: Option<String>,
    #[serde(default = "CliConfig::default_db_path_string")]
    pub db_path: String,
}

impl CliConfig {
    /// XDG config root: `$XDG_CONFIG_HOME` if set, else `~/.config`.
    ///
    /// We intentionally DON'T use `dirs::config_dir()` because on macOS it
    /// returns `~/Library/Application Support`, which violates the XDG Base
    /// Directory spec. A CLI moving between macOS and Linux should put its
    /// config in the same place on both.
    fn xdg_config_home() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            })
    }

    /// XDG data root: `$XDG_DATA_HOME` if set, else `~/.local/share`.
    fn xdg_data_home() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local")
                    .join("share")
            })
    }

    pub fn default_config_path() -> PathBuf {
        Self::xdg_config_home()
            .join("threads-cli")
            .join("config.toml")
    }

    pub fn default_db_path() -> PathBuf {
        Self::xdg_data_home().join("threads-cli").join("store.db")
    }

    fn default_db_path_string() -> String {
        Self::default_db_path().to_string_lossy().into_owned()
    }

    pub fn token_path() -> PathBuf {
        Self::xdg_config_home()
            .join("threads-cli")
            .join("token.json")
    }

    /// Load config, applying env overrides on top of the file contents.
    /// Precedence: env > config file > defaults.
    pub fn load(cli_override: Option<&Path>) -> Result<Self> {
        let path = cli_override
            .map(Path::to_path_buf)
            .unwrap_or_else(Self::default_config_path);
        let mut cfg = match config_persistence::read_private_file(&path)? {
            Some(contents) => {
                toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?
            }
            None => Self {
                db_path: Self::default_db_path_string(),
                ..Self::default()
            },
        };
        if let Ok(v) = std::env::var("THREADS_APP_ID") {
            cfg.app_id = Some(v);
        }
        if let Ok(v) = std::env::var("THREADS_APP_SECRET") {
            cfg.app_secret = Some(v);
        }
        if let Ok(v) = std::env::var("THREADS_REDIRECT_URI") {
            cfg.redirect_uri = Some(v);
        }
        if let Ok(v) = std::env::var("THREADS_DB_PATH") {
            cfg.db_path = v;
        }
        Ok(cfg)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let s = toml::to_string_pretty(self)?;
        config_persistence::create_private_dir(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("config path {} has no parent", path.display()))?,
        )?;
        config_persistence::atomic_write_private_file(
            path,
            s.as_bytes(),
            |temporary_path, destination_path| fs::rename(temporary_path, destination_path),
        )
        .with_context(|| format!("writing {}", path.display()))
    }

    #[cfg(test)]
    fn save_to_with_failure(&self, path: &Path) -> Result<()> {
        let s = toml::to_string_pretty(self)?;
        config_persistence::create_private_dir(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("config path {} has no parent", path.display()))?,
        )?;
        config_persistence::atomic_write_with_interruption(path, s.as_bytes())
    }

    pub fn db_path(&self) -> PathBuf {
        if self.db_path.is_empty() {
            Self::default_db_path()
        } else {
            PathBuf::from(&self.db_path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn physical_temp_dir(temp: &TempDir) -> PathBuf {
        fs::canonicalize(temp.path()).unwrap()
    }

    #[test]
    fn env_overrides_file_values() {
        let tmp = TempDir::new().unwrap();
        let path = physical_temp_dir(&tmp).join("config.toml");
        let initial = CliConfig {
            app_id: Some("file-id".into()),
            app_secret: Some("file-secret".into()),
            redirect_uri: Some("file-uri".into()),
            db_path: "/tmp/file.db".into(),
        };
        initial.save_to(&path).unwrap();

        // Safety: tests in this crate are single-threaded-enough for env vars
        // since cargo runs doc+unit in separate processes. If this becomes
        // flaky, isolate with `#[serial_test]`.
        unsafe { std::env::set_var("THREADS_APP_ID", "env-id") };
        let cfg = CliConfig::load(Some(&path)).unwrap();
        unsafe { std::env::remove_var("THREADS_APP_ID") };

        assert_eq!(cfg.app_id.as_deref(), Some("env-id"));
        assert_eq!(cfg.app_secret.as_deref(), Some("file-secret"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        // Runs in parallel with env_overrides_file_values which mutates
        // process env. Explicitly unset the vars we care about so load()
        // returns the file's values deterministically.
        for k in [
            "THREADS_APP_ID",
            "THREADS_APP_SECRET",
            "THREADS_REDIRECT_URI",
            "THREADS_DB_PATH",
        ] {
            unsafe { std::env::remove_var(k) };
        }
        let tmp = TempDir::new().unwrap();
        let path = physical_temp_dir(&tmp).join("nested").join("config.toml");
        let cfg = CliConfig {
            app_id: Some("abc".into()),
            app_secret: Some("def".into()),
            redirect_uri: Some("https://localhost/cb".into()),
            db_path: "/tmp/store.db".into(),
        };
        cfg.save_to(&path).unwrap();
        let loaded = CliConfig::load(Some(&path)).unwrap();
        assert_eq!(loaded.redirect_uri, cfg.redirect_uri);
        assert_eq!(loaded.db_path, cfg.db_path);
    }

    #[test]
    fn missing_file_returns_defaults() {
        let tmp = TempDir::new().unwrap();
        let path = physical_temp_dir(&tmp).join("missing.toml");
        let cfg = CliConfig::load(Some(&path)).unwrap();
        assert!(cfg.redirect_uri.is_none());
        assert!(!cfg.db_path.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_symlinked_config_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let temp_dir = physical_temp_dir(&temp);
        let victim = temp_dir.join("victim.toml");
        fs::write(&victim, "app_id = 'victim'\n").unwrap();
        let config = temp_dir.join("config.toml");
        symlink(&victim, &config).unwrap();

        assert!(CliConfig::load(Some(&config)).is_err());
        assert_eq!(fs::read_to_string(victim).unwrap(), "app_id = 'victim'\n");
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_symlinked_config_parent() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let temp_dir = physical_temp_dir(&temp);
        let victim_dir = temp_dir.join("victim");
        fs::create_dir(&victim_dir).unwrap();
        fs::write(victim_dir.join("config.toml"), "app_id = 'victim'\n").unwrap();
        let config_dir = temp_dir.join("config");
        symlink(&victim_dir, &config_dir).unwrap();

        assert!(CliConfig::load(Some(&config_dir.join("config.toml"))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_group_or_world_writable_config_parent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let config_dir = physical_temp_dir(&temp).join("config");
        fs::create_dir(&config_dir).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o775)).unwrap();
        let config = config_dir.join("config.toml");
        fs::write(&config, "app_id = 'unsafe-parent'\n").unwrap();

        assert!(CliConfig::load(Some(&config)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_group_or_world_writable_config_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let ancestor = physical_temp_dir(&temp).join("unsafe-ancestor");
        fs::create_dir(&ancestor).unwrap();
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o775)).unwrap();
        let config_dir = ancestor.join("config");
        fs::create_dir(&config_dir).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let config = config_dir.join("config.toml");
        fs::write(&config, "app_id = 'unsafe-ancestor'\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(CliConfig::load(Some(&config)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn save_rejects_group_or_world_writable_config_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let ancestor = physical_temp_dir(&temp).join("unsafe-ancestor");
        fs::create_dir(&ancestor).unwrap();
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o775)).unwrap();
        let config_dir = ancestor.join("config");
        fs::create_dir(&config_dir).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            CliConfig::default()
                .save_to(&config_dir.join("config.toml"))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_group_or_world_readable_config_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let config = physical_temp_dir(&temp).join("config.toml");
        fs::write(&config, "app_id = 'unsafe-file'\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();

        assert!(CliConfig::load(Some(&config)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_accepts_private_file_in_safe_parent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let config_dir = physical_temp_dir(&temp).join("config");
        fs::create_dir(&config_dir).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let config = config_dir.join("config.toml");
        fs::write(&config, "redirect_uri = 'https://safe.test/callback'\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();

        let loaded = CliConfig::load(Some(&config)).unwrap();

        assert_eq!(
            loaded.redirect_uri.as_deref(),
            Some("https://safe.test/callback")
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_accepts_private_file_below_root_owned_sticky_tmp() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let tmp = fs::canonicalize("/tmp").unwrap();
        let metadata = fs::metadata(&tmp).unwrap();
        assert_eq!(metadata.uid(), 0);
        assert_ne!(metadata.permissions().mode() & 0o1000, 0);

        let temp = TempDir::new_in(&tmp).unwrap();
        let config = temp.path().join("config.toml");
        fs::write(&config, "redirect_uri = 'https://sticky.test/callback'\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();

        let loaded = CliConfig::load(Some(&config)).unwrap();

        assert_eq!(
            loaded.redirect_uri.as_deref(),
            Some("https://sticky.test/callback")
        );
    }

    #[test]
    fn atomic_save_keeps_previous_config_bytes_when_replacement_is_interrupted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"app_id = \"old\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let replacement = CliConfig {
            app_id: Some("new".to_string()),
            ..CliConfig::default()
        };

        let result = replacement.save_to_with_failure(&path);

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"app_id = \"old\"\n");
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_existing_custom_parent_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let parent = temp.path().join("custom-parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        let path = parent.join("config.toml");

        CliConfig::default().save_to(&path).unwrap();

        let mode = fs::metadata(parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        let file_mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn save_to_rejects_symlinked_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target.toml");
        fs::write(&target, "app_id = 'original'\n").unwrap();
        let path = temp.path().join("config.toml");
        symlink(&target, &path).unwrap();

        assert!(CliConfig::default().save_to(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "app_id = 'original'\n");
    }

    #[cfg(unix)]
    #[test]
    fn save_to_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let parent = temp.path().join("config-dir");
        symlink(&target, &parent).unwrap();

        let error = CliConfig::default()
            .save_to(&parent.join("config.toml"))
            .unwrap_err();

        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );
        assert!(!target.join("config.toml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_to_rejects_group_writable_existing_parent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let parent = temp.path().join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o770)).unwrap();

        let error = CliConfig::default()
            .save_to(&parent.join("config.toml"))
            .unwrap_err();

        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_to_creates_private_config_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");

        CliConfig::default().save_to(&path).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
