use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub(super) fn read_private_file(path: &Path) -> Result<Option<String>> {
    use std::os::unix::fs::OpenOptionsExt;

    let path = normalize_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path {} has no parent", path.display()))?;
    validate_existing_parent_ancestors(parent)?;

    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_opened_file(&path, &file.metadata()?)?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("reading config at {}", path.display()))?;
    Ok(Some(contents))
}

pub(super) fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let path = normalize_path(path)?;
    let parent = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &path
    };
    let mut missing = Vec::new();
    let mut current = parent;

    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(current.to_path_buf());
                current = current.parent().ok_or_else(|| {
                    anyhow::anyhow!("no existing parent for config directory {}", path.display())
                })?;
            }
            Err(error) => return Err(error.into()),
        }
    }

    validate_existing_parent_ancestors(parent)?;

    for directory in missing.iter().rev() {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        validate_directory(directory, &fs::symlink_metadata(directory)?, true)?;
    }
    Ok(())
}

pub(super) fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

pub(super) fn validate_existing_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_private_file(path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_existing_parent_ancestors(path: &Path) -> Result<()> {
    let mut current = path;

    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = current.parent().ok_or_else(|| {
                    anyhow::anyhow!("no existing parent for config directory {}", path.display())
                })?;
            }
            Err(error) => return Err(error.into()),
        }
    }

    for ancestor in current.ancestors() {
        validate_directory(ancestor, &fs::symlink_metadata(ancestor)?, ancestor == path)?;
    }
    Ok(())
}

fn validate_directory(path: &Path, metadata: &fs::Metadata, is_immediate: bool) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if metadata.file_type().is_symlink() {
        return unsafe_path(path, "parent is a symlink");
    }
    if !metadata.is_dir() {
        return unsafe_path(path, "parent is not a directory");
    }
    let mode = metadata.permissions().mode();
    if mode & 0o022 != 0 && (is_immediate || !is_root_owned_sticky_directory(metadata, mode)) {
        return unsafe_path(path, "parent is group or world writable");
    }
    if metadata.uid() != current_effective_uid() && metadata.uid() != 0 {
        return unsafe_path(path, "parent is not owned by the current user");
    }
    Ok(())
}

fn validate_opened_file(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    validate_private_file(path, metadata)
}

fn validate_private_file(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if metadata.file_type().is_symlink() {
        return unsafe_path(path, "target is a symlink");
    }
    if !metadata.is_file() {
        return unsafe_path(path, "target is not a regular file");
    }
    if metadata.uid() != current_effective_uid() {
        return unsafe_path(path, "target is not owned by the current user");
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return unsafe_path(path, "target is readable or writable by group or world");
    }
    Ok(())
}

pub(super) fn normalize_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalize_platform_root(path))
}

#[cfg(target_os = "macos")]
fn normalize_platform_root(path: PathBuf) -> PathBuf {
    match path.strip_prefix("/var") {
        Ok(suffix) => Path::new("/private/var").join(suffix),
        Err(_) => match path.strip_prefix("/tmp") {
            Ok(suffix) => Path::new("/private/tmp").join(suffix),
            Err(_) => path,
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn normalize_platform_root(path: PathBuf) -> PathBuf {
    path
}

fn is_root_owned_sticky_directory(metadata: &fs::Metadata, mode: u32) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.uid() == 0 && mode & 0o1000 != 0
}

fn current_effective_uid() -> u32 {
    // SAFETY: [Category 8 — FFI boundary] `geteuid` has no pointer arguments,
    // does not retain Rust memory, and returns the process effective UID by value.
    unsafe { libc::geteuid() }
}

fn unsafe_path(path: &Path, reason: &str) -> Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("unsafe path {}: {reason}", path.display()),
    )
    .into())
}
