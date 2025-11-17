use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use notify::{event::{ModifyKind, RenameMode}, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::EventDispatcher;
use crate::core::utils::{extract_resource_metadata, is_base_conf};
use crate::types::ResourceKind;

pub struct FileSystemConfigLoader {
    root: PathBuf,
    dispatcher: Arc<dyn EventDispatcher>,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<PathBuf, String>>>,
    // Track pending renames: from_path -> to_path
    pending_renames: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

// TODO: Support nested directory watch and propagation. Currently only flat file
// updates inside the root directory are handled; directory-level operations are
// ignored with an error log.
impl FileSystemConfigLoader {
    pub fn new<P: Into<PathBuf>>(
        root: P,
        dispatcher: Arc<dyn EventDispatcher>,
        resource_kind: Option<ResourceKind>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            dispatcher,
            resource_kind,
            cache: Arc::new(Mutex::new(HashMap::new())),
            pending_renames: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let root = self.root.clone();
        tokio::spawn(async move {
            if let Err(err) = self.run().await {
                eprintln!(
                    "[FileSystemConfigLoader] watcher exited with error for {:?}: {}",
                    root, err
                );
            }
        })
    }

    async fn dispatch_change(&self, change: ResourceChange, data: String, use_base_conf: bool) {
        let resource_type = self.resource_kind;
        
        // Convert YAML to JSON for dispatcher
        let json_data = match Self::yaml_to_json(&data) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(
                    "Failed to convert YAML to JSON: {}, skipping file",
                    e
                );
                return;
            }
        };
        
        if use_base_conf {
            self.dispatcher
                .apply_base_conf(change, resource_type, json_data, None);
        } else {
            self.dispatcher
                .apply_resource_change(change, resource_type, json_data, None);
        }
    }
    
    fn yaml_to_json(yaml_str: &str) -> Result<String> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml_str)?;
        let json_str = serde_json::to_string(&value)?;
        Ok(json_str)
    }

    async fn read_file(path: &Path) -> Result<String> {
        let content = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read file {:?}", path))?;
        Ok(content)
    }

    async fn process_new_file(&self, path: &Path) -> Result<()> {
        self.process_file_with_change(path, ResourceChange::EventAdd)
            .await
    }

    async fn process_init_file(&self, path: &Path) -> Result<()> {
        self.process_file_with_change(path, ResourceChange::InitAdd)
            .await
    }

    async fn process_file_with_change(
        &self,
        path: &Path,
        change: ResourceChange,
    ) -> Result<()> {

        tracing::info!(
            component = "file_system_loader",
            path = ?path,
            change = ?change,
        );

        if path.is_dir() {
            tracing::warn!(
                component = "file_system_loader",
                event = "directory_not_supported",
                path = ?path,
            );
            log_directory_not_supported(path);
            return Ok(());
        }

        if !path.is_file() {
            tracing::warn!(
                component = "file_system_loader",
                event = "not_a_file",
                path = ?path,
            );
            return Ok(());
        }

        // Only process .yml or .yaml files
        let extension = path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if extension != "yml" && extension != "yaml" {
            tracing::debug!(
                component = "file_system_loader",
                event = "skip_non_yaml_file",
                path = ?path,
                extension = extension,
            );
            return Ok(());
        }

        let content = Self::read_file(path).await?;
        self.cache
            .lock()
            .await
            .insert(path.to_path_buf(), content.clone());
        
        // Extract resource metadata for logging
        if let Some(metadata) = extract_resource_metadata(&content) {
            let kind_str = metadata.kind.as_deref().unwrap_or("Unknown");
            let name_str = metadata.name.as_deref().unwrap_or("Unknown");
            let namespace_str = metadata.namespace.as_deref();
            
            if let Some(ns) = namespace_str {
                tracing::info!(
                    component = "file_system_loader",
                    event = "processing_file",
                    path = ?path,
                    change = ?change,
                    kind = kind_str,
                    namespace = ns,
                    name = name_str,
                    "Processing resource file"
                );
            } else {
                tracing::info!(
                    component = "file_system_loader",
                    event = "processing_file",
                    path = ?path,
                    change = ?change,
                    kind = kind_str,
                    name = name_str,
                    "Processing cluster-scoped resource file"
                );
            }
        }
        
        // Determine if this is a base conf resource
        let use_base_conf = is_base_conf(&content);
        self.dispatch_change(change, content, use_base_conf).await;
        Ok(())
    }

    async fn process_removed_file(&self, path: &Path) -> Result<()> {
        tracing::debug!(
            component = "file_system_loader",
            event = "process_removed_file",
            path = ?path,
            "Processing file removal"
        );
        
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(path) {
            let use_base_conf = is_base_conf(&old);
            
            // Extract resource metadata for logging
            if let Some(metadata) = extract_resource_metadata(&old) {
                let kind_str = metadata.kind.as_deref().unwrap_or("Unknown");
                let name_str = metadata.name.as_deref().unwrap_or("Unknown");
                let namespace_str = metadata.namespace.as_deref();
                
                drop(cache);
                if let Some(ns) = namespace_str {
                    tracing::info!(
                        component = "file_system_loader",
                        event = "file_removed",
                        path = ?path,
                        use_base_conf = use_base_conf,
                        kind = kind_str,
                        namespace = ns,
                        name = name_str,
                        "File found in cache, dispatching delete event"
                    );
                } else {
                    tracing::info!(
                        component = "file_system_loader",
                        event = "file_removed",
                        path = ?path,
                        use_base_conf = use_base_conf,
                        kind = kind_str,
                        name = name_str,
                        "File found in cache, dispatching delete event (cluster-scoped)"
                    );
                }
            } else {
                drop(cache);
                tracing::info!(
                    component = "file_system_loader",
                    event = "file_removed",
                    path = ?path,
                    use_base_conf = use_base_conf,
                    "File found in cache, dispatching delete event (metadata extraction failed)"
                );
            }
            
            self.dispatch_change(ResourceChange::EventDelete, old, use_base_conf).await;
        } else {
            let has_children = cache.keys().any(|entry| entry.starts_with(path));
            drop(cache);
            if has_children {
                log_directory_not_supported(path);
            } else {
                tracing::warn!(
                    component = "file_system_loader",
                    event = "file_not_in_cache",
                    path = ?path,
                    "File removed but not found in cache, skipping delete event"
                );
            }
        }
        Ok(())
    }

    async fn process_updated_file(&self, path: &Path) -> Result<()> {
        if path.is_dir() {
            log_directory_not_supported(path);
            return Ok(());
        }

        if !path.is_file() {
            return Ok(());
        }

        let new_content = Self::read_file(path).await?;
        let use_base_conf = is_base_conf(&new_content);
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(path) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old, use_base_conf).await;
        }
        let mut cache = self.cache.lock().await;
        cache.insert(path.to_path_buf(), new_content.clone());
        drop(cache);
        self.dispatch_change(ResourceChange::EventAdd, new_content, use_base_conf)
            .await;
        Ok(())
    }

    async fn handle_event(&self, event: Event) -> Result<()> {
        match event.kind {
            EventKind::Create(_) => {
                for path in event.paths.clone() {
                    // Check if this is a rename target (file moved in)
                    let mut pending_renames = self.pending_renames.lock().await;
                    let is_rename = pending_renames.values().any(|to_path| to_path == &path);
                    if is_rename {
                        // Find the source path
                        let from_path = pending_renames.iter()
                            .find(|(_, to_path)| to_path == &&path)
                            .map(|(from, _)| from.clone());
                        if let Some(from_path) = from_path {
                            pending_renames.remove(&from_path);
                            drop(pending_renames);
                            // Process as rename: remove old, add new
                            self.process_removed_file(&from_path).await?;
                            self.process_new_file(&path).await?;
                        } else {
                            drop(pending_renames);
                            self.process_new_file(&path).await?;
                        }
                    } else {
                        drop(pending_renames);
                        // Regular create event (new file or file moved in from outside)
                        self.process_new_file(&path).await?;
                    }
                }
            }
            EventKind::Modify(modify_kind) => match modify_kind {
                ModifyKind::Data(_) | ModifyKind::Metadata(_) => {
                    for path in event.paths.clone() {
                        self.process_updated_file(&path).await?;
                    }
                }
                ModifyKind::Name(rename_mode) => {
                    let paths = event.paths.clone();
                    match rename_mode {
                        RenameMode::Both => {
                            // Both paths in one event
                            if paths.len() == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "unexpected_rename_paths",
                                    path_count = paths.len(),
                                    "Expected 2 paths for RenameMode::Both, got {}",
                                    paths.len()
                                );
                            }
                        }
                        RenameMode::From => {
                            // Source path (old path) - store for potential matching To event
                            for from_path in paths.clone() {
                                tracing::debug!(
                                    component = "file_system_loader",
                                    event = "rename_from",
                                    path = ?from_path,
                                    "Received RenameMode::From event"
                                );
                                
                                // Check if source is in watched directory
                                let from_in_watched = from_path.starts_with(&self.root);
                                
                                // Store the source path, we'll match it with To or Create event
                                // But only if source is in watched directory (might be moved within)
                                let mut pending_renames = self.pending_renames.lock().await;
                                if from_in_watched {
                                    // Use a placeholder to_path for now, will be updated when we see To/Create
                                    pending_renames.insert(from_path.clone(), PathBuf::new());
                                }
                                drop(pending_renames);
                                
                                // Process removal immediately
                                // If file is moved outside watched directory, this will handle deletion
                                // If moved within, the To/Create event will handle the new location
                                self.process_removed_file(&from_path).await?;
                            }
                        }
                        RenameMode::To => {
                            // Target path (new path) - match with pending From
                            for to_path in paths.clone() {
                                let mut pending_renames = self.pending_renames.lock().await;
                                // Find matching From path (same directory or any pending)
                                let mut matched_from = None;
                                for (from_path, _) in pending_renames.iter() {
                                    // Try to match by same directory first
                                    if from_path.parent() == to_path.parent() {
                                        matched_from = Some(from_path.clone());
                                        break;
                                    }
                                }
                                // If no same-directory match, use any pending From
                                if matched_from.is_none() {
                                    matched_from = pending_renames.keys().next().cloned();
                                }
                                if let Some(from_path) = matched_from {
                                    pending_renames.remove(&from_path);
                                    drop(pending_renames);
                                    // Process as rename: From already processed as removal, now add new
                                    self.process_new_file(&to_path).await?;
                                } else {
                                    drop(pending_renames);
                                    // No matching From, treat as new file
                                    self.process_new_file(&to_path).await?;
                                }
                            }
                        }
                        RenameMode::Any => {
                            // Fallback: try to handle as Both if 2 paths, otherwise process individually
                            let path_count = paths.len();
                            if path_count == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else if path_count == 1 {
                                // Single path: could be:
                                // 1. File moved in from outside (Create) - not in cache, file exists
                                // 2. File moved out (Delete) - in cache, file doesn't exist or moved outside
                                // 3. File renamed within - in cache, file exists
                                for path in paths {
                                    let cache = self.cache.lock().await;
                                    let is_in_cache = cache.contains_key(&path);
                                    drop(cache);
                                    
                                    // Check if path is still in watched directory
                                    let is_in_watched_dir = path.starts_with(&self.root);
                                    
                                    if is_in_cache {
                                        // File was tracked, check if it still exists and is in watched directory
                                        if path.exists() && is_in_watched_dir {
                                            // File still exists in watched directory, treat as update/rename
                                            self.process_updated_file(&path).await?;
                                        } else {
                                            // File moved outside or deleted, treat as removal
                                            tracing::debug!(
                                                component = "file_system_loader",
                                                event = "rename_any_single_path_removed",
                                                path = ?path,
                                                exists = path.exists(),
                                                in_watched = is_in_watched_dir,
                                                "RenameMode::Any with single path in cache but file moved outside or deleted, treating as removal"
                                            );
                                            self.process_removed_file(&path).await?;
                                        }
                                    } else {
                                        // File not in cache, likely moved in from outside, treat as new
                                        if path.exists() && is_in_watched_dir {
                                            tracing::debug!(
                                                component = "file_system_loader",
                                                event = "rename_any_single_path_new",
                                                path = ?path,
                                                "RenameMode::Any with single path not in cache, treating as new file"
                                            );
                                            self.process_new_file(&path).await?;
                                        } else {
                                            tracing::warn!(
                                                component = "file_system_loader",
                                                event = "rename_any_single_path_skipped",
                                                path = ?path,
                                                exists = path.exists(),
                                                in_watched = is_in_watched_dir,
                                                "RenameMode::Any with single path not in cache and file doesn't exist or outside watched dir, skipping"
                                            );
                                        }
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "unexpected_rename_any",
                                    path_count = path_count,
                                    "RenameMode::Any with {} paths, skipping",
                                    path_count
                                );
                            }
                        }
                        RenameMode::Other => {
                            // Unknown rename mode, log and try to handle
                            tracing::warn!(
                                component = "file_system_loader",
                                event = "unknown_rename_mode",
                                path_count = paths.len(),
                                "Unknown rename mode, attempting to handle"
                            );
                            if paths.len() == 2 {
                                let old = paths[0].clone();
                                let new = paths[1].clone();
                                self.process_removed_file(&old).await?;
                                self.process_new_file(&new).await?;
                            } else {
                                for path in paths {
                                    self.process_updated_file(&path).await?;
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            EventKind::Remove(_) => {
                for path in event.paths.clone() {
                    tracing::debug!(
                        component = "file_system_loader",
                        event = "remove_event",
                        path = ?path,
                        "Received Remove event"
                    );
                    
                    // Check if this path is within the watched root directory
                    // If not, it might be a file moved outside, but we should still process it
                    let is_in_watched_dir = path.starts_with(&self.root);
                    
                    // Track removed files - if we see a Create event soon, it's a move
                    // Otherwise, it's a deletion
                    let mut pending_renames = self.pending_renames.lock().await;
                    // Store the removed path with empty target (will be updated if Create follows)
                    // But only if the file is still in the watched directory (might be moved within)
                    if is_in_watched_dir {
                        pending_renames.insert(path.clone(), PathBuf::new());
                    }
                    drop(pending_renames);
                    
                    // Process as removal immediately
                    // If it's actually a move within the directory, the Create event will handle adding the new file
                    // If it's moved outside, this will process the deletion
                    self.process_removed_file(&path).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn log_directory_not_supported(path: &Path) {
    eprintln!(
        "[FileSystemConfigLoader] directory changes are not supported: {:?}",
        path
    );
}

#[async_trait::async_trait]
impl ConfigLoader for FileSystemConfigLoader {
    /// Connect to filesystem (no-op for filesystem loader)
    async fn connect(&self) -> Result<()> {
        // Filesystem doesn't need connection setup
        if !self.root.exists() {
            return Err(anyhow!("Config directory {:?} does not exist", self.root));
        }
        Ok(())
    }

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    async fn bootstrap_base_conf(&self) -> Result<()> {
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .with_context(|| format!("Failed to read directory {:?}", dir))?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    // Only process base conf files
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match Self::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed_to_read_file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };
                        
                        if is_base_conf(&content) {
                            self.process_init_file(&path).await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> Result<()> {
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .with_context(|| format!("Failed to read directory {:?}", dir))?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    // Only process non-base conf files
                    if path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "yml" || ext == "yaml")
                        .unwrap_or(false)
                    {
                        let content = match Self::read_file(&path).await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    component = "file_system_loader",
                                    event = "failed_to_read_file",
                                    path = ?path,
                                    error = %e,
                                );
                                continue;
                            }
                        };
                        
                        if !is_base_conf(&content) {
                            self.process_init_file(&path).await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Set ready state after initialization
    async fn set_ready(&self) {
        self.dispatcher.set_ready();
    }

    /// Main run loop for watching configuration changes
    async fn run(&self) -> Result<()> {
        // Start watching for changes
        let (tx, mut rx) = mpsc::channel::<Result<Event>>(128);
        let tx_watch = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let _ = tx_watch.blocking_send(res.map_err(|e| anyhow!(e)));
            },
            notify::Config::default(),
        )?;

        watcher
            .watch(self.root.as_path(), RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch directory {:?}", self.root))?;

        while let Some(event) = rx.recv().await {
            match event {
                Ok(event) => {
                    if let Err(err) = self.handle_event(event).await {
                        eprintln!(
                            "[FileSystemConfigLoader] failed to handle event in {:?}: {}",
                            self.root, err
                        );
                    }
                }
                Err(err) => {
                    eprintln!(
                        "[FileSystemConfigLoader] watcher error in {:?}: {}",
                        self.root, err
                    );
                }
            }
        }

        Ok(())
    }
}
