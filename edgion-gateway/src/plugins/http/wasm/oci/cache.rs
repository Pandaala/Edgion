//! Bounded, content-addressed OCI artifact retention over the canonical LRU.
//!
//! Allocation accounting belongs to the shared payload, not the LRU entry:
//! eviction cannot free its budget while an acquisition still uses the bytes.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use edgion_resources::resources::edgion_plugins::MAX_LRU_CACHE_CAPACITY;
use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;

use edgion_algo::cache::ShardedLruCache;

/// Bounds the canonical LRU's eagerly allocated metadata, independently of bytes.
pub const MAX_CACHE_ENTRIES: usize = MAX_LRU_CACHE_CAPACITY;
const CLAIM_STRIPES: usize = 64;
const MIN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct OciMemoryCacheConfig {
    pub max_bytes: u64,
    pub max_entries: usize,
    pub ttl: Duration,
    pub purge_interval: Duration,
}

static CACHE: OnceLock<Arc<OciMemoryCache>> = OnceLock::new();

/// Install exactly once before plugin registration. No runtime reconfiguration:
/// replacing the cache would split accounting while old payloads remain live.
pub fn configure_oci_memory_cache(cfg: OciMemoryCacheConfig) -> Result<(), String> {
    let cache = OciMemoryCache::new(cfg)?;
    CACHE
        .set(cache)
        .map_err(|_| "Wasm OCI memory cache is already configured".to_string())
}

#[derive(Debug, Default, serde::Serialize)]
pub struct OciMemoryCacheStats {
    pub entries: usize,
    pub keys: Vec<String>,
    pub retained_bytes: u64,
}

pub fn memory_cache_stats() -> OciMemoryCacheStats {
    CACHE.get().map(|cache| cache.stats()).unwrap_or_default()
}

pub(in crate::plugins::http::wasm) fn register_owner() {
    if let Some(cache) = CACHE.get() {
        cache.register_owner();
    }
}

pub(in crate::plugins::http::wasm) fn unregister_owner() {
    if let Some(cache) = CACHE.get() {
        cache.unregister_owner();
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(super) enum CacheKind {
    Manifest,
    Blob,
}

impl CacheKind {
    fn key(self, digest: &str) -> Option<String> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let kind = match self {
            Self::Manifest => "manifest",
            Self::Blob => "blob",
        };
        Some(format!("{kind}:{}", digest.to_ascii_lowercase()))
    }
}

#[derive(Debug)]
struct Charge {
    budget: Arc<AtomicU64>,
    capacity: u64,
}

impl Drop for Charge {
    fn drop(&mut self) {
        // Never take the policy mutex: eviction may drop the last Arc under it.
        self.budget.fetch_sub(self.capacity, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct Payload {
    bytes: Vec<u8>,
    // Declared after bytes so the allocation is released before its charge.
    _charge: Option<Charge>,
}

/// Immutable shared artifact bytes. Cloning this handle never clones the body.
#[derive(Debug, Clone)]
pub struct ArtifactBytes(Arc<Payload>);

impl From<Vec<u8>> for ArtifactBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(Arc::new(Payload { bytes, _charge: None }))
    }
}

impl Deref for ArtifactBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl AsRef<[u8]> for ArtifactBytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

#[derive(Debug, Clone)]
struct Entry {
    bytes: ArtifactBytes,
    last_used: Instant,
}

#[derive(Debug, Default)]
struct Policy {
    owners: usize,
    generation: u64,
}

#[derive(Debug)]
pub(super) struct OciMemoryCache {
    cfg: OciMemoryCacheConfig,
    entries: ShardedLruCache<String, Entry>,
    policy: Mutex<Policy>,
    budget: Arc<AtomicU64>,
    claims: [Arc<Semaphore>; CLAIM_STRIPES],
    maintenance: Mutex<Option<AbortHandle>>,
}

impl Drop for OciMemoryCache {
    fn drop(&mut self) {
        if let Some(task) = self.maintenance.get_mut().take() {
            task.abort();
        }
    }
}

impl OciMemoryCache {
    pub(super) fn new(mut cfg: OciMemoryCacheConfig) -> Result<Arc<Self>, String> {
        if cfg.max_entries > MAX_CACHE_ENTRIES {
            return Err(format!(
                "Wasm OCI cache max entries must not exceed {MAX_CACHE_ENTRIES}"
            ));
        }
        cfg.ttl = cfg.ttl.max(MIN_INTERVAL);
        cfg.purge_interval = cfg.purge_interval.max(MIN_INTERVAL);
        if Instant::now().checked_add(cfg.ttl).is_none() || Instant::now().checked_add(cfg.purge_interval).is_none() {
            return Err("Wasm OCI cache TTL and purge interval must fit monotonic clock deadlines".to_string());
        }
        // Disabled retention allocates only a single inert canonical slot.
        let capacity = if cfg.max_bytes == 0 { 1 } else { cfg.max_entries.max(1) };
        Ok(Arc::new(Self {
            cfg,
            entries: ShardedLruCache::new(capacity, 1),
            policy: Mutex::new(Policy::default()),
            budget: Arc::new(AtomicU64::new(0)),
            claims: std::array::from_fn(|_| Arc::new(Semaphore::new(1))),
            maintenance: Mutex::new(None),
        }))
    }

    pub(super) fn register_owner(self: &Arc<Self>) {
        self.policy.lock().owners += 1;
        self.ensure_maintenance();
    }

    pub(super) fn unregister_owner(&self) {
        let mut policy = self.policy.lock();
        if policy.owners == 0 {
            return;
        }
        policy.owners -= 1;
        if policy.owners == 0 {
            self.entries.clear();
            policy.generation = policy.generation.wrapping_add(1);
        }
    }

    fn ensure_maintenance(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut maintenance = self.maintenance.lock();
        if maintenance.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let cache = Arc::downgrade(self);
        let interval = self.cfg.purge_interval;
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(cache) = cache.upgrade() else { break };
                let _policy = cache.policy.lock();
                cache.sweep(Instant::now());
            }
        });
        *maintenance = Some(task.abort_handle());
    }

    /// Caller holds policy. Reverse iteration restores surviving MRU order
    /// because canonical get touches entries; native TTL cannot physically sweep.
    fn sweep(&self, now: Instant) {
        for key in self.entries.get_keys().into_iter().rev() {
            if self
                .entries
                .get(&key)
                .is_some_and(|entry| now.saturating_duration_since(entry.last_used) >= self.cfg.ttl)
            {
                self.entries.remove(&key);
            }
        }
    }

    fn stats(&self) -> OciMemoryCacheStats {
        let _policy = self.policy.lock();
        self.sweep(Instant::now());
        OciMemoryCacheStats {
            entries: self.entries.len(),
            keys: self.entries.get_keys(),
            retained_bytes: self.budget.load(Ordering::Acquire),
        }
    }

    pub(super) fn access(self: &Arc<Self>) -> CacheAccess {
        self.ensure_maintenance();
        CacheAccess {
            cache: Some(self.clone()),
            generation: self.policy.lock().generation,
        }
    }
}

