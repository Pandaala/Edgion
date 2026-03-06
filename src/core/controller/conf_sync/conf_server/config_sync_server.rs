//! Simplified ConfigSyncServer implementation
//!
//! This server only handles gRPC list/watch operations.
//! It holds a HashMap<kind, Arc<dyn WatchObj>> registered by ResourceProcessors.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::mpsc;

use super::client_registry::ClientRegistry;
use super::traits::WatchObj;
use crate::core::controller::conf_mgr::conf_center::EndpointMode;

/// Simplified event data for watch responses (includes server_id)
#[derive(Debug, Clone)]
pub struct EventDataSimple {
    pub data: String,
    pub sync_version: u64,
    pub err: Option<String>,
    pub server_id: String,
}

/// Simplified list data response
#[derive(Debug, Clone)]
pub struct ListDataSimple {
    pub data: String,
    pub sync_version: u64,
    pub server_id: String,
}

/// Simplified ConfigSyncServer
///
/// This server only holds watch objects registered by ResourceProcessors.
/// It does NOT create or manage ServerCache<T> instances - that's done by processors.
pub struct ConfigSyncServer {
    /// Server instance ID, generated at startup
    server_id: RwLock<String>,

    /// Endpoint discovery mode
    endpoint_mode: RwLock<Option<EndpointMode>>,

    /// Watch objects by kind (registered by processors)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>,

    /// Registry for tracking connected gateway instances (for Cluster scope rate limiting)
    client_registry: Arc<ClientRegistry>,
}

impl ConfigSyncServer {
    /// Create a new ConfigSyncServer
    pub fn new() -> Self {
        let server_id = Self::generate_server_id();

        tracing::info!(
            component = "config_sync_server",
            server_id = %server_id,
            "ConfigSyncServer initialized"
        );

        Self {
            server_id: RwLock::new(server_id),
            endpoint_mode: RwLock::new(None),
            watch_objects: RwLock::new(HashMap::new()),
            client_registry: Arc::new(ClientRegistry::new()),
        }
    }

    /// Get the client registry (for WatchServerMeta)
    pub fn client_registry(&self) -> Arc<ClientRegistry> {
        self.client_registry.clone()
    }

