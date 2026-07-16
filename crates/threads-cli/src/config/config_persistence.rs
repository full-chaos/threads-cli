use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, anyhow};

pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    create_private_dir_platform(path)
}

pub(crate) fn atomic_write_private_file<F>(path: &Path, bytes: &[u8], persist: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let (mut file, temporary_path) = open_private_temporary_file(path)?;
    let result = (|| {
        file.write_all(bytes).with_context(|| {
            format!("writing temporary config file {}", temporary_path.display())
        })?;
        file.sync_all().with_context(|| {
            format!("syncing temporary config file {}", temporary_path.display())
        })?;
        drop(file);
        persist(&temporary_path, path)
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
        .ok_or_else(|| anyhow!("config path {} has no parent", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("config path {} has no file name", path.display()))?
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
                return Err(anyhow!(
                    "creating temporary config file {}: {error}",
                    temporary_path.display()
                ));
            }
        }
    }
    Err(anyhow!(
        "could not allocate a temporary config file beside {}",
        path.display()
    ))
}

#[cfg(unix)]
fn create_private_dir_platform(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if path.exists() {
        let mut permissions = fs::metadata(path)
            .with_context(|| format!("stat config directory {}", path.display()))?
            .permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions)
                .with_context(|| format!("chmod config directory {}", path.display()))?;
        }
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("creating config directory {}", path.display()))
}

#[cfg(not(unix))]
fn create_private_dir_platform(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("creating config directory {}", path.display()))
}

#[cfg(unix)]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_new_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}
