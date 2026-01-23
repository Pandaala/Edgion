//! FileSystemWatcher - Centralized file watching with event dispatch
//!
//! Provides a unified inotify watch that dispatches events to per-kind channels.
//! This allows each resource type to have its own independent event stream,
//! similar to how K8s mode works with separate watchers per resource.

use super::event::{FileSystemEvent, ParsedFileInfo};
use crate::core::conf_mgr_new::sync_runtime::ShutdownSignal;
use crate::types::ResourceKind;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Sender type for parsed resource events per kind
pub type KindEventSender = broadcast::Sender<FileSystemEvent<String>>;
/// Receiver type for parsed resource events per kind
pub type KindEventReceiver = broadcast::Receiver<FileSystemEvent<String>>;

/// FileSystemWatcher - Centralized file watching with event dispatch by kind
///
/// Architecture:
/// ```text
/// [inotify] -> [debouncer] -> [dispatcher] -> [kind channels]
///                                  │
///                                  ├── HTTPRoute channel
///                                  ├── Gateway channel
///                                  ├── Secret channel
///                                  └── ... other channels
/// ```
pub struct FileSystemWatcher {
    /// Configuration directory to watch
    conf_dir: PathBuf,
    
    /// Event senders per resource kind
    /// Key is kind name (e.g., "HTTPRoute", "Gateway")
    senders: Arc<RwLock<HashMap<&'static str, KindEventSender>>>,
}

impl FileSystemWatcher {
    /// Create a new FileSystemWatcher
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            conf_dir,
            senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a kind and get its event receiver
    ///
    /// Must be called before `run()` for each resource kind that needs events.
    pub async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        let mut senders = self.senders.write().await;
        
