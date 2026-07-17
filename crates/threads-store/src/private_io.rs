use std::{io, path::Path};

use crate::{Result, StoreError};

pub(crate) fn prepare_database_path(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        create_safe_parent(path)?;
        create_or_validate_database(path)
            .and_then(|()| create_or_validate_database(&sqlite_sidecar(path, "-wal")))
            .and_then(|()| validate_existing_database(&sqlite_sidecar(path, "-shm")))
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(unix)]
fn create_safe_parent(path: &Path) -> Result<()> {
    use std::{fs, os::unix::fs::DirBuilderExt};

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut missing = Vec::new();
    let mut current = parent;

    loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                validate_parent(current, &metadata)?;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(current.to_path_buf());
                current = current.parent().ok_or_else(|| {
                    StoreError::Io(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no existing parent for {}", path.display()),
                    ))
                })?;
            }
            Err(error) => return Err(StoreError::Io(error)),
        }
    }

    for directory in missing.iter().rev() {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StoreError::Io(error)),
        }
        validate_parent(
            directory,
            &fs::symlink_metadata(directory).map_err(StoreError::Io)?,
        )?;
    }

    Ok(())
}

#[cfg(unix)]
fn create_or_validate_database(path: &Path) -> Result<()> {
    use std::{fs, os::unix::fs::OpenOptionsExt};

    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_database(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map(|_| ())
            .map_err(StoreError::Io),
        Err(error) => Err(StoreError::Io(error)),
    }
}

#[cfg(unix)]
fn validate_existing_database(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_database(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

#[cfg(unix)]
fn sqlite_sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    std::path::PathBuf::from(sidecar)
}

#[cfg(unix)]
fn validate_parent(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.file_type().is_symlink() {
        return unsafe_path(path, "parent is a symlink");
    }
    if !metadata.is_dir() {
        return unsafe_path(path, "parent is not a directory");
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        return unsafe_path(path, "parent is group or world writable");
    }
    Ok(())
}

#[cfg(unix)]
fn validate_database(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.file_type().is_symlink() {
        return unsafe_path(path, "database is a symlink");
    }
    if !metadata.is_file() {
        return unsafe_path(path, "database is not a regular file");
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return unsafe_path(path, "database is readable or writable by group or world");
    }
    Ok(())
}

#[cfg(unix)]
fn unsafe_path(path: &Path, reason: &str) -> Result<()> {
    Err(StoreError::Io(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("unsafe path {}: {reason}", path.display()),
    )))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::TempDir;

    use super::prepare_database_path;

    #[test]
    fn newly_created_database_is_private_before_sqlite_opens() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");

        prepare_database_path(&path).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
