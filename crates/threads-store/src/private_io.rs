use std::path::{Path, PathBuf};

use crate::{Result, StoreError};

#[cfg(unix)]
#[path = "private_io_unix.rs"]
mod unix;

#[cfg(unix)]
pub(crate) use unix::prepare_database_path;

#[cfg(all(test, unix))]
use unix::{
    effective_user_id, has_expected_owner, is_trusted_ancestor, normalize_existing_database,
    open_existing_database, sqlite_sidecar,
};

pub(crate) fn normalize_database_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(StoreError::Io)?.join(path)
    };
    Ok(normalize_macos_system_alias(&absolute))
}

fn normalize_macos_system_alias(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(suffix) = path.strip_prefix("/var") {
            return Path::new("/private/var").join(suffix);
        }
        if let Ok(suffix) = path.strip_prefix("/tmp") {
            return Path::new("/private/tmp").join(suffix);
        }
    }
    path.to_path_buf()
}

#[cfg(not(unix))]
pub(crate) fn prepare_database_path(path: &Path) -> Result<()> {
    let _ = path;
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "private_io_tests.rs"]
mod tests;