/// An acquisition captures its admission generation before network work.
/// Clearing the last owner invalidates old acquisitions even after re-registration.
#[derive(Clone, Default)]
pub(super) struct CacheAccess {
    cache: Option<Arc<OciMemoryCache>>,
    generation: u64,
}

impl CacheAccess {
    pub(super) fn active() -> Self {
        CACHE.get().map(|cache| cache.access()).unwrap_or_default()
    }

    pub(super) fn load(&self, kind: CacheKind, digest: &str) -> Option<ArtifactBytes> {
        let cache = self.cache.as_ref()?;
        let key = kind.key(digest)?;
        let policy = cache.policy.lock();
        if policy.owners == 0 || policy.generation != self.generation {
            return None;
        }
        let mut entry = cache.entries.get(&key)?;
        let now = Instant::now();
        if now.saturating_duration_since(entry.last_used) >= cache.cfg.ttl {
            cache.entries.remove(&key);
            return None;
        }
        entry.last_used = now;
        let bytes = entry.bytes.clone();
        cache.entries.insert(key, entry, None);
        Some(bytes)
    }

    /// Only call after verifying the immutable bytes against digest. Bypass is
    /// best-effort: valid uncached bytes remain usable by the bounded pipeline.
    pub(super) fn store_verified(&self, kind: CacheKind, digest: &str, bytes: Vec<u8>) -> ArtifactBytes {
        let Some(cache) = self.cache.as_ref() else {
            return bytes.into();
        };
        let Some(key) = kind.key(digest) else {
            return bytes.into();
        };
        let policy = cache.policy.lock();
        let incoming = bytes.capacity() as u64;
        if policy.owners == 0
            || policy.generation != self.generation
            || cache.cfg.max_entries == 0
            || cache.cfg.max_bytes == 0
            || incoming > cache.cfg.max_bytes
        {
            return bytes.into();
        }
        let now = Instant::now();
        cache.sweep(now);
        // Identical content already retained: reuse its allocation, drop ours.
        if let Some(mut entry) = cache.entries.get(&key) {
            entry.last_used = now;
            let bytes = entry.bytes.clone();
            cache.entries.insert(key, entry, None);
            return bytes;
        }
        for victim in cache.entries.get_keys().into_iter().rev() {
            if cache.budget.load(Ordering::Acquire) <= cache.cfg.max_bytes - incoming
                && cache.entries.len() < cache.cfg.max_entries
            {
                break;
            }
            cache.entries.remove(&victim);
        }
        if cache.budget.load(Ordering::Acquire) > cache.cfg.max_bytes - incoming {
            return bytes.into();
        }
        cache.budget.fetch_add(incoming, Ordering::AcqRel);
        let bytes = ArtifactBytes(Arc::new(Payload {
            bytes,
            _charge: Some(Charge {
                budget: cache.budget.clone(),
                capacity: incoming,
            }),
        }));
        cache.entries.insert(
            key,
            Entry {
                bytes: bytes.clone(),
                last_used: now,
            },
            None,
        );
        bytes
    }

    /// Bounded load-concurrency claims, not a shared-state lock held over I/O.
    /// Callers recheck after acquiring and hold at most one claim at a time.
    pub(super) async fn claim(&self, kind: CacheKind, digest: &str) -> Option<OwnedSemaphorePermit> {
        let cache = self.cache.as_ref()?;
        let key = kind.key(digest)?;
        let mut hash = DefaultHasher::new();
        key.hash(&mut hash);
        cache.claims[hash.finish() as usize % CLAIM_STRIPES]
            .clone()
            .acquire_owned()
            .await
            .ok()
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/wasm/oci/cache_test.rs"]
mod tests;
