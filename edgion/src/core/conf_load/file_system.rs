use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use notify::{event::{ModifyKind, RenameMode}, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::EventDispatcher;
use crate::core::utils::{extract_resource_metadata, is_base_conf, ResourceMetadata};
use crate::types::ResourceKind;

pub struct FileSystemConfigLoader {
    root: PathBuf,
    dispatcher: Arc<dyn EventDispatcher>,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<PathBuf, String>>>,
    // Track pending renames: from_path -> to_path
    pending_renames: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
    // Track resource metadata to file paths mapping for duplicate detection
    // Key: ResourceMetadata (kind/namespace/name), Value: Vec<PathBuf> (file paths)
    resource_to_files: Arc<Mutex<HashMap<ResourceMetadata, Vec<PathBuf>>>>,
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
        let loader = Arc::new(Self {
            root: root.into(),
            dispatcher,
            resource_kind,
            cache: Arc::new(Mutex::new(HashMap::new())),
            pending_renames: Arc::new(Mutex::new(HashMap::new())),
            resource_to_files: Arc::new(Mutex::new(HashMap::new())),
        });
        
        // Spawn duplicate detection task
        let loader_clone = loader.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                loader_clone.check_duplicate_resources().await;
            }
        });
        
        loader
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
    
    /// Normalize a path to its absolute canonical form
    /// This ensures that relative and absolute paths pointing to the same file are treated as identical
    async fn normalize_path(&self, path: &Path) -> PathBuf {
        // Try to canonicalize first (resolves symlinks and makes absolute)
        if let Ok(canonical) = fs::canonicalize(path).await {
            return canonical;
        }
        
        // If canonicalize fails (e.g., file doesn't exist), try to make it absolute
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // For relative paths, resolve relative to the loader's root directory
            // This is more reliable than using current working directory
            let root_canonical = fs::canonicalize(&self.root).await.ok();
            if let Some(root) = root_canonical {
                root.join(path)
            } else {
                // Fallback: try current working directory
                std::env::current_dir()
                    .ok()
                    .map(|cwd| cwd.join(path))
                    .unwrap_or_else(|| path.to_path_buf())
            }
        }
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
        
        // Update resource_to_files mapping
        self.update_resource_to_files(path, &content).await;
        
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
        
        // Check if this resource has duplicate files before removing from mapping
        let normalized_path = self.normalize_path(path).await;
        let resource_to_files = self.resource_to_files.lock().await;
        
        // Find the resource metadata for this file
        let mut resource_metadata: Option<(ResourceMetadata, Vec<PathBuf>)> = None;
        for (metadata, files) in resource_to_files.iter() {
            if files.contains(&normalized_path) {
                let mut remaining_files = files.clone();
                remaining_files.retain(|p| p != &normalized_path);
                resource_metadata = Some((metadata.clone(), remaining_files));
                break;
            }
        }
        drop(resource_to_files);
        
        // Try to get content from cache first (most reliable)
        let mut cache = self.cache.lock().await;
        let cached_content = cache.remove(path);
        drop(cache);
        
        let content_to_delete = if let Some(cached) = cached_content {
            // Use cached content
            tracing::debug!(
                component = "file_system_loader",
                event = "using_cached_content",
                path = ?path,
                "Using cached content for deletion"
            );
            Some(cached)
        } else if path.exists() && path.is_file() {
            // File still exists, try to read it (might be a race condition)
            tracing::debug!(
                component = "file_system_loader",
                event = "reading_file_for_deletion",
                path = ?path,
                "File not in cache but still exists, reading for deletion"
            );
            match Self::read_file(path).await {
                Ok(content) => Some(content),
                Err(e) => {
                    tracing::warn!(
                        component = "file_system_loader",
                        event = "failed_to_read_file",
                        path = ?path,
                        error = %e,
                        "Failed to read file for deletion, skipping"
                    );
                    None
                }
            }
        } else {
            // File doesn't exist and not in cache
            // Try to get content from other files defining the same resource
            if let Some((_metadata, remaining_files)) = &resource_metadata {
                if !remaining_files.is_empty() {
                    // Try to read from the first remaining file to get resource content
                    let first_file = &remaining_files[0];
                    tracing::debug!(
                        component = "file_system_loader",
                        event = "reading_duplicate_file_for_deletion",
                        path = ?path,
                        duplicate_file = ?first_file,
                        "File not in cache, trying to read from duplicate file for deletion"
                    );
                    match Self::read_file(first_file).await {
                        Ok(content) => {
                            tracing::info!(
                                component = "file_system_loader",
                                event = "using_duplicate_content_for_deletion",
                                path = ?path,
                                duplicate_file = ?first_file,
                                "Using content from duplicate file for deletion"
                            );
                            Some(content)
                        }
                        Err(e) => {
                            tracing::warn!(
                                component = "file_system_loader",
                                event = "failed_to_read_duplicate_file",
                                path = ?path,
                                duplicate_file = ?first_file,
                                error = %e,
                                "Failed to read duplicate file for deletion"
                            );
                            None
                        }
                    }
                } else {
                    // No remaining files, check if it's a directory
                    let cache = self.cache.lock().await;
                    let has_children = cache.keys().any(|entry| entry.starts_with(path));
                    drop(cache);
                    
                    if has_children {
                        log_directory_not_supported(path);
                    } else {
                        tracing::warn!(
                            component = "file_system_loader",
                            event = "file_not_in_cache_and_missing",
                            path = ?path,
                            "File removed but not found in cache and file doesn't exist, cannot delete resource"
                        );
                    }
                    None
                }
            } else {
                // Not found in resource_to_files, check if it's a directory
                let cache = self.cache.lock().await;
                let has_children = cache.keys().any(|entry| entry.starts_with(path));
                drop(cache);
                
                if has_children {
                    log_directory_not_supported(path);
                } else {
                    tracing::warn!(
                        component = "file_system_loader",
                        event = "file_not_in_cache_and_missing",
                        path = ?path,
                        "File removed but not found in cache and file doesn't exist, cannot delete resource"
                    );
                }
                None
            }
        };
        
        // Remove file from resource_to_files mapping
        self.remove_resource_from_files(path).await;
        
        // Dispatch delete event or update with first remaining file
        if let Some(old_content) = content_to_delete {
            let use_base_conf = is_base_conf(&old_content);
            
            // Check if there are other files defining the same resource
            if let Some((_metadata, remaining_files)) = resource_metadata {
                if !remaining_files.is_empty() {
                    // There are other files, use the first one for update instead of delete
                    let first_file = &remaining_files[0];
                    
                    // Extract metadata for logging
                    if let Some(metadata_extracted) = extract_resource_metadata(&old_content) {
                        let kind_str = metadata_extracted.kind.as_deref().unwrap_or("Unknown");
                        let name_str = metadata_extracted.name.as_deref().unwrap_or("Unknown");
                        let namespace_str = metadata_extracted.namespace.as_deref();
                        
                        if let Some(ns) = namespace_str {
                            tracing::info!(
                                component = "file_system_loader",
                                event = "file_removed_with_duplicate",
                                path = ?path,
                                remaining_file = ?first_file,
                                kind = kind_str,
                                namespace = ns,
                                name = name_str,
                                "File removed but resource has duplicates, updating with first remaining file instead of deleting"
                            );
                        } else {
                            tracing::info!(
                                component = "file_system_loader",
                                event = "file_removed_with_duplicate",
                                path = ?path,
                                remaining_file = ?first_file,
                                kind = kind_str,
                                name = name_str,
                                "File removed but resource has duplicates, updating with first remaining file instead of deleting (cluster-scoped)"
                            );
                        }
                    }
                    
                    // Read the first remaining file and update the resource
                    match Self::read_file(first_file).await {
                        Ok(new_content) => {
                            let new_use_base_conf = is_base_conf(&new_content);
                            
                            // Delete old resource first, then add new resource
                            // This ensures correct handling if namespace or other fields changed
                            self.dispatch_change(ResourceChange::EventDelete, old_content, use_base_conf).await;
                            self.dispatch_change(ResourceChange::EventAdd, new_content, new_use_base_conf).await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                component = "file_system_loader",
                                event = "failed_to_read_remaining_file",
                                path = ?first_file,
                                error = %e,
                                "Failed to read remaining file for update, falling back to delete"
                            );
                            // Fallback to delete if we can't read the remaining file
                            self.dispatch_change(ResourceChange::EventDelete, old_content, use_base_conf).await;
                        }
                    }
                } else {
                    // No remaining files, proceed with normal delete
                    if let Some(metadata_extracted) = extract_resource_metadata(&old_content) {
                        let kind_str = metadata_extracted.kind.as_deref().unwrap_or("Unknown");
                        let name_str = metadata_extracted.name.as_deref().unwrap_or("Unknown");
                        let namespace_str = metadata_extracted.namespace.as_deref();
                        
                        if let Some(ns) = namespace_str {
                            tracing::info!(
                                component = "file_system_loader",
                                event = "file_removed",
                                path = ?path,
                                use_base_conf = use_base_conf,
                                kind = kind_str,
                                namespace = ns,
                                name = name_str,
                                "Dispatching delete event for removed file"
                            );
                        } else {
                            tracing::info!(
                                component = "file_system_loader",
                                event = "file_removed",
                                path = ?path,
                                use_base_conf = use_base_conf,
                                kind = kind_str,
                                name = name_str,
                                "Dispatching delete event for removed file (cluster-scoped)"
                            );
                        }
                    } else {
                        tracing::info!(
                            component = "file_system_loader",
                            event = "file_removed",
                            path = ?path,
                            use_base_conf = use_base_conf,
                            "Dispatching delete event (metadata extraction failed)"
                        );
                    }
                    
                    self.dispatch_change(ResourceChange::EventDelete, old_content, use_base_conf).await;
                }
            } else {
                // No metadata found, proceed with normal delete
                if let Some(metadata_extracted) = extract_resource_metadata(&old_content) {
                    let kind_str = metadata_extracted.kind.as_deref().unwrap_or("Unknown");
                    let name_str = metadata_extracted.name.as_deref().unwrap_or("Unknown");
                    let namespace_str = metadata_extracted.namespace.as_deref();
                    
                    if let Some(ns) = namespace_str {
                        tracing::info!(
                            component = "file_system_loader",
                            event = "file_removed",
                            path = ?path,
                            use_base_conf = use_base_conf,
                            kind = kind_str,
                            namespace = ns,
                            name = name_str,
                            "Dispatching delete event for removed file"
                        );
                    } else {
                        tracing::info!(
                            component = "file_system_loader",
                            event = "file_removed",
                            path = ?path,
                            use_base_conf = use_base_conf,
                            kind = kind_str,
                            name = name_str,
                            "Dispatching delete event for removed file (cluster-scoped)"
                        );
                    }
                } else {
                    tracing::info!(
                        component = "file_system_loader",
                        event = "file_removed",
                        path = ?path,
                        use_base_conf = use_base_conf,
                        "Dispatching delete event (metadata extraction failed)"
                    );
                }
                
                self.dispatch_change(ResourceChange::EventDelete, old_content, use_base_conf).await;
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
        let new_use_base_conf = is_base_conf(&new_content);
        
        // Get old content from cache and update cache with new content in one lock
        let mut cache = self.cache.lock().await;
        let old_content = cache.remove(path);
        cache.insert(path.to_path_buf(), new_content.clone());
        drop(cache);
        
        // Remove old resource from resource_to_files mapping before processing
        if old_content.is_some() {
            self.remove_resource_from_files(path).await;
        }
        
        // Update resource_to_files mapping with new content
        self.update_resource_to_files(path, &new_content).await;
        
        // Always delete old resource first, then add new resource
        // This is necessary because namespace or other identifying fields might have changed
        // Using delete+add instead of update ensures correct handling of namespace changes
        if let Some(old_content) = old_content {
            let old_use_base_conf = is_base_conf(&old_content);
            
            // Extract metadata for logging
            if let Some(old_metadata) = extract_resource_metadata(&old_content) {
                let old_kind = old_metadata.kind.as_deref().unwrap_or("Unknown");
                let old_name = old_metadata.name.as_deref().unwrap_or("Unknown");
                let old_namespace = old_metadata.namespace.as_deref();
                
                if let Some(new_metadata) = extract_resource_metadata(&new_content) {
                    let new_kind = new_metadata.kind.as_deref().unwrap_or("Unknown");
                    let new_name = new_metadata.name.as_deref().unwrap_or("Unknown");
                    let new_namespace = new_metadata.namespace.as_deref();
                    
                    // Check if namespace or name changed
                    let namespace_changed = old_namespace != new_namespace;
                    let name_changed = old_name != new_name;
                    let kind_changed = old_kind != new_kind;
                    
                    if namespace_changed || name_changed || kind_changed {
                        tracing::info!(
                            component = "file_system_loader",
                            event = "file_modified_with_identity_change",
                            path = ?path,
                            old_kind = old_kind,
                            old_namespace = ?old_namespace,
                            old_name = old_name,
                            new_kind = new_kind,
                            new_namespace = ?new_namespace,
                            new_name = new_name,
                            "File modified with identity change, using delete+add instead of update"
                        );
                    }
                }
            }
            
            // Delete old resource (use old content's use_base_conf flag)
            self.dispatch_change(ResourceChange::EventDelete, old_content, old_use_base_conf).await;
        }
        
        // Add the new resource (use new content's use_base_conf flag)
        self.dispatch_change(ResourceChange::EventAdd, new_content, new_use_base_conf)
            .await;
        Ok(())
    }
    
    /// Update resource_to_files mapping when a file is added or updated
    async fn update_resource_to_files(&self, path: &Path, content: &str) {
        if let Some(metadata) = extract_resource_metadata(content) {
            let mut resource_to_files = self.resource_to_files.lock().await;
            // Normalize path to avoid duplicates from relative/absolute path differences
            let normalized_path = self.normalize_path(path).await;
            let files = resource_to_files
                .entry(metadata)
                .or_insert_with(Vec::new);
            
            // Only add if not already present (avoid duplicates)
            if !files.contains(&normalized_path) {
                files.push(normalized_path);
            }
        }
    }
    
    /// Remove a file from resource_to_files mapping
    async fn remove_resource_from_files(&self, path: &Path) {
        let mut resource_to_files = self.resource_to_files.lock().await;
        
        // Normalize path to match against normalized paths in the map
        let normalized_path = self.normalize_path(path).await;
        
        // Find and remove the normalized path from all resource entries
        let mut to_remove = Vec::new();
        for (metadata, files) in resource_to_files.iter_mut() {
            files.retain(|p| p != &normalized_path);
            if files.is_empty() {
                to_remove.push(metadata.clone());
            }
        }
        
        // Remove entries with empty file lists
        for metadata in to_remove {
            resource_to_files.remove(&metadata);
        }
    }
    
    /// Check for duplicate resources (multiple files pointing to the same resource)
    async fn check_duplicate_resources(&self) {
        let resource_to_files = self.resource_to_files.lock().await;
        
        for (metadata, files) in resource_to_files.iter() {
            if files.len() > 1 {
                let kind_str = metadata.kind.as_deref().unwrap_or("Unknown");
                let name_str = metadata.name.as_deref().unwrap_or("Unknown");
                let namespace_str = metadata.namespace.as_deref();
                
                let files_str: Vec<String> = files.iter()
                    .map(|p| p.display().to_string())
                    .collect();
                
                if let Some(ns) = namespace_str {
                    tracing::error!(
                        component = "file_system_loader",
                        event = "duplicate_resource_detected",
                        kind = kind_str,
                        namespace = ns,
                        name = name_str,
                        file_count = files.len(),
                        files = ?files_str,
                        "Multiple files define the same resource: kind={}, namespace={}, name={}. Files: {:?}",
                        kind_str, ns, name_str, files_str
                    );
                } else {
                    tracing::error!(
                        component = "file_system_loader",
                        event = "duplicate_resource_detected",
                        kind = kind_str,
                        name = name_str,
                        file_count = files.len(),
                        files = ?files_str,
                        "Multiple files define the same cluster-scoped resource: kind={}, name={}. Files: {:?}",
                        kind_str, name_str, files_str
                    );
                }
            }
        }
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
                                // 2. File moved out (Delete) - in cache or resource_to_files, file doesn't exist or moved outside
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
                                        // File not in cache, check if it exists and is in watched directory
                                        if path.exists() && is_in_watched_dir {
                                            // File exists in watched directory, treat as new
                                            tracing::debug!(
                                                component = "file_system_loader",
                                                event = "rename_any_single_path_new",
                                                path = ?path,
                                                "RenameMode::Any with single path not in cache, treating as new file"
                                            );
                                            self.process_new_file(&path).await?;
                                        } else {
                                            // File doesn't exist or moved outside, check resource_to_files mapping
                                            let normalized_path = self.normalize_path(&path).await;
                                            let resource_to_files = self.resource_to_files.lock().await;
                                            
                                            // Check if this path was tracked in resource_to_files
                                            let was_tracked = resource_to_files.values()
                                                .any(|files| files.contains(&normalized_path));
                                            drop(resource_to_files);
                                            
                                            if was_tracked {
                                                // File was tracked but not in cache, treat as removal
                                                tracing::debug!(
                                                    component = "file_system_loader",
                                                    event = "rename_any_single_path_removed_from_mapping",
                                                    path = ?path,
                                                    exists = path.exists(),
                                                    in_watched = is_in_watched_dir,
                                                    "RenameMode::Any with single path not in cache but found in resource_to_files, treating as removal"
                                                );
                                                self.process_removed_file(&path).await?;
                                            } else {
                                                // File not tracked at all, skip
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
