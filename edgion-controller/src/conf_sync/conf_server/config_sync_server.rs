//! Simplified ConfigSyncServer implementation
//!
//! This server only handles gRPC list/watch operations.
//! It holds a HashMap<kind, Arc<dyn WatchObj>> registered by ResourceProcessors.

use bytes::Bytes;
use edgion_common::conf_sync::payload::{self, EncodedPayload, PayloadError};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
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
    pub data: Bytes,
    pub sync_version: u64,
    pub server_id: String,
}

struct CachedList {
    sync_version: u64,
    data: Bytes,
    /// gRPC encoding is computed once for this exact serialized snapshot.
    wire: Arc<tokio::sync::OnceCell<Result<EncodedPayload, PayloadError>>>,
}

/// Per-kind versioned list cache with single-flight fill.
#[derive(Default)]
struct KindListCache {
    /// Single-flight: at most one serialization in flight per kind. A tokio
    /// Mutex so waiters yield their worker thread. No poisoning: a panicking
    /// filler releases the guard on unwind and the next waiter retries.
    fill_lock: tokio::sync::Mutex<()>,
    cached: RwLock<Option<CachedList>>,
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

    /// Per-kind versioned cache of serialized list responses. Used ONLY by
    /// `list_sync_cached` (conf_sync gRPC + federation transports, which ship
    /// real secrets). The admin path stays on the uncached `list()` so its
    /// thread-local redaction can never poison or read this cache.
    list_cache: RwLock<HashMap<String, Arc<KindListCache>>>,
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
            list_cache: RwLock::new(HashMap::new()),
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
        let map = self.watch_objects.read();
        let obj = map.get(kind).ok_or_else(|| format!("Unknown kind: {}", kind))?;

        let (data, sync_version) = obj.list_json()?;
        Ok(ListDataSimple {
            data: Bytes::from(data),
            sync_version,
            server_id: self.server_id(),
        })
    }

    /// Get or lazily create the cache slot for a kind.
    fn kind_list_cache(&self, kind: &str) -> Arc<KindListCache> {
        if let Some(kc) = self.list_cache.read().get(kind) {
            return kc.clone();
        }
        self.list_cache.write().entry(kind.to_string()).or_default().clone()
    }

    /// Version-equality cache lookup. Raw bytes are shared with federation.
    fn cache_lookup(kc: &KindListCache, current_version: u64) -> Option<Bytes> {
        let cached = kc.cached.read();
        let entry = cached.as_ref()?;
        if entry.sync_version != current_version {
            return None;
        }
        Some(entry.data.clone())
    }

    fn hit_to_response(&self, data: Bytes, sync_version: u64) -> ListDataSimple {
        ListDataSimple {
            data,
            sync_version,
            server_id: self.server_id(),
        }
    }

    /// List resources by kind through the per-(kind, sync_version)
    /// serialization cache with single-flight fill.
    ///
    /// Transport-only: callers are the conf_sync gRPC list handler and the
    /// federation client — both ship REAL secrets. The admin path MUST keep
    /// using the uncached `list()` (see the spec's redaction-isolation
    /// section).
    pub async fn list_sync_cached(&self, kind: &str) -> Result<ListDataSimple, String> {
        if edgion_resources::redact::redact_active() {
            // Defense in depth: a redacting caller must never read a warm
            // cache (real bytes) nor fill it (redacted bytes). Route it to
            // the uncached path entirely.
            return self.list(kind);
        }

        // Clone the Arcs out and drop the read guards before any await:
        // parking_lot guards must never live across an await point
        // (workspace lints await_holding_lock).
        let obj = {
            let map = self.watch_objects.read();
            map.get(kind)
                .cloned()
                .ok_or_else(|| format!("Unknown kind: {}", kind))?
        };
        let kc = self.kind_list_cache(kind);

        // Fast path: version-equal hit, no fill lock.
        let current_version = obj.current_version();
        if let Some(hit) = Self::cache_lookup(&kc, current_version) {
            return Ok(self.hit_to_response(hit, current_version));
        }

        // Single-flight: at most one serialization per version.
        let _fill_guard = kc.fill_lock.lock().await;

        // Double-check against a FRESH version read — the version may have
        // advanced (and a peer may have filled) while we waited for the lock.
        let current_version = obj.current_version();
        if let Some(hit) = Self::cache_lookup(&kc, current_version) {
            return Ok(self.hit_to_response(hit, current_version));
        }

        // The one serialization for this version. The (data, sync_version)
        // pair is produced atomically under the store read lock, so the
        // cache label always matches the bytes even if the store advanced
        // after the current_version read above.
        let started = Instant::now();
        let (data, sync_version) = obj.list_json()?;

        let bytes = Bytes::from(data);
        *kc.cached.write() = Some(CachedList {
            sync_version,
            data: bytes.clone(),
            wire: Arc::new(tokio::sync::OnceCell::new()),
        });
        tracing::debug!(
            component = "config_sync_server",
            kind = kind,
            sync_version = sync_version,
            size_bytes = bytes.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "List serialization cache filled"
        );

        Ok(ListDataSimple {
            data: bytes,
            sync_version,
            server_id: self.server_id(),
        })
    }

    /// Encode the gRPC snapshot without changing the raw federation/admin API.
    pub async fn list_grpc_cached(&self, kind: &str) -> Result<(EncodedPayload, u64, String), tonic::Status> {
        // Do not let a thread-local redacting context read or populate wire caches.
        let redacting = edgion_resources::redact::redact_active();
        let raw = self
            .list_sync_cached(kind)
            .await
            .map_err(|_| tonic::Status::internal("Failed to serialize configuration snapshot"))?;
        let wire = if redacting {
            Arc::new(tokio::sync::OnceCell::new())
        } else {
            let kc = self.kind_list_cache(kind);
            let cached = kc.cached.read();
            cached
                .as_ref()
                .filter(|entry| entry.sync_version == raw.sync_version && entry.data.as_ptr() == raw.data.as_ptr())
                .map(|entry| entry.wire.clone())
                // A concurrent new snapshot may have replaced the entry already.
                .unwrap_or_else(|| Arc::new(tokio::sync::OnceCell::new()))
        };
        let encoded = wire
            .get_or_init(|| async {
                let result = payload::encode(raw.data).await;
                if let Err(error) = &result {
                    tracing::warn!(component = "config_sync_server", kind, sync_version = raw.sync_version,
                    error = %error, "Configuration snapshot cannot be encoded for transport");
                }
                result
            })
            .await
            .clone()
            .map_err(|error| error.into_status())?;
        Ok((encoded, raw.sync_version, raw.server_id))
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
        let (tx, rx) = mpsc::channel(1);

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
        // Defense in depth: drop all cached list bytes together with the
        // stores. Entries are recreated lazily on the next list.
        self.list_cache.write().clear();
    }
}

impl Default for ConfigSyncServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_server/config_sync_server_test.rs"]
mod tests;
