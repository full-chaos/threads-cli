use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};

#[cfg(unix)]
mod unix;

pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    create_private_dir_platform(path)
}

pub(crate) fn read_private_file(path: &Path) -> Result<Option<String>> {
    read_private_file_platform(path)
}

pub(crate) fn atomic_write_private_file<F>(path: &Path, bytes: &[u8], persist: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    #[cfg(unix)]
    let path = unix::normalize_path(path)?;
    #[cfg(not(unix))]
    let path = path.to_path_buf();
    #[cfg(unix)]
    unix::validate_existing_file(&path)?;
    let (mut file, temporary_path) = open_private_temporary_file(&path)?;
    let result = (|| {
        file.write_all(bytes).with_context(|| {
            format!("writing temporary config file {}", temporary_path.display())
        })?;
        file.sync_all().with_context(|| {
            format!("syncing temporary config file {}", temporary_path.display())
        })?;
        drop(file);
        persist(&temporary_path, &path)
            .with_context(|| format!("replacing config file {} atomically", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(test)]
pub(crate) fn atomic_write_with_interruption(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_private_file(path, bytes, |_, _| {
        Err(std::io::Error::other("simulated replacement interruption"))
    })
}

fn open_private_temporary_file(path: &Path) -> Result<(fs::File, PathBuf)> {
    const MAX_ATTEMPTS: u8 = 16;
    static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path {} has no parent", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("config path {} has no file name", path.display()))?
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
                return Err(anyhow::anyhow!(
                    "creating temporary config file {}: {error}",
                    temporary_path.display()
                ));
            }
        }
    }
    Err(anyhow::anyhow!(
        "could not allocate a temporary config file beside {}",
        path.display()
    ))
}

#[cfg(unix)]
fn read_private_file_platform(path: &Path) -> Result<Option<String>> {
    unix::read_private_file(path)
}

#[cfg(not(unix))]
fn read_private_file_platform(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn create_private_dir_platform(path: &Path) -> Result<()> {
    unix::create_private_dir(path)
}

#[cfg(not(unix))]
fn create_private_dir_platform(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("creating config directory {}", path.display()))
}

#[cfg(unix)]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    unix::open_private_new_file(path)
}

#[cfg(not(unix))]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}