    /// Generate a new server ID using millisecond timestamp
    fn generate_server_id() -> String {
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        )
    }

    /// Get the current server ID
    pub fn server_id(&self) -> String {
        self.server_id.read().unwrap().clone()
    }

    /// Regenerate server ID (for relink)
    pub fn regenerate_server_id(&self) {
        let old_id = self.server_id();
        let new_id = Self::generate_server_id();
        tracing::info!(
            component = "config_sync_server",
            old_server_id = %old_id,
            new_server_id = %new_id,
            "Regenerating server_id for relink"
        );
        *self.server_id.write().unwrap() = new_id;
    }

    /// Set endpoint discovery mode
    pub fn set_endpoint_mode(&self, mode: EndpointMode) {
        *self.endpoint_mode.write().unwrap() = Some(mode);
    }

    /// Get endpoint discovery mode
    pub fn endpoint_mode(&self) -> Option<EndpointMode> {
        *self.endpoint_mode.read().unwrap()
    }

    // ==================== WatchObj Registration ====================

    /// Register a WatchObj (called by Processor initialization)
    pub fn register_watch_obj(&self, kind: &str, obj: Arc<dyn WatchObj>) {
        let mut map = self.watch_objects.write().unwrap();
        tracing::info!(
            component = "config_sync_server",
            kind = kind,
            "Registering watch object"
        );
        map.insert(kind.to_string(), obj);
    }

    /// Batch register watch objects
    pub fn register_all(&self, objs: HashMap<String, Arc<dyn WatchObj>>) {
        let mut map = self.watch_objects.write().unwrap();
        for (kind, obj) in objs {
            tracing::info!(
                component = "config_sync_server",
                kind = %kind,
                "Registering watch object (batch)"
            );
            map.insert(kind, obj);
        }
    }

    /// Get a watch object by kind
    pub fn get_watch_obj(&self, kind: &str) -> Option<Arc<dyn WatchObj>> {
        self.watch_objects.read().unwrap().get(kind).cloned()
    }

    /// Get all registered kind names
    pub fn all_kinds(&self) -> Vec<String> {
        self.watch_objects.read().unwrap().keys().cloned().collect()
    }

    // ==================== List/Watch Operations ====================

    /// List resources by kind name
    pub fn list(&self, kind: &str) -> Result<ListDataSimple, String> {
        let map = self.watch_objects.read().unwrap();
        let obj = map.get(kind).ok_or_else(|| format!("Unknown kind: {}", kind))?;

        let (data, sync_version) = obj.list_json()?;

        Ok(ListDataSimple {
            data,
            sync_version,
            server_id: self.server_id(),
        })
    }

    /// Watch resources by kind name
    pub fn watch(
        &self,
        kind: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        let map = self.watch_objects.read().unwrap();
        let obj = map.get(kind).ok_or_else(|| format!("Unknown kind: {}", kind))?;

        tracing::info!(
            component = "config_sync_server",
            kind = kind,
            client_id = %client_id,
            client_name = %client_name,
            from_version = from_version,
            "Starting watch"
        );

        let simple_rx = obj.watch_json(client_id, client_name, from_version);
        let server_id = self.server_id();

        // Convert WatchResponseSimple to EventDataSimple (add server_id)
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let mut simple_rx = simple_rx;
            loop {
                tokio::select! {
                    // Forward data from upstream
                    response = simple_rx.recv() => {
                        match response {
                            Some(response) => {
                                let event_data = EventDataSimple {
                                    data: response.data,
                                    sync_version: response.sync_version,
                                    err: response.err,
                                    server_id: server_id.clone(),
                                };

                                if tx.send(event_data).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // Upstream closed
                        }
                    }
                    // Exit when downstream receiver is dropped
                    _ = tx.closed() => {
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    // ==================== State Management ====================

    /// Check if all registered watch objects are ready
    pub fn is_all_ready(&self) -> bool {
        let map = self.watch_objects.read().unwrap();
        map.values().all(|obj| obj.is_ready())
    }

    /// Get list of kinds that are not ready
    pub fn not_ready_kinds(&self) -> Vec<String> {
        let map = self.watch_objects.read().unwrap();
        map.iter()
            .filter(|(_, obj)| !obj.is_ready())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Set a specific watch object to ready state by kind
    pub fn set_cache_ready_by_kind(&self, kind: &str) {
        let map = self.watch_objects.read().unwrap();
        if let Some(obj) = map.get(kind) {
            obj.set_ready();
            tracing::info!(component = "config_sync_server", kind = kind, "Cache marked as ready");
        } else {
            tracing::warn!(
                component = "config_sync_server",
                kind = kind,
                "Unknown kind for set_cache_ready_by_kind"
            );
        }
    }

    /// Set all watch objects to not ready state
    pub fn set_all_not_ready(&self) {
        let map = self.watch_objects.read().unwrap();
        for obj in map.values() {
            obj.set_not_ready();
        }
    }

    /// Clear all watch objects
    pub fn clear_all(&self) {
        let map = self.watch_objects.read().unwrap();
        for obj in map.values() {
            obj.clear();
        }
    }

    /// Reset for relink
    pub fn reset_for_relink(&self) {
        tracing::info!(component = "config_sync_server", "Resetting for relink");
        self.set_all_not_ready();
        self.clear_all();
        self.regenerate_server_id();
        tracing::info!(
            component = "config_sync_server",
            server_id = %self.server_id(),
            "Reset complete"
        );
    }
}

impl Default for ConfigSyncServer {
    fn default() -> Self {
        Self::new()
    }
}
