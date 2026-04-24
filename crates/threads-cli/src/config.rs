use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
        Self::xdg_config_home().join("threads-cli").join("config.toml")
    }

    pub fn default_db_path() -> PathBuf {
        Self::xdg_data_home().join("threads-cli").join("store.db")
    }

    fn default_db_path_string() -> String {
        Self::default_db_path().to_string_lossy().into_owned()
    }

    pub fn token_path() -> PathBuf {
        Self::xdg_config_home().join("threads-cli").join("token.json")
    }

    /// Load config, applying env overrides on top of the file contents.
    /// Precedence: env > config file > defaults.
    pub fn load(cli_override: Option<&Path>) -> Result<Self> {
        let path = cli_override
            .map(Path::to_path_buf)
            .unwrap_or_else(Self::default_config_path);
        let mut cfg = if path.exists() {
            let s = fs::read_to_string(&path)
                .with_context(|| format!("reading config at {}", path.display()))?;
            toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Self {
                db_path: Self::default_db_path_string(),
                ..Self::default()
            }
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let s = toml::to_string_pretty(self)?;
        fs::write(path, s).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
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

    #[test]
    fn env_overrides_file_values() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
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
        for k in ["THREADS_APP_ID", "THREADS_APP_SECRET", "THREADS_REDIRECT_URI", "THREADS_DB_PATH"] {
            unsafe { std::env::remove_var(k) };
        }
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("config.toml");
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
        let path = tmp.path().join("missing.toml");
        let cfg = CliConfig::load(Some(&path)).unwrap();
        assert!(cfg.app_id.is_none());
        assert!(!cfg.db_path.is_empty());
    }
}
