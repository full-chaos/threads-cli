use super::*;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn writes_private_file_and_directory() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("private").join("token.json");
    write_private_file(&path, b"{}").unwrap();
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn interrupted_replacement_preserves_existing_bytes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("token.json");
    fs::write(&path, b"old").unwrap();
    let result = atomic_write_private_file(&path, b"new", |_, _| {
        Err(std::io::Error::other("interrupted"))
    });
    assert!(result.is_err());
    assert_eq!(fs::read(path).unwrap(), b"old");
}

#[cfg(unix)]
#[test]
fn read_rejects_symlinked_token_target_without_touching_victim() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let victim = temp.path().join("victim.json");
    let token_path = temp.path().join("token.json");
    fs::write(&victim, b"victim").unwrap();
    symlink(&victim, &token_path).unwrap();

    let error = read_private_file(&token_path).expect_err("symlink must be rejected");

    assert!(error.to_string().contains("unsafe token file"));
    assert_eq!(fs::read(&victim).unwrap(), b"victim");
}

#[cfg(unix)]
#[test]
fn write_rejects_symlinked_token_parent_without_creating_a_target_file() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = TempDir::new().unwrap();
    let target_parent = temp.path().join("target");
    fs::create_dir(&target_parent).unwrap();
    fs::set_permissions(&target_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let symlink_parent = temp.path().join("linked-parent");
    symlink(&target_parent, &symlink_parent).unwrap();
    let token_path = symlink_parent.join("token.json");

    let error = write_private_file(&token_path, b"token").expect_err("symlinked parent must fail");

    assert!(error.to_string().contains("unsafe token directory"));
    assert!(!target_parent.join("token.json").exists());
}

#[cfg(unix)]
#[test]
fn read_rejects_symlinked_token_parent() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = TempDir::new().unwrap();
    let private_parent = temp.path().join("private");
    fs::create_dir(&private_parent).unwrap();
    fs::set_permissions(&private_parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(private_parent.join("token.json"), b"token").unwrap();
    fs::set_permissions(
        private_parent.join("token.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let token_path = temp.path().join("linked-parent").join("token.json");
    symlink(&private_parent, temp.path().join("linked-parent")).unwrap();

    let error = read_private_file(&token_path).expect_err("symlinked parent must be rejected");

    assert!(error.to_string().contains("unsafe token directory"));
}

#[cfg(unix)]
#[test]
fn read_rejects_group_or_world_writable_parent() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("private");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let token_path = parent.join("token.json");
    fs::write(&token_path, b"token").unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();

    let error = read_private_file(&token_path).expect_err("writable parent must be rejected");

    assert!(error.to_string().contains("unsafe token directory"));
}

#[cfg(unix)]
#[test]
fn read_rejects_group_or_world_accessible_token_file() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let token_path = temp.path().join("token.json");
    fs::write(&token_path, b"token").unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o644)).unwrap();

    let error = read_private_file(&token_path).expect_err("insecure file must be rejected");

    assert!(error.to_string().contains("unsafe token file"));
}

#[cfg(unix)]
#[test]
fn metadata_validation_rejects_foreign_owner_without_chown() {
    use std::os::unix::fs::MetadataExt;

    let temp = TempDir::new().unwrap();
    let token_path = temp.path().join("token.json");
    fs::write(&token_path, b"token").unwrap();
    let metadata = fs::metadata(&token_path).unwrap();

    let error = validate_private_file_metadata(&metadata, metadata.uid().wrapping_add(1))
        .expect_err("a non-owner must be rejected");

    assert!(error.to_string().contains("unsafe token file"));
}

#[cfg(unix)]
#[test]
fn read_accepts_0700_parent_and_0600_token_file() {
    assert_read_accepts_private_path(0o700);
}

#[cfg(unix)]
#[test]
fn read_accepts_0755_parent_and_0600_token_file() {
    assert_read_accepts_private_path(0o755);
}

#[cfg(unix)]
#[test]
fn read_accepts_private_child_beneath_root_owned_sticky_tmp() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new_in("/tmp").unwrap();
    let parent = temp.path().join("private");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let token_path = parent.join("token.json");
    fs::write(&token_path, b"token").unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(
        read_private_file(&token_path).unwrap(),
        Some("token".into())
    );
}

#[cfg(unix)]
#[test]
fn read_and_write_reject_nonsticky_writable_ancestor() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new_in("/tmp").unwrap();
    let ancestor = temp.path().join("writable");
    fs::create_dir(&ancestor).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o777)).unwrap();
    let parent = ancestor.join("private");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let path = parent.join("token.json");
    fs::write(&path, b"token").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_private_file(&path).is_err());
    assert!(write_private_file(&path, b"new").is_err());
}

#[cfg(unix)]
#[test]
fn read_and_write_reject_intermediate_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempDir::new_in("/tmp").unwrap();
    let target = temp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let linked = temp.path().join("linked");
    symlink(&target, &linked).unwrap();
    let parent = linked.join("private");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let path = parent.join("token.json");
    fs::write(&path, b"token").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_private_file(&path).is_err());
    assert!(write_private_file(&path, b"new").is_err());
}

#[cfg(unix)]
#[test]
fn ancestor_policy_rejects_foreign_owned_0755_directory() {
    assert!(!unix_file_security::is_trusted_ancestor_owner(
        effective_user_id().wrapping_add(1)
    ));
}

#[cfg(unix)]
fn assert_read_accepts_private_path(parent_mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("private");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(parent_mode)).unwrap();
    let token_path = parent.join("token.json");
    fs::write(&token_path, b"token").unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(
        read_private_file(&token_path).unwrap(),
        Some("token".into())
    );
}
