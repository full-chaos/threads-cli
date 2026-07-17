use std::{
    fs, io,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use crate::{Result, StoreError};

const PRIVATE_MODE: u32 = 0o600;
const FULL_MODE_MASK: u32 = 0o7777;

pub(crate) fn prepare_database_path(path: &Path) -> Result<()> {
    let path = crate::private_io::normalize_database_path(path)?;
    create_safe_parent(&path)?;
    create_or_validate_database(&path)
        .and_then(|()| create_or_validate_database(&sqlite_sidecar(&path, "-wal")))
        .and_then(|()| normalize_existing_database_if_present(&sqlite_sidecar(&path, "-shm")))
}

fn create_safe_parent(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut missing = Vec::new();
    let mut current = parent;

    loop {
        match fs::symlink_metadata(current) {
            Ok(_) if missing.is_empty() => {
                validate_private_ancestor_chain(current)?;
                break;
            }
            Ok(_) => {
                validate_existing_ancestor_chain(current)?;
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
    }
    validate_private_ancestor_chain(parent)
}

fn create_or_validate_database(path: &Path) -> Result<()> {
    match open_existing_database(path) {
        Ok(file) => normalize_existing_database(path, &file, tighten_permissions),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_database(path),
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn create_database(path: &Path) -> Result<()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let file = open_existing_database(path).map_err(StoreError::Io)?;
            normalize_existing_database(path, &file, tighten_permissions)
        }
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn normalize_existing_database_if_present(path: &Path) -> Result<()> {
    match open_existing_database(path) {
        Ok(file) => normalize_existing_database(path, &file, tighten_permissions),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

/// Opens a legacy file without following it and without blocking on special files.
///
/// Owner-readable files use the read-only descriptor. Owner-write-only files use
/// the write-only fallback. Files with neither access mode, such as `0000`, are
/// rejected; this module never repairs permissions through a pathname.
pub(super) fn open_existing_database(path: &Path) -> io::Result<fs::File> {
    open_existing_database_with_access(path, true).or_else(|error| {
        if error.kind() == io::ErrorKind::PermissionDenied {
            open_existing_database_with_access(path, false)
        } else {
            Err(error)
        }
    })
}

fn open_existing_database_with_access(path: &Path, readable: bool) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options
        .read(readable)
        .write(!readable)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path)
}

pub(super) fn normalize_existing_database<F>(
    path: &Path,
    file: &fs::File,
    change_permissions: F,
) -> Result<()>
where
    F: FnOnce(&fs::File) -> io::Result<()>,
{
    let metadata = file.metadata().map_err(StoreError::Io)?;
    validate_existing_database(path, &metadata)?;
    if private_mode(&metadata) != PRIVATE_MODE {
        change_permissions(file).map_err(StoreError::Io)?;
    }
    let metadata = file.metadata().map_err(StoreError::Io)?;
    validate_existing_database(path, &metadata)?;
    if private_mode(&metadata) != PRIVATE_MODE {
        return unsafe_path(path, "database permissions could not be set to 0600");
    }
    Ok(())
}

fn tighten_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: [Category 8 — FFI boundary] `File` owns a valid descriptor, and
    // `fchmod` only receives that descriptor plus the constant permission mask.
    let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o600) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn validate_private_ancestor_chain(parent: &Path) -> Result<()> {
    let mut ancestors = parent.ancestors();
    let immediate = ancestors.next().ok_or_else(|| {
        StoreError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("parent is missing for {}", parent.display()),
        ))
    })?;
    validate_directory(immediate, true)?;
    for ancestor in ancestors {
        validate_directory(ancestor, false)?;
    }
    Ok(())
}

fn validate_existing_ancestor_chain(existing: &Path) -> Result<()> {
    for ancestor in existing.ancestors() {
        validate_directory(ancestor, false)?;
    }
    Ok(())
}

fn validate_directory(path: &Path, immediate: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(StoreError::Io)?;
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_symlink() {
        return unsafe_path(path, "parent is a symlink");
    }
    if !metadata.is_dir() {
        return unsafe_path(path, "parent is not a directory");
    }
    let expected_uid = effective_user_id();
    let owner_uid = metadata.uid();
    let mode = private_mode(&metadata);
    if immediate {
        if !has_expected_owner(owner_uid, expected_uid) || mode & 0o022 != 0 {
            return unsafe_path(path, "immediate parent is not private");
        }
    } else if !is_trusted_ancestor(owner_uid, mode, expected_uid) {
        return unsafe_path(path, "ancestor is not trusted");
    }
    Ok(())
}

fn validate_existing_database(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.is_file() {
        return unsafe_path(path, "database is not a regular file");
    }
    if !has_expected_owner(metadata.uid(), effective_user_id()) {
        return unsafe_path(path, "database is not owned by the current user");
    }
    Ok(())
}

fn private_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & FULL_MODE_MASK
}

pub(super) fn has_expected_owner(owner_uid: u32, expected_uid: u32) -> bool {
    owner_uid == expected_uid
}

pub(super) fn is_trusted_ancestor(owner_uid: u32, mode: u32, expected_uid: u32) -> bool {
    let root_owned_sticky = owner_uid == 0 && mode & 0o1000 != 0;
    (has_expected_owner(owner_uid, expected_uid) || owner_uid == 0)
        && (mode & 0o022 == 0 || root_owned_sticky)
}

pub(super) fn effective_user_id() -> u32 {
    // SAFETY: [Category 8 — FFI boundary] `geteuid` has no pointer arguments or
    // memory preconditions and returns the process effective user identifier.
    unsafe { libc::geteuid() }
}

fn unsafe_path(path: &Path, reason: &str) -> Result<()> {
    Err(StoreError::Io(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("unsafe path {}: {reason}", path.display()),
    )))
}
