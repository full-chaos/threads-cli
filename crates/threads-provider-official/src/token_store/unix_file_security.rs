use std::{
    ffi::CString,
    fs,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
};

use threads_core::{Error, Result};

pub(super) fn open_private_read_only_file(path: &Path) -> Result<Option<fs::File>> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Config(format!("token path {} has no parent", path.display())))?;
    validate_private_ancestor_chain(parent)?;
    let parent_path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|_| Error::Config(format!("unsafe token directory {}", parent.display())))?;
    // SAFETY: [Category 8 — FFI boundary] The C string is valid for the call.
    let parent_fd = unsafe {
        libc::open(
            parent_path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if parent_fd < 0 {
        return open_error(parent, "directory");
    }
    // SAFETY: `open` returned an owned file descriptor.
    let parent_file = unsafe { fs::File::from_raw_fd(parent_fd) };
    validate_private_directory_metadata(
        &parent_file
            .metadata()
            .map_err(|error| Error::Config(format!("stat token directory: {error}")))?,
        effective_user_id(),
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Config(format!("token path {} has no file name", path.display())))?;
    let name = CString::new(name.as_bytes())
        .map_err(|_| Error::Config(format!("unsafe token file {}", path.display())))?;
    // SAFETY: [Category 8 — FFI boundary] The directory descriptor and C string are valid.
    let file_fd = unsafe {
        libc::openat(
            parent_file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if file_fd < 0 {
        return open_error(path, "file");
    }
    // SAFETY: `openat` returned an owned file descriptor.
    Ok(Some(unsafe { fs::File::from_raw_fd(file_fd) }))
}

fn open_error(path: &Path, kind: &str) -> Result<Option<fs::File>> {
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(Error::Config(format!(
            "unsafe token {kind} {}: {error}",
            path.display()
        )))
    }
}

pub(super) fn effective_user_id() -> u32 {
    // SAFETY: [Category 8 — FFI boundary] `geteuid` has no pointer preconditions.
    unsafe { libc::geteuid() }
}

pub(super) fn validate_private_directory_metadata(
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<()> {
    let mode = metadata.mode() & 0o777;
    if !metadata.is_dir() || metadata.uid() != expected_uid || mode & 0o022 != 0 {
        return Err(Error::Config(format!(
            "unsafe token directory: expected an owner-only directory without group or world write permission (uid {}, mode {mode:o})",
            metadata.uid()
        )));
    }
    Ok(())
}

pub(super) fn validate_private_file_metadata(
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<()> {
    let mode = metadata.mode() & 0o777;
    if !metadata.is_file() || metadata.uid() != expected_uid || mode & 0o077 != 0 {
        return Err(Error::Config(format!(
            "unsafe token file: expected an owner-only regular file (uid {}, mode {mode:o})",
            metadata.uid()
        )));
    }
    Ok(())
}

pub(super) fn validate_existing_private_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_private_file_metadata(&metadata, effective_user_id()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Config(format!("stat token file: {error}"))),
    }
}

pub(super) fn validate_private_ancestor_chain(parent: &Path) -> Result<()> {
    validate_private_directory_metadata(
        &fs::symlink_metadata(parent)
            .map_err(|error| Error::Config(format!("stat token directory: {error}")))?,
        effective_user_id(),
    )?;
    for ancestor in parent.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|error| Error::Config(format!("stat token ancestor: {error}")))?;
        let trusted_system_alias = metadata.file_type().is_symlink()
            && matches!(ancestor.to_str(), Some("/tmp") | Some("/var"));
        if trusted_system_alias {
            continue;
        }
        let mode = metadata.mode();
        let trusted_sticky_root =
            metadata.is_dir() && metadata.uid() == 0 && mode & 0o1000 != 0 && mode & 0o022 != 0;
        if !is_trusted_ancestor_owner(metadata.uid())
            || metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || (mode & 0o022 != 0 && !trusted_sticky_root)
        {
            return Err(Error::Config(format!(
                "unsafe token ancestor {}",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn is_trusted_ancestor_owner(owner_uid: u32) -> bool {
    owner_uid == effective_user_id() || owner_uid == 0
}
