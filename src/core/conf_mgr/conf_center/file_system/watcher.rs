//! File system watcher using notify crate
//!
//! Monitors configuration directory for file changes and notifies ConfigServer.
//! Uses debouncing to handle rapid file change events.
//! Supports graceful shutdown via ShutdownSignal.

use super::resource_applier::apply_resource_change;
use crate::core::conf_mgr::conf_center::sync_runtime::ShutdownSignal;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::ConfigServer;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// File watcher for configuration directory
pub struct FileWatcher {
    conf_dir: PathBuf,
    config_server: Arc<ConfigServer>,
    /// Cache of file content hashes to detect actual changes
    file_hashes: HashMap<PathBuf, u64>,
}

impl FileWatcher {
    /// Create a new file watcher
    pub fn new(conf_dir: impl Into<PathBuf>, config_server: Arc<ConfigServer>) -> Self {
        Self {
            conf_dir: conf_dir.into(),
            config_server,
            file_hashes: HashMap::new(),
        }
    }

    /// Start watching for file changes
    ///
    /// This method spawns a background task that monitors the configuration directory
    /// and applies changes to ConfigServer when files are modified.
    ///
    /// # Arguments
    /// * `shutdown_signal` - Signal to trigger graceful shutdown
    pub async fn start(mut self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        let conf_dir = self.conf_dir.clone();

        tracing::info!(
            component = "file_watcher",
            conf_dir = %conf_dir.display(),
            "Starting file watcher"
        );

        // Create channel for file events
        let (tx, mut rx) = mpsc::channel::<Event>(100);

        // Create watcher in a blocking thread
        let conf_dir_clone = conf_dir.clone();
        let watcher_handle = tokio::task::spawn_blocking(move || -> Result<RecommendedWatcher> {
            let tx_clone = tx.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        // Filter events: exclude Access (read-only), include all others
                        match event.kind {
                            EventKind::Access(_) => {} // Ignore read-only access events
                            _ => {
                                let _ = tx_clone.blocking_send(event);
                            }
                        }
                    }
                },
                Config::default().with_poll_interval(Duration::from_secs(2)),
            )
            .context("Failed to create file watcher")?;

            // Watch the directory (non-recursive - only watch YAML files in the root)
            watcher
                .watch(&conf_dir_clone, RecursiveMode::NonRecursive)
                .context("Failed to start watching directory")?;

            tracing::info!(
                component = "file_watcher",
                conf_dir = %conf_dir_clone.display(),
                "File watcher initialized"
            );

