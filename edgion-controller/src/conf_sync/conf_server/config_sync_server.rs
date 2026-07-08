//! Simplified ConfigSyncServer implementation
//!
//! This server only handles gRPC list/watch operations.
//! It holds a HashMap<kind, Arc<dyn WatchObj>> registered by ResourceProcessors.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::client_registry::ClientRegistry;
use super::traits::WatchObj;

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
    /// Server instance ID; regenerated on each relink.
    server_id: RwLock<String>,

    /// Watch objects by kind (registered by processors)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>,

    /// Registry for tracking connected gateway instances (for Cluster scope rate limiting)
    client_registry: Arc<ClientRegistry>,
}

static LAST_SERVER_ID_MS: AtomicU64 = AtomicU64::new(0);

impl ConfigSyncServer {
    pub fn new() -> Self {
        let server_id = Self::generate_server_id();

        tracing::info!(
            component = "config_sync_server",
            server_id = %server_id,
            "ConfigSyncServer initialized"
        );

        Self {
            server_id: RwLock::new(server_id),
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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut last = LAST_SERVER_ID_MS.load(Ordering::SeqCst);
        loop {
            let next = if now_ms > last { now_ms } else { last + 1 };
            match LAST_SERVER_ID_MS.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return next.to_string(),
                Err(current) => last = current,
            }
        }
    }

    pub fn server_id(&self) -> String {
        self.server_id.read().clone()
    }

    // ==================== WatchObj Registration ====================

    /// Register a WatchObj (called by Processor initialization).
    /// Test-only: production registration uses the batch `register_all`.
    #[cfg(test)]
    pub fn register_watch_obj(&self, kind: &str, obj: Arc<dyn WatchObj>) {
        let mut map = self.watch_objects.write();
        tracing::info!(
            component = "config_sync_server",
            kind = kind,
            "Registering watch object"
        );
        map.insert(kind.to_string(), obj);
    }

    /// Batch register watch objects
    pub fn register_all(&self, objs: HashMap<String, Arc<dyn WatchObj>>) {
        let mut map = self.watch_objects.write();
        for (kind, obj) in objs {
            tracing::info!(
                component = "config_sync_server",
                kind = %kind,
                "Registering watch object (batch)"
            );
            map.insert(kind, obj);
        }
    }

    pub fn all_kinds(&self) -> Vec<String> {
        self.watch_objects.read().keys().cloned().collect()
    }

    // ==================== List/Watch Operations ====================

    /// List resources by kind name
    pub fn list(&self, kind: &str) -> Result<ListDataSimple, String> {
        // 64 MB hard limit — prevents OOM when serializing very large resource sets
        // (e.g. thousands of EndpointSlices). Pagination is the long-term solution.
        const MAX_LIST_BYTES: usize = 64 * 1024 * 1024;

        let map = self.watch_objects.read();
        let obj = map.get(kind).ok_or_else(|| format!("Unknown kind: {}", kind))?;

        let (data, sync_version) = obj.list_json()?;
        if data.len() > MAX_LIST_BYTES {
            tracing::warn!(
                component = "config_sync_server",
                kind = kind,
                size_bytes = data.len(),
                limit_bytes = MAX_LIST_BYTES,
                "List response exceeds size limit, rejecting to prevent OOM"
            );
            return Err(format!(
                "List response for kind '{}' is too large ({} bytes, limit {} bytes)",
                kind,
                data.len(),
                MAX_LIST_BYTES
            ));
        }

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
        let obj = {
            let map = self.watch_objects.read();
            map.get(kind)
                .cloned()
                .ok_or_else(|| format!("Unknown kind: {}", kind))?
        };

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
                            None => {
                                tracing::warn!(
                                    component = "config_sync_server",
                                    server_id = %server_id,
                                    "watch upstream channel closed unexpectedly — WatchObj sender dropped"
                                );
                                break;
                            }
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

    pub fn not_ready_kinds(&self) -> Vec<String> {
        let map = self.watch_objects.read();
        map.iter()
            .filter(|(_, obj)| !obj.is_ready())
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn clear_all(&self) {
        let map = self.watch_objects.read();
        for obj in map.values() {
            obj.clear();
        }
    }
}

impl Default for ConfigSyncServer {
    fn default() -> Self {
        Self::new()
    }
}
