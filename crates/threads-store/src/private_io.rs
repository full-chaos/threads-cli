use std::{io, path::Path};

use crate::{Result, StoreError};

pub(crate) fn prepare_database_path(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        create_safe_parent(path)?;
        create_or_validate_database(path)
            .and_then(|()| create_or_validate_database(&sqlite_sidecar(path, "-wal")))
            .and_then(|()| normalize_existing_database_if_present(&sqlite_sidecar(path, "-shm")))
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

    match open_existing_database(path) {
        Ok(file) => normalize_existing_database(path, &file, tighten_permissions),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map(|_| ())
            .map_err(StoreError::Io),
        Err(error) => Err(StoreError::Io(error)),
    }
}

#[cfg(unix)]
fn normalize_existing_database_if_present(path: &Path) -> Result<()> {
    match open_existing_database(path) {
        Ok(file) => normalize_existing_database(path, &file, tighten_permissions),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

#[cfg(unix)]
fn open_existing_database(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(unix)]
fn normalize_existing_database<F>(
    path: &Path,
    file: &std::fs::File,
    change_permissions: F,
) -> Result<()>
where
    F: FnOnce(&std::fs::File) -> io::Result<()>,
{
    use std::os::unix::fs::PermissionsExt;

    let metadata = file.metadata().map_err(StoreError::Io)?;
    validate_existing_database(path, &metadata)?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        change_permissions(file).map_err(StoreError::Io)?;
    }
    let metadata = file.metadata().map_err(StoreError::Io)?;
    validate_existing_database(path, &metadata)?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return unsafe_path(path, "database permissions could not be set to 0600");
    }
    Ok(())
}

#[cfg(unix)]
fn tighten_permissions(file: &std::fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: [Category 8 — FFI boundary] `File` guarantees a valid owned descriptor for fchmod.
    let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o600) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
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
fn validate_existing_database(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.is_file() {
        return unsafe_path(path, "database is not a regular file");
    }
    if !has_expected_owner(metadata.uid(), effective_user_id()) {
        return unsafe_path(path, "database is not owned by the current user");
    }
    Ok(())
}

#[cfg(unix)]
fn has_expected_owner(owner_uid: u32, expected_uid: u32) -> bool {
    owner_uid == expected_uid
}

#[cfg(unix)]
fn effective_user_id() -> u32 {
    // SAFETY: [Category 8 — FFI boundary] geteuid has no pointer preconditions.
    unsafe { libc::geteuid() }
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

    use super::{
        has_expected_owner, normalize_existing_database, open_existing_database,
        prepare_database_path, sqlite_sidecar,
    };

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

    #[test]
    fn existing_owned_database_mode_0644_is_normalized_without_data_loss() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");
        let store = crate::Store::open(&path).unwrap();
        store
            .raw_conn()
            .execute_batch("CREATE TABLE retained_data (value TEXT); INSERT INTO retained_data VALUES ('kept');")
            .unwrap();
        drop(store);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let reopened = crate::Store::open(&path).unwrap();

        assert_eq!(
            reopened
                .raw_conn()
                .query_row("SELECT value FROM retained_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "kept"
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn existing_owned_sqlite_sidecars_mode_0644_are_normalized() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");
        let store = crate::Store::open(&path).unwrap();
        let wal = sqlite_sidecar(&path, "-wal");
        let shm = sqlite_sidecar(&path, "-shm");
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&shm, fs::Permissions::from_mode(0o644)).unwrap();

        let reopened = crate::Store::open(&path).unwrap();

        assert_eq!(
            fs::metadata(wal).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(shm).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(reopened);
        drop(store);
    }

    #[test]
    fn existing_owned_database_noncanonical_owner_modes_are_normalized() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");
        prepare_database_path(&path).unwrap();

        for mode in [0o400, 0o700] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            prepare_database_path(&path).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn foreign_owner_policy_rejects_a_different_uid() {
        assert!(!has_expected_owner(501, 502));
    }

    #[test]
    fn permission_change_failure_propagates() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");
        prepare_database_path(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let file = open_existing_database(&path).unwrap();

        let error = normalize_existing_database(&path, &file, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission change denied",
            ))
        })
        .unwrap_err();

        assert!(
            matches!(error, crate::StoreError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
    }
}
