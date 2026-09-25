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
async fn large_snapshot_is_compressed_once_and_shared_without_changing_raw_cache() {
    let big = "x".repeat(64 * 1024 * 1024 + 1);
    let obj = Arc::new(CountingWatchObj::new(1, &big));
    drop(big);
    let server = server_with(obj.clone());

    let (first, version, _) = server.list_grpc_cached("CountingKind").await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(first.encoding, 1);
    assert_eq!(first.uncompressed_size, 64 * 1024 * 1024 + 1);
    assert_eq!(obj.calls(), 1);
    let (second, _, _) = server.list_grpc_cached("CountingKind").await.unwrap();
    assert_eq!(first.data.as_ptr(), second.data.as_ptr());
    assert_eq!(obj.calls(), 1);
    let raw = server.list_sync_cached("CountingKind").await.unwrap();
    assert_eq!(raw.data.len() as u64, first.uncompressed_size);
    assert_eq!(
        payload::decode(first.data, first.encoding, first.uncompressed_size)
            .await
            .unwrap(),
        raw.data
    );

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

/// The core claim of the bytes migration: repeated cache hits must hand
/// out the SAME underlying buffer (refcount clone), not a fresh copy.
#[tokio::test]
async fn cache_hits_share_one_buffer() {
    let obj = Arc::new(CountingWatchObj::new(1, "[\"a\"]"));
    let server = server_with(obj.clone());

    let first = server.list_sync_cached("CountingKind").await.unwrap();
    let second = server.list_sync_cached("CountingKind").await.unwrap();
    assert_eq!(
        first.data.as_ptr(),
        second.data.as_ptr(),
        "cache hits must share one buffer (refcount clone, no copy)"
    );
}

impl ConfigSyncServer {
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
}
