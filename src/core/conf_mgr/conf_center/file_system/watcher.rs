//! File system watcher using notify crate
//!
//! Monitors configuration directory for file changes and notifies ConfigServer.
//! Uses debouncing to handle rapid file change events.

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer};
use crate::types::prelude_resources::*;
use crate::types::ResourceKind;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
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
    pub async fn start(mut self) -> Result<()> {
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
                        // Filter only relevant events
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                                let _ = tx_clone.blocking_send(event);
                            }
                            _ => {}
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

        // Process events with debouncing
        let mut pending_events: HashMap<PathBuf, EventKind> = HashMap::new();
        let debounce_duration = Duration::from_millis(500);
        let mut debounce_timer = tokio::time::interval(debounce_duration);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Accumulate events for debouncing
                    for path in event.paths {
                        // Only process YAML files
                        if self.is_yaml_file(&path) {
                            pending_events.insert(path, event.kind.clone());
                        }
                    }
                }
                _ = debounce_timer.tick() => {
                    // Process all pending events
                    if !pending_events.is_empty() {
                        let events: Vec<_> = pending_events.drain().collect();
                        for (path, kind) in events {
                            if let Err(e) = self.handle_file_event(&path, kind).await {
                                tracing::error!(
                                    component = "file_watcher",
                                    path = %path.display(),
                                    error = %e,
                                    "Failed to handle file event"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check if a path is a YAML file
    fn is_yaml_file(&self, path: &Path) -> bool {
        path.extension()
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false)
    }

    /// Handle a file event
    async fn handle_file_event(&mut self, path: &Path, kind: EventKind) -> Result<()> {
        tracing::debug!(
            component = "file_watcher",
            path = %path.display(),
            event = ?kind,
            "Handling file event"
        );

        match kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                self.handle_file_change(path).await?;
            }
            EventKind::Remove(_) => {
                self.handle_file_delete(path).await?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Handle file creation or modification
    async fn handle_file_change(&mut self, path: &Path) -> Result<()> {
        // Read file content
        let content = tokio::fs::read_to_string(path)
            .await
            .context("Failed to read file")?;

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
        self.apply_resource_change(&content, change).await?;

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
    async fn apply_resource_change(&self, content: &str, change: ResourceChange) -> Result<()> {
        let kind = ResourceKind::from_content(content);

        match kind {
            Some(ResourceKind::GatewayClass) => {
                if let Ok(resource) = serde_yaml::from_str::<GatewayClass>(content) {
                    self.config_server
                        .gateway_classes
                        .apply_change(change, resource);
                }
            }
            Some(ResourceKind::Gateway) => {
                if let Ok(resource) = serde_yaml::from_str::<Gateway>(content) {
                    self.config_server.apply_gateway_change(change, resource);
                }
            }
            Some(ResourceKind::EdgionGatewayConfig) => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionGatewayConfig>(content) {
                    self.config_server
                        .edgion_gateway_configs
                        .apply_change(change, resource);
                }
            }
            Some(ResourceKind::HTTPRoute) => {
                if let Ok(resource) = serde_yaml::from_str::<HTTPRoute>(content) {
                    self.config_server.apply_http_route_change(change, resource);
                }
            }
            Some(ResourceKind::GRPCRoute) => {
                if let Ok(resource) = serde_yaml::from_str::<GRPCRoute>(content) {
                    self.config_server.apply_grpc_route_change(change, resource);
                }
            }
            Some(ResourceKind::TCPRoute) => {
                if let Ok(resource) = serde_yaml::from_str::<TCPRoute>(content) {
                    self.config_server.apply_tcp_route_change(change, resource);
                }
            }
            Some(ResourceKind::UDPRoute) => {
                if let Ok(resource) = serde_yaml::from_str::<UDPRoute>(content) {
                    self.config_server.apply_udp_route_change(change, resource);
                }
            }
            Some(ResourceKind::TLSRoute) => {
                if let Ok(resource) = serde_yaml::from_str::<TLSRoute>(content) {
                    self.config_server.apply_tls_route_change(change, resource);
                }
            }
            Some(ResourceKind::Service) => {
                if let Ok(resource) = serde_yaml::from_str::<Service>(content) {
                    self.config_server.apply_service_change(change, resource);
                }
            }
            Some(ResourceKind::Endpoint) => {
                if let Ok(resource) = serde_yaml::from_str::<Endpoints>(content) {
                    self.config_server.apply_endpoint_change(change, resource);
                }
            }
            Some(ResourceKind::EndpointSlice) => {
                if let Ok(resource) = serde_yaml::from_str::<EndpointSlice>(content) {
                    self.config_server
                        .apply_endpoint_slice_change(change, resource);
                }
            }
            Some(ResourceKind::ReferenceGrant) => {
                if let Ok(resource) = serde_yaml::from_str::<ReferenceGrant>(content) {
                    self.config_server
                        .reference_grants
                        .apply_change(change, resource);
                }
            }
            Some(ResourceKind::BackendTLSPolicy) => {
                if let Ok(resource) = serde_yaml::from_str::<BackendTLSPolicy>(content) {
                    self.config_server
                        .backend_tls_policies
                        .apply_change(change, resource);
                }
            }
            Some(ResourceKind::EdgionTls) => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionTls>(content) {
                    self.config_server.apply_edgion_tls_change(change, resource);
                }
            }
            Some(ResourceKind::Secret) => {
                if let Ok(resource) = serde_yaml::from_str::<Secret>(content) {
                    self.config_server.apply_secret_change(change, resource);
                }
            }
            Some(ResourceKind::EdgionPlugins) => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionPlugins>(content) {
                    self.config_server
                        .apply_edgion_plugins_change(change, resource);
                }
            }
            Some(ResourceKind::EdgionStreamPlugins) => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionStreamPlugins>(content) {
                    self.config_server
                        .edgion_stream_plugins
                        .apply_change(change, resource);
                }
            }
            Some(ResourceKind::PluginMetaData) => {
                if let Ok(resource) = serde_yaml::from_str::<PluginMetaData>(content) {
                    self.config_server
                        .apply_plugin_metadata_change(change, resource);
                }
            }
            Some(ResourceKind::LinkSys) => {
                if let Ok(resource) = serde_yaml::from_str::<LinkSys>(content) {
                    self.config_server.apply_link_sys_change(change, resource);
                }
            }
            _ => {
                tracing::debug!(
                    component = "file_watcher",
                    kind = ?kind,
                    "Skipping unknown resource type"
                );
            }
        }

        Ok(())
    }
}
