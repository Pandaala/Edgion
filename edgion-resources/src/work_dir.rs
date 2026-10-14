//! Working Directory Management
//!
//! Provides unified path management with a standard directory layout:
//! - work_dir/logs/     - Log files
//! - work_dir/runtime/  - Runtime state
//! - work_dir/config/   - Configuration files
//!
//! Priority order: CLI --work-dir > ENV EDGION_WORK_DIR > Config > Default (".")

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
            std::env::current_dir().map_err(|e| format!("Cannot get current directory: {}", e))?
        } else {
            base.to_path_buf()
        };

        // Try to canonicalize (resolve symlinks), but fall back to original path if it fails
        let base = base.canonicalize().unwrap_or_else(|_| {
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
            std::fs::create_dir_all(&self.base).map_err(|e| {
                format!(
                    "Cannot create work directory {}: {}\n\
                     Please check permissions or specify a different directory with --work-dir",
                    self.base.display(),
                    e
                )
            })?;
        }

        if !self.base.is_dir() {
            return Err(format!(
                "Work directory path exists but is not a directory: {}",
                self.base.display()
            ));
        }

        // 2. Check write permission
        static NEXT_PROBE_ID: AtomicU64 = AtomicU64::new(0);
        WriteProbe::create(&self.base, NEXT_PROBE_ID.fetch_add(1, Ordering::Relaxed))
            .and_then(WriteProbe::check)
            .map_err(|e| {
                format!(
                    "Work directory {} is not writable: {}\n\
                     Please check directory permissions",
                    self.base.display(),
                    e
                )
            })?;

        // 3. Check/create subdirectories
        for (name, dir) in [
            ("logs", &self.logs),
            ("runtime", &self.runtime),
            ("config", &self.config),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Cannot create {} directory {}: {}", name, dir.display(), e))?;
        }

        tracing::info!(
            work_dir = %self.base.display(),
            "Working directory validated and subdirectories created"
        );

        Ok(())
    }
}

/// A startup probe owns only a file it created exclusively.
struct WriteProbe {
    file: File,
    path: PathBuf,
}

impl WriteProbe {
    const MAX_ATTEMPTS: u32 = 16;

    fn create(base: &Path, id: u64) -> io::Result<Self> {
        for attempt in 0..Self::MAX_ATTEMPTS {
            let path = base.join(format!(".edgion_write_test.{}.{id}.{attempt}", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(Self { file, path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "All work-directory write-probe candidates already exist",
        ))
    }

    fn check(mut self) -> io::Result<()> {
        let result = self.file.write_all(b"test");
        // Close before unlinking for portability, even when writing failed.
        drop(self.file);
        // Exclusive creation does not protect against later hostile path replacement.
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(path = %self.path.display(), error = %error, "Failed to remove write-test file");
        }
        result
    }
}

/// Global working directory instance
static WORK_DIR: OnceLock<WorkDir> = OnceLock::new();

/// Initialize the global working directory (call once at startup)
///
/// Returns an error if already initialized or if validation fails.
pub fn init_work_dir(base: impl AsRef<Path>) -> Result<(), String> {
    let wd = WorkDir::new(base)?;

    WORK_DIR
        .set(wd)
        .map_err(|_| "Work directory already initialized".to_string())?;

    Ok(())
}

/// Get the global working directory
///
/// If not initialized, returns a WorkDir based on current directory.
/// Should always call `init_work_dir()` first during startup.
pub fn work_dir() -> WorkDir {
    WORK_DIR.get().cloned().unwrap_or_else(|| {
        tracing::warn!("WorkDir not initialized, using current directory");
        WorkDir::new(".").expect("Failed to create default WorkDir")
    })
}

#[cfg(test)]
#[path = "../tests/unit/work_dir_test.rs"]
mod tests;
