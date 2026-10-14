use super::*;
use std::fs;

#[test]
fn test_work_dir_basic() {
    let temp = tempfile::TempDir::new().unwrap();
    let wd = WorkDir::new(temp.path()).unwrap();

    // After canonicalization, paths may differ (e.g., /var -> /private/var on macOS)
    // Just check that subdirectories have correct names
    assert_eq!(wd.logs.file_name().unwrap(), "logs");
    assert_eq!(wd.runtime.file_name().unwrap(), "runtime");
    assert_eq!(wd.config.file_name().unwrap(), "config");

    // Verify they're under the base directory
    assert!(wd.logs.starts_with(wd.base()));
    assert!(wd.runtime.starts_with(wd.base()));
    assert!(wd.config.starts_with(wd.base()));
}

#[test]
fn test_resolve_absolute_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let wd = WorkDir::new(temp.path()).unwrap();

    let abs_path = PathBuf::from("/var/log/app.log");
    assert_eq!(wd.resolve(&abs_path), abs_path);
}

#[test]
fn test_resolve_relative_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let wd = WorkDir::new(temp.path()).unwrap();

    let resolved = wd.resolve("logs/app.log");
    // After canonicalization, check it's under base directory
    assert!(resolved.starts_with(wd.base()));
    assert!(resolved.ends_with("logs/app.log"));
}

#[test]
fn test_validate_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let wd = WorkDir::new(temp.path()).unwrap();

    assert!(wd.validate().is_ok());

    // Check that subdirectories were created
    assert!(wd.logs.exists());
    assert!(wd.runtime.exists());
    assert!(wd.config.exists());
    assert_eq!(fs::read_dir(wd.base()).unwrap().count(), 3);
    wd.validate().unwrap();
    assert_eq!(fs::read_dir(wd.base()).unwrap().count(), 3);
}

#[test]
#[cfg(unix)]
fn test_validate_readonly_fails() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::TempDir::new().unwrap();

    // Set directory to read-only
    let mut perms = temp.path().metadata().unwrap().permissions();
    perms.set_mode(0o444);
    fs::set_permissions(temp.path(), perms).unwrap();

    let wd = WorkDir::new(temp.path()).unwrap();
    assert!(wd.validate().is_err());

    // Restore permissions for cleanup
    let mut perms = temp.path().metadata().unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(temp.path(), perms).unwrap();
}

#[test]
fn test_empty_string_to_current_dir() {
    let wd = WorkDir::new("").unwrap();
    let current = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(wd.base(), current);
}

#[test]
fn test_dot_to_current_dir() {
    let wd = WorkDir::new(".").unwrap();
    let current = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(wd.base(), current);
}

#[test]
fn test_symlink_resolution() {
    let temp = tempfile::TempDir::new().unwrap();
    let real_dir = temp.path().join("real");
    fs::create_dir(&real_dir).unwrap();

    #[cfg(unix)]
    {
        let link_dir = temp.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

        let wd = WorkDir::new(&link_dir).unwrap();
        // Should resolve to the real directory
        assert_eq!(wd.base(), real_dir.canonicalize().unwrap());
    }
}

#[test]
fn test_nonexistent_dir_validation_creates_it() {
    let temp = tempfile::TempDir::new().unwrap();
    let nonexistent = temp.path().join("nonexistent");

    let wd = WorkDir::new(&nonexistent).unwrap();
    assert!(wd.validate().is_ok());
    assert!(nonexistent.exists());
}

#[test]
fn test_validate_preserves_existing_legacy_probe_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let existing = temp.path().join(".edgion_write_test");
    fs::write(&existing, b"original data").unwrap();

    WorkDir::new(temp.path()).unwrap().validate().unwrap();

    assert_eq!(fs::read(&existing).unwrap(), b"original data");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 4);
}

#[test]
#[cfg(unix)]
fn test_validate_preserves_legacy_probe_symlink() {
    let temp = tempfile::TempDir::new().unwrap();
    let target = temp.path().join("original");
    let link = temp.path().join(".edgion_write_test");
    fs::write(&target, b"original data").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    WorkDir::new(temp.path()).unwrap().validate().unwrap();

    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), b"original data");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 5);
}

#[test]
fn test_write_probe_retries_without_modifying_collision() {
    let temp = tempfile::TempDir::new().unwrap();
    let existing = temp
        .path()
        .join(format!(".edgion_write_test.{}.7.0", std::process::id()));
    fs::write(&existing, b"original data").unwrap();

    let probe = WriteProbe::create(temp.path(), 7).unwrap();
    assert_ne!(probe.path, existing);
    probe.check().unwrap();

    assert_eq!(fs::read(&existing).unwrap(), b"original data");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
#[cfg(unix)]
fn test_write_probe_preserves_colliding_symlink() {
    let temp = tempfile::TempDir::new().unwrap();
    let target = temp.path().join("original");
    let link = temp
        .path()
        .join(format!(".edgion_write_test.{}.7.0", std::process::id()));
    fs::write(&target, b"original data").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    WriteProbe::create(temp.path(), 7).unwrap().check().unwrap();

    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), b"original data");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[test]
fn test_write_probe_collision_exhaustion_preserves_files() {
    let temp = tempfile::TempDir::new().unwrap();
    let paths: Vec<_> = (0..WriteProbe::MAX_ATTEMPTS)
        .map(|attempt| {
            temp.path()
                .join(format!(".edgion_write_test.{}.7.{attempt}", std::process::id()))
        })
        .collect();
    for path in &paths {
        fs::write(path, b"original data").unwrap();
    }

    let error = WriteProbe::create(temp.path(), 7).err().unwrap();

    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), paths.len());
    for path in paths {
        assert_eq!(fs::read(path).unwrap(), b"original data");
    }
}

#[test]
fn test_write_probe_returns_non_collision_error() {
    let temp = tempfile::TempDir::new().unwrap();
    let missing = temp.path().join("missing");

    let error = WriteProbe::create(&missing, 7).err().unwrap();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!missing.exists());
}

#[test]
fn test_write_probe_cleans_up_after_write_failure() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut probe = WriteProbe::create(temp.path(), 7).unwrap();
    let path = probe.path.clone();
    // A read-only handle deterministically rejects writes without filesystem privileges.
    probe.file = File::open(&path).unwrap();

    assert!(probe.check().is_err());
    assert!(!path.exists());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
#[cfg(unix)]
fn test_write_probe_cleanup_failure_is_non_fatal() {
    let temp = tempfile::TempDir::new().unwrap();
    let probe = WriteProbe::create(temp.path(), 7).unwrap();
    // Unix keeps the open file writable after its directory entry is removed.
    fs::remove_file(&probe.path).unwrap();

    assert!(probe.check().is_ok());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