        if let Some(sender) = senders.get(kind) {
            sender.subscribe()
        } else {
            // Create new broadcast channel with reasonable capacity
            let (tx, rx) = broadcast::channel(256);
            senders.insert(kind, tx);
            rx
        }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }

    /// Run the watcher: init phase + runtime phase
    ///
    /// This method:
    /// 1. Init phase: Scans directory, dispatches Init/InitApply/InitDone per kind
    /// 2. Runtime phase: Watches for changes, dispatches Apply/Delete events
    pub async fn run(&self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting FileSystemWatcher"
        );

        // Phase 1: Init - scan directory and dispatch events
        self.init_phase().await?;

        // Phase 2: Runtime - watch for changes
        self.runtime_phase(&mut shutdown_signal).await?;

        tracing::info!(
            component = "fs_watcher",
            "FileSystemWatcher stopped"
        );

        Ok(())
    }

    /// Init phase: scan directory and dispatch Init/InitApply/InitDone events per kind
    pub async fn init_phase(&self) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting init phase"
        );

        // Collect files grouped by kind
        let mut files_by_kind: HashMap<&'static str, Vec<(PathBuf, String)>> = HashMap::new();

        // Scan directory
        let entries = std::fs::read_dir(&self.conf_dir)
            .with_context(|| format!("Failed to read directory: {}", self.conf_dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Skip non-YAML files
            if !is_yaml_file(&path) {
                continue;
            }

            // Read file content
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        component = "fs_watcher",
                        path = %path.display(),
                        error = %e,
                        "Failed to read file during init"
                    );
                    continue;
                }
            };

            // Determine kind from content
            if let Some(kind) = ResourceKind::from_content(&content) {
                let kind_str = kind.as_str();
                files_by_kind
                    .entry(kind_str)
                    .or_default()
                    .push((path, content));
            }
        }

        // Dispatch events per kind
        let senders = self.senders.read().await;
        
        for (kind, files) in &files_by_kind {
            if let Some(sender) = senders.get(kind) {
                // Send Init
                let _ = sender.send(FileSystemEvent::Init);

                // Send InitApply for each file
                for (path, content) in files {
                    // The content is sent as the event payload
                    // ResourceController will parse it to the actual type
                    let _ = sender.send(FileSystemEvent::InitApply(content.clone()));
                    
                    tracing::trace!(
                        component = "fs_watcher",
                        kind = kind,
                        path = %path.display(),
                        "Dispatched InitApply event"
                    );
                }

                // Send InitDone
                let _ = sender.send(FileSystemEvent::InitDone);

                tracing::debug!(
                    component = "fs_watcher",
                    kind = kind,
                    count = files.len(),
                    "Completed init for kind"
                );
            }
        }

        // Send Init/InitDone for kinds that have subscribers but no files
        for (kind, sender) in senders.iter() {
            if !files_by_kind.contains_key(kind) {
                let _ = sender.send(FileSystemEvent::Init);
                let _ = sender.send(FileSystemEvent::InitDone);
                
                tracing::debug!(
                    component = "fs_watcher",
                    kind = kind,
                    "Sent empty init for kind (no files found)"
                );
            }
        }

        let total_files: usize = files_by_kind.values().map(|v| v.len()).sum();
        tracing::info!(
            component = "fs_watcher",
            total_files = total_files,
            kinds = files_by_kind.len(),
            "Init phase complete"
        );

        Ok(())
    }

    /// Runtime phase: watch for file changes and dispatch Apply/Delete events
    async fn runtime_phase(&self, shutdown_signal: &mut ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_watcher",
            conf_dir = %self.conf_dir.display(),
            "Starting runtime phase"
        );

        // Create channel for raw file events
        let (tx, mut rx) = mpsc::channel::<Event>(100);

        // Create file watcher
        let conf_dir = self.conf_dir.clone();
        let watcher = tokio::task::spawn_blocking(move || -> Result<RecommendedWatcher> {
            let tx_clone = tx.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        match event.kind {
                            EventKind::Access(_) => {} // Ignore read-only access
                            _ => {
                                let _ = tx_clone.blocking_send(event);
                            }
                        }
                    }
                },
                Config::default().with_poll_interval(Duration::from_secs(2)),
            )
            .context("Failed to create file watcher")?;

            watcher
                .watch(&conf_dir, RecursiveMode::NonRecursive)
                .context("Failed to start watching directory")?;

            Ok(watcher)
        })
        .await??;

        // Debounce and dispatch events
        let mut pending_paths: HashSet<PathBuf> = HashSet::new();
        let debounce_duration = Duration::from_secs(1);
        let mut debounce_timer = tokio::time::interval(debounce_duration);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    for path in event.paths {
                        if is_yaml_file(&path) {
                            pending_paths.insert(path);
                        }
                    }
                }
                _ = debounce_timer.tick() => {
                    if !pending_paths.is_empty() {
                        for path in pending_paths.drain() {
                            self.dispatch_file_event(&path).await;
                        }
                    }
                }
                _ = shutdown_signal.wait() => {
                    tracing::info!(
                        component = "fs_watcher",
                        "Received shutdown signal"
                    );
                    break;
                }
            }
        }

        // Explicitly drop watcher to stop file monitoring
        drop(watcher);

        Ok(())
    }

    /// Dispatch a file event (Apply or Delete) to the appropriate kind channel
    async fn dispatch_file_event(&self, path: &Path) {
        // Parse file info from filename
        let Some(info) = parse_resource_filename(path) else {
            tracing::trace!(
                component = "fs_watcher",
                path = %path.display(),
                "Skipping file: not a valid resource filename"
            );
            return;
        };

        let senders = self.senders.read().await;
        let Some(sender) = senders.get(info.kind.as_str()) else {
            tracing::trace!(
                component = "fs_watcher",
                kind = %info.kind,
                path = %path.display(),
                "No subscriber for kind"
            );
            return;
        };

        if path.exists() {
            // File exists -> Apply event
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let _ = sender.send(FileSystemEvent::Apply(content));
                    tracing::debug!(
                        component = "fs_watcher",
                        kind = %info.kind,
                        key = %info.key,
                        "Dispatched Apply event"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        component = "fs_watcher",
                        path = %path.display(),
                        error = %e,
                        "Failed to read file for Apply event"
                    );
                }
            }
        } else {
            // File doesn't exist -> Delete event
            // For delete, we send the key as content so the controller knows what to delete
            let delete_info = format!("__DELETE__:{}:{}", info.kind, info.key);
            let _ = sender.send(FileSystemEvent::Delete(delete_info));
            tracing::debug!(
                component = "fs_watcher",
                kind = %info.kind,
                key = %info.key,
                "Dispatched Delete event"
            );
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Check if a path is a YAML file
fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext == "yaml" || ext == "yml")
        .unwrap_or(false)
}

/// Parse resource info from filename
///
/// Filename format:
/// - With namespace: `{Kind}_{namespace}_{name}.yaml`
/// - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)
///
/// Returns ParsedFileInfo with kind, namespace, name, and key
fn parse_resource_filename(path: &Path) -> Option<ParsedFileInfo> {
    let filename = path.file_stem()?.to_str()?;

    // Split into at most 3 parts by underscore
    let parts: Vec<&str> = filename.splitn(3, '_').collect();

    if parts.len() == 3 {
        let kind = parts[0].to_string();
        let namespace = if parts[1].is_empty() {
            None // Double underscore means cluster-scoped
        } else {
            Some(parts[1].to_string())
        };
        let name = parts[2].to_string();
        Some(ParsedFileInfo::new(kind, namespace, name))
    } else {
        None
    }
}

/// Build file path from kind and key
///
/// Key format:
/// - Namespaced: "namespace/name"
/// - Cluster-scoped: "name"
pub fn build_path_from_key(conf_dir: &Path, kind: &str, key: &str) -> PathBuf {
    let (namespace, name) = if let Some(pos) = key.find('/') {
        (Some(&key[..pos]), &key[pos + 1..])
    } else {
        (None, key)
    };

    let filename = match namespace {
        Some(ns) => format!("{}_{}_{}.yaml", kind, ns, name),
        None => format!("{}__{}.yaml", kind, name),
    };

    conf_dir.join(filename)
}
