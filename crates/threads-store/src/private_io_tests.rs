use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    process::Command,
};

use tempfile::TempDir;

use super::{
    effective_user_id, has_expected_owner, is_trusted_ancestor, normalize_database_path,
    normalize_existing_database, open_existing_database, prepare_database_path, sqlite_sidecar,
};

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o7777
}

#[test]
fn newly_created_database_is_private_before_sqlite_opens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");

    prepare_database_path(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
}

#[test]
fn existing_owned_database_mode_0644_is_normalized_without_data_loss() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    let store = crate::Store::open(&path).unwrap();
    store
        .raw_conn()
        .execute_batch(
            "CREATE TABLE retained_data (value TEXT); INSERT INTO retained_data VALUES ('kept');",
        )
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
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn existing_owner_write_only_database_mode_0200_is_normalized_by_descriptor() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    fs::write(&path, b"retained database bytes").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();

    prepare_database_path(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
    assert_eq!(fs::read(&path).unwrap(), b"retained database bytes");
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

    assert_eq!(mode(&wal), 0o600);
    assert_eq!(mode(&shm), 0o600);
    drop(reopened);
    drop(store);
}

#[test]
fn existing_owned_database_special_mode_bits_are_cleared() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    prepare_database_path(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o7600)).unwrap();

    prepare_database_path(&path).unwrap();

    assert_eq!(mode(&path), 0o600);
}

#[test]
fn mode_0000_database_is_rejected_when_no_safe_descriptor_open_is_available() {
    if effective_user_id() == 0 {
        return;
    }

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    fs::write(&path, b"unreadable database bytes").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

    assert!(prepare_database_path(&path).is_err());
    assert_eq!(mode(&path), 0o000);
}

#[test]
fn fifo_is_rejected_without_changing_its_mode() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    assert!(
        Command::new("mkfifo")
            .args(["-m", "600"])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );

    assert!(prepare_database_path(&path).is_err());
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn non_sticky_writable_ancestor_is_rejected() {
    let temp = TempDir::new().unwrap();
    let ancestor = temp.path().join("writable-ancestor");
    let parent = ancestor.join("private-parent");
    fs::create_dir(&ancestor).unwrap();
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o777)).unwrap();

    assert!(prepare_database_path(&parent.join("store.db")).is_err());
}

#[test]
fn symlinked_ancestor_is_rejected_without_creating_a_database() {
    let temp = TempDir::new().unwrap();
    let trusted = temp.path().join("trusted");
    let aliased = temp.path().join("aliased");
    fs::create_dir(&trusted).unwrap();
    fs::create_dir(trusted.join("private-parent")).unwrap();
    symlink(&trusted, &aliased).unwrap();
    let path = aliased.join("private-parent").join("store.db");

    assert!(prepare_database_path(&path).is_err());
    assert!(!trusted.join("private-parent/store.db").exists());
}

#[test]
fn nondirectory_ancestor_is_rejected_without_modifying_the_victim() {
    let temp = TempDir::new().unwrap();
    let victim = temp.path().join("not-a-directory");
    fs::write(&victim, b"unchanged").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(prepare_database_path(&victim.join("private-parent/store.db")).is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
    assert_eq!(mode(&victim), 0o644);
}

#[test]
fn root_owned_sticky_tmp_ancestor_allows_private_immediate_parent() {
    let tmp = fs::canonicalize("/tmp").unwrap();
    let metadata = fs::metadata(&tmp).unwrap();
    if metadata.uid() != 0 || metadata.mode() & 0o1000 == 0 {
        return;
    }

    let temp = tempfile::tempdir_in(tmp).unwrap();
    let parent = temp.path().join("private-parent");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();

    prepare_database_path(&parent.join("store.db")).unwrap();
}

#[test]
fn foreign_owned_mode_0755_ancestor_is_rejected_by_policy() {
    let expected_uid = effective_user_id();

    assert!(!is_trusted_ancestor(
        expected_uid.wrapping_add(1),
        0o755,
        expected_uid
    ));
}

#[test]
fn root_owned_sticky_writable_ancestor_is_allowed_by_policy() {
    assert!(is_trusted_ancestor(0, 0o1777, effective_user_id()));
}

#[test]
fn existing_owned_database_noncanonical_owner_modes_are_normalized() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    prepare_database_path(&path).unwrap();

    for permissions in [0o400, 0o700] {
        fs::set_permissions(&path, fs::Permissions::from_mode(permissions)).unwrap();
        prepare_database_path(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
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

#[test]
fn test_fixture_is_owned_by_the_effective_user() {
    let temp = TempDir::new().unwrap();
    assert_eq!(
        fs::metadata(temp.path()).unwrap().uid(),
        effective_user_id()
    );
}

#[test]
fn relative_database_path_is_resolved_from_the_current_directory() {
    let current = std::env::current_dir().unwrap();

    assert_eq!(
        normalize_database_path(std::path::Path::new("store.db")).unwrap(),
        current.join("store.db")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_system_database_aliases_are_normalized_lexically() {
    assert_eq!(
        normalize_database_path(std::path::Path::new("/tmp/example/store.db")).unwrap(),
        std::path::Path::new("/private/tmp/example/store.db")
    );
    assert_eq!(
        normalize_database_path(std::path::Path::new("/var/example/store.db")).unwrap(),
        std::path::Path::new("/private/var/example/store.db")
    );
    assert_eq!(
        normalize_database_path(std::path::Path::new("/tmpx/example/store.db")).unwrap(),
        std::path::Path::new("/tmpx/example/store.db")
    );
}
