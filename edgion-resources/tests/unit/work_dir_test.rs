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
