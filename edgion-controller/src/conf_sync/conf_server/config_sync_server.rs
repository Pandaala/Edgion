//! Simplified ConfigSyncServer implementation
//!
//! This server only handles gRPC list/watch operations.
//! It holds a HashMap<kind, Arc<dyn WatchObj>> registered by ResourceProcessors.

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
    pub data: String,
    pub sync_version: u64,
    pub server_id: String,
}

/// 64 MB hard limit — prevents OOM when serializing very large resource sets
/// (e.g. thousands of EndpointSlices). Pagination is the long-term solution.
const MAX_LIST_BYTES: usize = 64 * 1024 * 1024;

fn oversize_error(kind: &str, size: usize) -> String {
    format!(
        "List response for kind '{}' is too large ({} bytes, limit {} bytes)",
        kind, size, MAX_LIST_BYTES
    )
}

/// Cached serialization result for one kind at one sync_version.
enum CachedPayload {
    /// Serialized snapshot bytes, shared across concurrent hits.
    Bytes(Arc<str>),
    /// Negative cache: serialization at this version exceeded MAX_LIST_BYTES.
    Oversize { size: usize },
}

struct CachedList {
    sync_version: u64,
    payload: CachedPayload,
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
            return Err(oversize_error(kind, data.len()));
        }

        Ok(ListDataSimple {
            data,
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

    /// Version-equality cache lookup. `None` = miss; `Some(Ok)` = bytes hit;
    /// `Some(Err)` = negative (oversize) hit.
    fn cache_lookup(kind: &str, kc: &KindListCache, current_version: u64) -> Option<Result<Arc<str>, String>> {
        let cached = kc.cached.read();
        let entry = cached.as_ref()?;
        if entry.sync_version != current_version {
            return None;
        }
        Some(match &entry.payload {
            CachedPayload::Bytes(bytes) => Ok(bytes.clone()),
            CachedPayload::Oversize { size } => Err(oversize_error(kind, *size)),
        })
    }

    fn hit_to_response(&self, hit: Result<Arc<str>, String>, sync_version: u64) -> Result<ListDataSimple, String> {
        hit.map(|bytes| ListDataSimple {
            data: bytes.to_string(),
            sync_version,
            server_id: self.server_id(),
        })
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
        if let Some(hit) = Self::cache_lookup(kind, &kc, current_version) {
            return self.hit_to_response(hit, current_version);
        }

        // Single-flight: at most one serialization per version.
        let _fill_guard = kc.fill_lock.lock().await;

        // Double-check against a FRESH version read — the version may have
        // advanced (and a peer may have filled) while we waited for the lock.
        let current_version = obj.current_version();
        if let Some(hit) = Self::cache_lookup(kind, &kc, current_version) {
            return self.hit_to_response(hit, current_version);
        }

        // The one serialization for this version. The (data, sync_version)
        // pair is produced atomically under the store read lock, so the
        // cache label always matches the bytes even if the store advanced
        // after the current_version read above.
        let started = Instant::now();
        let (data, sync_version) = obj.list_json()?;

        if data.len() > MAX_LIST_BYTES {
            // warn once per version (single-flight guarantees one fill),
            // instead of today's warn per request.
            tracing::warn!(
                component = "config_sync_server",
                kind = kind,
                sync_version = sync_version,
                size_bytes = data.len(),
                limit_bytes = MAX_LIST_BYTES,
                "List response exceeds size limit, rejecting to prevent OOM (negative-cached for this version)"
            );
            let size = data.len();
            *kc.cached.write() = Some(CachedList {
                sync_version,
                payload: CachedPayload::Oversize { size },
            });
            return Err(oversize_error(kind, size));
        }

        let bytes: Arc<str> = Arc::from(data);
        *kc.cached.write() = Some(CachedList {
            sync_version,
            payload: CachedPayload::Bytes(bytes.clone()),
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
            data: bytes.to_string(),
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
mod tests {
    use super::*;
    use crate::conf_sync::conf_server::traits::{WatchObj, WatchResponseSimple};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Mock WatchObj that counts serializations and lets tests control the
    /// store version and payload independently. `list_version` may diverge
    /// from `version` to simulate the store advancing mid-fill.
    struct CountingWatchObj {
        version: AtomicU64,
        list_version: AtomicU64,
        payload: parking_lot::RwLock<String>,
        serialize_calls: AtomicU64,
    }

    impl CountingWatchObj {
        fn new(version: u64, payload: &str) -> Self {
            Self {
                version: AtomicU64::new(version),
                list_version: AtomicU64::new(version),
                payload: parking_lot::RwLock::new(payload.to_string()),
                serialize_calls: AtomicU64::new(0),
            }
        }

        fn set_version(&self, v: u64) {
            self.version.store(v, Ordering::SeqCst);
            self.list_version.store(v, Ordering::SeqCst);
        }

        fn calls(&self) -> u64 {
            self.serialize_calls.load(Ordering::SeqCst)
        }
    }

    impl WatchObj for CountingWatchObj {
        fn kind_name(&self) -> &'static str {
            "CountingKind"
        }

        fn list_json(&self) -> Result<(String, u64), String> {
            self.serialize_calls.fetch_add(1, Ordering::SeqCst);
            Ok((self.payload.read().clone(), self.list_version.load(Ordering::SeqCst)))
        }

        fn current_version(&self) -> u64 {
            self.version.load(Ordering::SeqCst)
        }

        fn watch_json(
            &self,
            _client_id: String,
            _client_name: String,
            _from_version: u64,
        ) -> mpsc::Receiver<WatchResponseSimple> {
            let (_tx, rx) = mpsc::channel(1);
            rx
        }

        fn is_ready(&self) -> bool {
            true
        }

        fn set_ready(&self) {}

        fn set_not_ready(&self) {}

        fn clear(&self) {}
    }

    fn server_with(obj: Arc<CountingWatchObj>) -> ConfigSyncServer {
        let server = ConfigSyncServer::new();
        server.register_watch_obj("CountingKind", obj);
        server
    }

    #[tokio::test]
    async fn same_version_serializes_once() {
        let obj = Arc::new(CountingWatchObj::new(1, "[\"a\"]"));
        let server = server_with(obj.clone());

        for _ in 0..5 {
            let r = server.list_sync_cached("CountingKind").await.unwrap();
            assert_eq!(r.data, "[\"a\"]");
            assert_eq!(r.sync_version, 1);
        }
        assert_eq!(obj.calls(), 1);
    }

    #[tokio::test]
    async fn version_bump_reserializes_exactly_once() {
        let obj = Arc::new(CountingWatchObj::new(1, "[\"a\"]"));
        let server = server_with(obj.clone());

        server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(obj.calls(), 1);

        *obj.payload.write() = "[\"a\",\"b\"]".to_string();
        obj.set_version(2);

        for _ in 0..5 {
            let r = server.list_sync_cached("CountingKind").await.unwrap();
            assert_eq!(r.data, "[\"a\",\"b\"]");
            assert_eq!(r.sync_version, 2);
        }
        assert_eq!(obj.calls(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_misses_serialize_once() {
        let obj = Arc::new(CountingWatchObj::new(1, "[\"a\"]"));
        let server = Arc::new(server_with(obj.clone()));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let s = server.clone();
            handles.push(tokio::spawn(async move {
                s.list_sync_cached("CountingKind").await.unwrap()
            }));
        }
        for h in handles {
            let r = h.await.unwrap();
            assert_eq!(r.data, "[\"a\"]");
            assert_eq!(r.sync_version, 1);
        }
        assert_eq!(obj.calls(), 1);
    }

    #[tokio::test]
    async fn fill_labels_entry_with_list_json_version() {
        // Simulate the store advancing between the version check and the
        // serialization: current_version says 5 but list_json returns 6.
        let obj = Arc::new(CountingWatchObj::new(5, "[\"new\"]"));
        obj.list_version.store(6, Ordering::SeqCst);
        let server = server_with(obj.clone());

        let r = server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(r.sync_version, 6, "response must carry list_json's version");
        assert_eq!(obj.calls(), 1);

        // Once the store visibly reaches 6, the entry must HIT — proving the
        // cached label was 6, not the stale pre-fill 5.
        obj.version.store(6, Ordering::SeqCst);
        let r = server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(r.sync_version, 6);
        assert_eq!(obj.calls(), 1, "no re-serialization after the store catches up");
    }

    #[tokio::test]
    async fn oversize_is_negative_cached_per_version() {
        let big = "x".repeat(MAX_LIST_BYTES + 1);
        let obj = Arc::new(CountingWatchObj::new(1, &big));
        drop(big);
        let server = server_with(obj.clone());

        let expected_err = format!(
            "List response for kind 'CountingKind' is too large ({} bytes, limit {} bytes)",
            MAX_LIST_BYTES + 1,
            MAX_LIST_BYTES
        );

        let e1 = server.list_sync_cached("CountingKind").await.unwrap_err();
        assert_eq!(e1, expected_err);
        assert_eq!(obj.calls(), 1);

        // Same version: fail-fast from the negative cache, no re-serialization.
        let e2 = server.list_sync_cached("CountingKind").await.unwrap_err();
        assert_eq!(e2, expected_err);
        assert_eq!(obj.calls(), 1);

        // A version bump retries naturally.
        *obj.payload.write() = "[]".to_string();
        obj.set_version(2);
        let r = server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(r.data, "[]");
        assert_eq!(obj.calls(), 2);
    }

    #[tokio::test]
    async fn clear_all_invalidates_cache() {
        let obj = Arc::new(CountingWatchObj::new(7, "[\"a\"]"));
        let server = server_with(obj.clone());

        server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(obj.calls(), 1);

        server.clear_all();

        // Same version again (worst case for staleness): must re-serialize,
        // never serve the pre-clear bytes.
        *obj.payload.write() = "[\"b\"]".to_string();
        let r = server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(r.data, "[\"b\"]");
        assert_eq!(obj.calls(), 2);
    }

    #[tokio::test]
    async fn redacting_caller_bypasses_cache_in_both_directions() {
        // AdminRedactGuard is a thread-local. This test relies on the
        // current-thread runtime (#[tokio::test] default): every poll runs on
        // the guard's thread, so redact_active() is observable. On a
        // multi-thread runtime this test would silently assert nothing.
        let obj = Arc::new(CountingWatchObj::new(1, "[\"real\"]"));
        let server = server_with(obj.clone());

        // Warm the cache without the guard.
        server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(obj.calls(), 1);

        // Under the guard: must NOT serve the warm cache — it re-serializes via
        // the uncached list() path, so the call count rises.
        {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            let r = server.list_sync_cached("CountingKind").await.unwrap();
            assert_eq!(r.data, "[\"real\"]");
            assert_eq!(obj.calls(), 2, "guarded call must bypass the warm cache");
        }

        // ...and must NOT have touched the cache: the next unguarded call at the
        // same version hits without a new serialization.
        server.list_sync_cached("CountingKind").await.unwrap();
        assert_eq!(obj.calls(), 2, "guarded call must not have filled the cache");
    }
}
