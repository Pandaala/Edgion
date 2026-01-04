//! Working Directory Management
//!
//! Provides unified path management with a standard directory layout:
//! - work_dir/logs/     - Log files
//! - work_dir/runtime/  - Runtime state
//! - work_dir/config/   - Configuration files
//!
//! Priority order: CLI --work-dir > ENV EDGION_WORK_DIR > Config > Default (".")

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Global working directory configuration
#[derive(Clone, Debug)]
pub struct WorkDir {
    /// Base working directory
    base: PathBuf,
    /// Logs subdirectory
    logs: PathBuf,
    /// Runtime data subdirectory
    runtime: PathBuf,
    /// Config subdirectory
    config: PathBuf,
}

impl WorkDir {
    /// Create a new WorkDir instance
    ///
    /// Handles special cases:
    /// - Empty string or "." -> current working directory (absolute)
    /// - Symbolic links -> resolved (with fallback on failure)
    pub fn new(base: impl AsRef<Path>) -> Result<Self, String> {
        let base = base.as_ref();
        
        // Handle empty string or "."
        let base = if base.as_os_str().is_empty() || base == Path::new(".") {
            std::env::current_dir()
                .map_err(|e| format!("Cannot get current directory: {}", e))?
        } else {
            base.to_path_buf()
        };
        
        // Try to canonicalize (resolve symlinks), but fall back to original path if it fails
        let base = base.canonicalize()
            .unwrap_or_else(|_| {
                tracing::warn!("Cannot canonicalize work_dir {:?}, using as-is", base);
                base
            });
        
        Ok(Self {
            logs: base.join("logs"),
            runtime: base.join("runtime"),
            config: base.join("config"),
            base,
        })
    }
    
    /// Get the base working directory
    pub fn base(&self) -> &Path {
        &self.base
    }
    
    /// Get the logs subdirectory
    pub fn logs(&self) -> &Path {
        &self.logs
    }
    
    /// Get the runtime subdirectory
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }
    
    /// Get the config subdirectory
    pub fn config(&self) -> &Path {
        &self.config
    }
    
    /// Resolve a path (absolute paths pass through, relative paths join with base)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let wd = WorkDir::new("/work").unwrap();
    /// assert_eq!(wd.resolve("/var/log/app.log"), PathBuf::from("/var/log/app.log"));
    /// assert_eq!(wd.resolve("logs/app.log"), PathBuf::from("/work/logs/app.log"));
    /// ```
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base.join(path)
        }
    }
    
    /// Validate that the working directory is accessible and writable
    ///
    /// Checks:
    /// 1. Base directory exists or can be created
    /// 2. Directory is writable (by creating a test file)
    /// 3. Subdirectories can be created (logs, runtime, config)
    pub fn validate(&self) -> Result<(), String> {
        // 1. Check if base directory exists or can be created
        if !self.base.exists() {
            std::fs::create_dir_all(&self.base)
                .map_err(|e| format!(
                    "Cannot create work directory {}: {}\n\
                     Please check permissions or specify a different directory with --work-dir",
                    self.base.display(), e
                ))?;
        }
        
        if !self.base.is_dir() {
            return Err(format!(
                "Work directory path exists but is not a directory: {}",
                self.base.display()
            ));
        }
        
        // 2. Check write permission
        let test_file = self.base.join(".edgion_write_test");
        std::fs::write(&test_file, b"test")
            .map_err(|e| format!(
                "Work directory {} is not writable: {}\n\
                 Please check directory permissions",
                self.base.display(), e
            ))?;
        let _ = std::fs::remove_file(&test_file);
        
        // 3. Check/create subdirectories
        for (name, dir) in [
            ("logs", &self.logs),
            ("runtime", &self.runtime),
            ("config", &self.config),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!(
                    "Cannot create {} directory {}: {}",
                    name, dir.display(), e
                ))?;
        }
        
        tracing::info!(
            work_dir = %self.base.display(),
            "Working directory validated and subdirectories created"
        );
        
        Ok(())
    }
}

/// Global working directory instance
static WORK_DIR: OnceLock<WorkDir> = OnceLock::new();

/// Initialize the global working directory (call once at startup)
///
/// Returns an error if already initialized or if validation fails.
pub fn init_work_dir(base: impl AsRef<Path>) -> Result<(), String> {
    let wd = WorkDir::new(base)?;
    
    WORK_DIR.set(wd)
        .map_err(|_| "Work directory already initialized".to_string())?;
    
    Ok(())
}

/// Get the global working directory
///
/// If not initialized, returns a WorkDir based on current directory.
/// Should always call `init_work_dir()` first during startup.
pub fn work_dir() -> WorkDir {
    WORK_DIR.get()
        .cloned()
        .unwrap_or_else(|| {
            tracing::warn!("WorkDir not initialized, using current directory");
            WorkDir::new(".").expect("Failed to create default WorkDir")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    
    #[test]
    fn test_work_dir_basic() {
        let temp = tempfile::TempDir::new().unwrap();
        let wd = WorkDir::new(temp.path()).unwrap();
        
        // After canonicalization, paths may differ (e.g., /var -> /private/var on macOS)
        // Just check that subdirectories have correct names
        assert_eq!(wd.logs().file_name().unwrap(), "logs");
        assert_eq!(wd.runtime().file_name().unwrap(), "runtime");
        assert_eq!(wd.config().file_name().unwrap(), "config");
        
        // Verify they're under the base directory
        assert!(wd.logs().starts_with(wd.base()));
        assert!(wd.runtime().starts_with(wd.base()));
        assert!(wd.config().starts_with(wd.base()));
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
        assert!(wd.logs().exists());
        assert!(wd.runtime().exists());
        assert!(wd.config().exists());
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
}

