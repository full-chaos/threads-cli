use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use threads_core::{Error, Result};

#[cfg(unix)]
#[path = "unix_file_security.rs"]
mod unix_file_security;
#[cfg(unix)]
use unix_file_security::{
    effective_user_id, open_private_read_only_file, validate_existing_private_file,
    validate_private_ancestor_chain, validate_private_directory_metadata,
    validate_private_file_metadata,
};

pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Config(format!("token path {} has no parent", path.display())))?;
    create_private_dir(parent)?;
    #[cfg(unix)]
    validate_private_ancestor_chain(parent)?;
    atomic_write_private_file(path, bytes, |temporary_path, destination_path| {
        fs::rename(temporary_path, destination_path)
    })
}

pub(super) fn read_private_file(path: &Path) -> Result<Option<String>> {
    read_private_file_impl(path)
}

#[cfg(unix)]
fn read_private_file_impl(path: &Path) -> Result<Option<String>> {
    let Some(mut file) = open_private_read_only_file(path)? else {
        return Ok(None);
    };
    validate_private_file_metadata(
        &file
            .metadata()
            .map_err(|error| Error::Config(format!("stat token file: {error}")))?,
        effective_user_id(),
    )?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| Error::Config(format!("reading token file: {error}")))?;
    Ok(Some(contents))
}

#[cfg(not(unix))]
fn read_private_file_impl(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|error| Error::Config(format!("reading token file: {error}")))
}

pub(super) fn remove_private_file(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| Error::Config(format!("removing token file: {error}")))?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut missing = Vec::new();
    let mut current = path;
    loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                validate_private_directory_metadata(&metadata, effective_user_id())?;
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(current.to_path_buf());
                current = current.parent().ok_or_else(|| {
                    Error::Config(format!(
                        "no existing parent for token directory {}",
                        path.display()
                    ))
                })?;
            }
            Err(error) => return Err(Error::Config(format!("stat token directory: {error}"))),
        }
    }
    for directory in missing.iter().rev() {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(Error::Config(format!("creating token directory: {error}"))),
        }
        validate_private_directory_metadata(
            &fs::symlink_metadata(directory)
                .map_err(|error| Error::Config(format!("stat token directory: {error}")))?,
            effective_user_id(),
        )?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| Error::Config(format!("creating token dir: {error}")))
}

fn atomic_write_private_file<F>(path: &Path, bytes: &[u8], persist: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    #[cfg(unix)]
    validate_existing_private_file(path)?;
    let (mut file, temporary_path) = open_private_temporary_file(path)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|error| Error::Config(format!("writing temporary token file: {error}")))?;
        file.sync_all()
            .map_err(|error| Error::Config(format!("syncing temporary token file: {error}")))?;
        drop(file);
        persist(&temporary_path, path)
            .map_err(|error| Error::Config(format!("replacing token file atomically: {error}")))
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
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
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
#[path = "file_store_tests.rs"]
mod tests;
