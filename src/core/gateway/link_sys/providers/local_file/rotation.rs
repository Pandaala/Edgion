//! File rotation utilities for LocalFileWriter

use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::types::link_sys::RotationStrategy;

/// Generate the archive (rotated) file path for time-based strategies.
/// Used when rotating: the active file (base path) is renamed to this path.
/// e.g., "logs/access.log" with Daily => "logs/access.2025-12-05.log"
/// For Size strategy, use `get_size_rotated_path` instead.
pub fn get_rotated_path(base_path: &Path, strategy: &RotationStrategy) -> PathBuf {
    match strategy {
        RotationStrategy::Never | RotationStrategy::Size(_) => base_path.to_path_buf(),
        RotationStrategy::Daily => {
            let date_suffix = Local::now().format("%Y-%m-%d");
            append_suffix(base_path, &date_suffix.to_string())
        }
        RotationStrategy::Hourly => {
            let datetime_suffix = Local::now().format("%Y-%m-%d-%H");
            append_suffix(base_path, &datetime_suffix.to_string())
        }
    }
}

/// Generate rotated file path for Size strategy with index
/// e.g., "logs/access.log" with index 1 => "logs/access.1.log"
pub fn get_size_rotated_path(base_path: &Path, index: u32) -> PathBuf {
    if index == 0 {
        base_path.to_path_buf()
    } else {
        append_suffix(base_path, &index.to_string())
    }
}

/// Find the next available index for size-based rotation
/// Scans existing files and returns the next index to use
pub fn find_next_size_index(base_path: &Path) -> u32 {
    let parent = match base_path.parent() {
        Some(p) => p,
        None => return 0,
    };

    let stem = base_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = base_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut max_index: u32 = 0;

    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Some(filename) = entry.path().file_name().and_then(|n| n.to_str()) {
                // Match pattern: {stem}.{number}.{ext}
                let prefix = format!("{}.", stem);
                let suffix = format!(".{}", ext);

                if filename.starts_with(&prefix) && filename.ends_with(&suffix) {
                    let middle = &filename[prefix.len()..filename.len() - suffix.len()];
                    if let Ok(index) = middle.parse::<u32>() {
                        max_index = max_index.max(index);
                    }
                }
            }
        }
    }

    max_index + 1
}

/// Append suffix before file extension
/// e.g., "logs/access.log" + "2025-12-05" => "logs/access.2025-12-05.log"
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("log");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("log");
    let new_name = format!("{}.{}.{}", stem, suffix, ext);
    path.with_file_name(new_name)
}

/// Get current rotation key for comparison (date or hour)
/// For Size strategy, returns empty string (rotation is based on file size, not time)
pub fn get_rotation_key(strategy: &RotationStrategy) -> String {
    match strategy {
        RotationStrategy::Never | RotationStrategy::Size(_) => String::new(),
        RotationStrategy::Daily => Local::now().format("%Y-%m-%d").to_string(),
        RotationStrategy::Hourly => Local::now().format("%Y-%m-%d-%H").to_string(),
    }
}

/// Open file for writing, creating parent directories if needed
pub fn open_log_file(path: &Path) -> Result<File, std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Cleanup old rotated files, keeping only max_files most recent ones
///
/// Scans directory for files matching the pattern: {stem}.*.{ext}
/// Sorts by modification time and removes oldest files exceeding max_files
pub fn cleanup_old_files(base_path: &Path, max_files: usize) {
    if max_files == 0 {
        return; // 0 means unlimited
    }

    let parent = match base_path.parent() {
        Some(p) => p,
        None => return,
    };

    let stem = base_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = base_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Pattern: {stem}.*.{ext} (e.g., "access.2025-12-05.log")
    let pattern_prefix = format!("{}.", stem);
    let pattern_suffix = format!(".{}", ext);

    // Collect matching files with their modification times
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match pattern: starts with "{stem}." and ends with ".{ext}"
                // But exclude the base file itself (no date suffix)
                if filename.starts_with(&pattern_prefix)
                    && filename.ends_with(&pattern_suffix)
                    && filename != format!("{}.{}", stem, ext)
                {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            files.push((path, modified));
                        }
                    }
                }
            }
        }
    }

    // Sort by modification time (newest first)
    files.sort_by(|a, b| b.1.cmp(&a.1));

    // Remove files exceeding max_files
    for (path, _) in files.into_iter().skip(max_files) {
        if let Err(e) = fs::remove_file(&path) {
            tracing::warn!(error = %e, path = %path.display(), "Failed to cleanup old log file");
        } else {
            tracing::info!(path = %path.display(), "Cleaned up old log file");
        }
    }
}