            Ok(watcher)
        });

        // Keep watcher alive
        let _watcher = watcher_handle.await??;

        // Process events with debouncing (1s interval for deduplication)
        let mut pending_paths: HashSet<PathBuf> = HashSet::new();
        let debounce_duration = Duration::from_secs(1);
        let mut debounce_timer = tokio::time::interval(debounce_duration);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Accumulate paths for debouncing (HashSet auto-deduplicates)
                    for path in event.paths {
                        // Only process YAML files
                        if self.is_yaml_file(&path) {
                            pending_paths.insert(path);
                        }
                    }
                }
                _ = debounce_timer.tick() => {
                    // Process all pending paths
                    if !pending_paths.is_empty() {
                        for path in pending_paths.drain() {
                            if let Err(e) = self.process_path(&path).await {
                                tracing::error!(
                                    component = "file_watcher",
                                    path = %path.display(),
                                    error = %e,
                                    "Failed to process path"
                                );
                            }
                        }
                    }
                }
                _ = shutdown_signal.wait() => {
                    tracing::info!(
                        component = "file_watcher",
                        conf_dir = %conf_dir.display(),
                        "Received shutdown signal, stopping file watcher"
                    );
                    break;
                }
            }
        }

        tracing::info!(
            component = "file_watcher",
            conf_dir = %conf_dir.display(),
            "File watcher stopped"
        );

        Ok(())
    }

    /// Check if a path is a YAML file
    fn is_yaml_file(&self, path: &Path) -> bool {
        path.extension()
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false)
    }

    /// Process a changed path - check existence to determine operation
    async fn process_path(&mut self, path: &Path) -> Result<()> {
        if path.exists() {
            // File exists: Add or Update
            self.handle_file_change(path).await
        } else {
            // File doesn't exist: Delete
            self.handle_file_delete(path).await
        }
    }

    /// Handle file creation or modification
    async fn handle_file_change(&mut self, path: &Path) -> Result<()> {
        // Read file content
        let content = tokio::fs::read_to_string(path).await.context("Failed to read file")?;

        // Calculate hash to detect actual changes
        let new_hash = self.hash_content(&content);
        let old_hash = self.file_hashes.get(path).copied();

        // Skip if content hasn't actually changed
        if old_hash == Some(new_hash) {
            tracing::trace!(
                component = "file_watcher",
                path = %path.display(),
                "File content unchanged, skipping"
            );
            return Ok(());
        }

        // Update hash cache
        self.file_hashes.insert(path.to_path_buf(), new_hash);

        // Determine change type
        let change = if old_hash.is_some() {
            ResourceChange::EventUpdate
        } else {
            ResourceChange::EventAdd
        };

        // Apply to ConfigServer
        self.apply_change(&content, change).await?;

        tracing::info!(
            component = "file_watcher",
            path = %path.display(),
            change = ?change,
            "Applied file change to ConfigServer"
        );

        Ok(())
    }

    /// Handle file deletion
    async fn handle_file_delete(&mut self, path: &Path) -> Result<()> {
        // Remove from hash cache
        if self.file_hashes.remove(path).is_none() {
            // File wasn't tracked, nothing to do
            return Ok(());
        }

        tracing::info!(
            component = "file_watcher",
            path = %path.display(),
            "File deleted, resource will be removed on next full sync"
        );

        // Note: For delete, we would need to know the resource kind and name
        // to properly remove it from ConfigServer. This requires either:
        // 1. Caching the resource metadata along with the hash
        // 2. Doing a full re-sync of the directory
        //
        // For now, log the deletion. A full re-sync approach is safer.
        // In practice, admin-api delete should handle ConfigServer update directly.

        Ok(())
    }

    /// Calculate a simple hash of content
    fn hash_content(&self, content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Apply resource change to ConfigServer based on content
    async fn apply_change(&self, content: &str, change: ResourceChange) -> Result<()> {
        apply_resource_change(&self.config_server, content, change)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    // Helper to test is_yaml_file without needing a full FileWatcher instance
    fn check_yaml_file(path: &Path) -> bool {
        path.extension()
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false)
    }

    // Helper to test hash_content
    fn compute_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_is_yaml_file() {
        // Should recognize .yaml files
        assert!(check_yaml_file(Path::new("config.yaml")));
        assert!(check_yaml_file(Path::new("/path/to/gateway.yaml")));

        // Should recognize .yml files
        assert!(check_yaml_file(Path::new("config.yml")));
        assert!(check_yaml_file(Path::new("/path/to/route.yml")));

        // Should reject non-YAML files
        assert!(!check_yaml_file(Path::new("config.json")));
        assert!(!check_yaml_file(Path::new("config.toml")));
        assert!(!check_yaml_file(Path::new("config.txt")));
        assert!(!check_yaml_file(Path::new("config")));
        assert!(!check_yaml_file(Path::new(".yaml"))); // hidden file without name
    }

    #[test]
    fn test_hash_content_same_content() {
        let content = "apiVersion: v1\nkind: Gateway\n";

        let hash1 = compute_hash(content);
        let hash2 = compute_hash(content);

        // Same content should produce same hash
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_content_different_content() {
        let content1 = "apiVersion: v1\nkind: Gateway\n";
        let content2 = "apiVersion: v1\nkind: HTTPRoute\n";

        let hash1 = compute_hash(content1);
        let hash2 = compute_hash(content2);

        // Different content should produce different hash
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_content_whitespace_matters() {
        let content1 = "kind: Gateway";
        let content2 = "kind:  Gateway"; // extra space

        let hash1 = compute_hash(content1);
        let hash2 = compute_hash(content2);

        // Whitespace differences should produce different hash
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_content_empty() {
        // Empty content should still produce a valid hash (just verify it doesn't panic)
        let _hash = compute_hash("");
    }
}
