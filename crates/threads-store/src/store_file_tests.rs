use std::os::unix::fs::{PermissionsExt, symlink};

use crate::Store;

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn file_store_creates_private_parent_and_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private").join("threads.sqlite");
    let store = Store::open(&path).unwrap();

    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&path.with_file_name("threads.sqlite-wal")), 0o600);
    assert_eq!(mode(&path.with_file_name("threads.sqlite-shm")), 0o600);
    drop(store);
}

#[test]
fn file_store_preserves_existing_custom_parent_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("custom-parent");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();

    Store::open(parent.join("threads.sqlite")).unwrap();

    assert_eq!(mode(&parent), 0o755);
}

#[test]
fn open_rejects_symlinked_database_target() {
    let temp = tempfile::tempdir().unwrap();
    let victim = temp.path().join("victim.sqlite");
    std::fs::write(&victim, b"original database bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let path = temp.path().join("threads.sqlite");
    symlink(&victim, &path).unwrap();

    assert!(Store::open(&path).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"original database bytes");
    assert_eq!(mode(&victim), 0o644);
}

#[test]
fn open_rejects_existing_symlink_without_chmodding_victim() {
    let temp = tempfile::tempdir().unwrap();
    let victim = temp.path().join("victim.sqlite");
    std::fs::write(&victim, b"original database bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let path = temp.path().join("threads.sqlite");
    symlink(&victim, &path).unwrap();

    assert!(Store::open(&path).is_err());
    assert_eq!(mode(&victim), 0o644);
}

#[test]
fn open_rejects_existing_symlinked_sqlite_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("threads.sqlite");
    std::fs::write(&path, []).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let wal_victim = temp.path().join("wal-victim.sqlite");
    let shm_victim = temp.path().join("shm-victim.sqlite");
    std::fs::write(&wal_victim, b"wal untouched").unwrap();
    std::fs::write(&shm_victim, b"shm untouched").unwrap();
    std::fs::set_permissions(&wal_victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&shm_victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let wal = path.with_file_name("threads.sqlite-wal");
    symlink(&wal_victim, &wal).unwrap();

    assert!(Store::open(&path).is_err());
    assert_eq!(std::fs::read(&wal_victim).unwrap(), b"wal untouched");
    assert_eq!(mode(&wal_victim), 0o644);

    std::fs::remove_file(wal).unwrap();
    std::fs::write(path.with_file_name("threads.sqlite-wal"), []).unwrap();
    std::fs::set_permissions(
        path.with_file_name("threads.sqlite-wal"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let shm = path.with_file_name("threads.sqlite-shm");
    symlink(&shm_victim, &shm).unwrap();

    assert!(Store::open(&path).is_err());
    assert_eq!(std::fs::read(&shm_victim).unwrap(), b"shm untouched");
    assert_eq!(mode(&shm_victim), 0o644);
}

#[test]
fn open_rejects_group_writable_existing_parent() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("shared");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o770)).unwrap();

    assert!(Store::open(parent.join("threads.sqlite")).is_err());
}

#[test]
fn open_creates_database_with_mode_0600() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("threads.sqlite");

    Store::open(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
}

#[test]
fn open_creates_private_wal_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("threads.sqlite");
    let store = Store::open(&path).unwrap();

    assert_eq!(mode(&path.with_file_name("threads.sqlite-wal")), 0o600);
    assert_eq!(mode(&path.with_file_name("threads.sqlite-shm")), 0o600);
    drop(store);
}

#[cfg(target_os = "macos")]
#[test]
fn open_raw_tmp_database_path_uses_the_private_tmp_alias() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let basename = temp.path().file_name().unwrap();
    let raw_path = std::path::Path::new("/tmp").join(basename).join("store.db");

    let store = Store::open(&raw_path).unwrap();

    assert_eq!(mode(&temp.path().join("store.db")), 0o600);
    drop(store);
}
