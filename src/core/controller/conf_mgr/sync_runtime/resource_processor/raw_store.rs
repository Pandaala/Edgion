//! Raw resource store: direct per-key lookup without handler filter
//!
//! Provides `RawStore<K>` trait and two implementations:
//! - `KubeRawStore<K>`: wraps kube's reflector `Store<K>` (K8s mode)
//! - `FsRawStore<K>`: self-maintained `HashMap` (FileSystem mode)
//!
//! The `ProcessorObj::get_raw_any` method exposes this to callers
//! in an object-safe way (returning `Option<Arc<dyn Any + Send + Sync>>`).
//!
//! Crucially, this bypasses the handler `filter()` step so cross-resource
//! lookups (e.g. Route → Gateway) can find objects that were excluded from
//! `ServerCache` by `gatewayClassName` or namespace filters.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, RwLock};

use kube::runtime::reflector::Store;
use kube::Resource;

/// Raw data-source interface: look up a single resource by key, bypassing handler filter.
///
/// Key format follows `make_resource_key`: `"namespace/name"` for namespaced
/// resources, `"name"` for cluster-scoped.
pub trait RawStore<K>: Send + Sync {
    fn get(&self, key: &str) -> Option<Arc<K>>;
}

// ============================================================================
// K8s backend: wraps reflector::Store<K>
// ============================================================================

/// K8s raw store — thin wrapper around kube's reflector `Store<K>`.
///
/// `Store` internally uses `Arc<DashMap<ObjectRef, Arc<K>>>` so cloning the
/// `Store` is cheap (just clones the outer Arc).
pub struct KubeRawStore<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    store: Store<K>,
}

impl<K> KubeRawStore<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    pub fn new(store: Store<K>) -> Self {
        Self { store }
    }
}

impl<K> RawStore<K> for KubeRawStore<K>
where
    K: Resource + Clone + Send + Sync + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Send + Sync,
{
    fn get(&self, key: &str) -> Option<Arc<K>> {
        let obj_ref = parse_key_to_obj_ref::<K>(key);
        self.store.get(&obj_ref)
    }
}

/// Parse a `"namespace/name"` or `"name"` key into an `ObjectRef<K>`.
fn parse_key_to_obj_ref<K>(key: &str) -> kube::runtime::reflector::ObjectRef<K>
where
    K: Resource,
    K::DynamicType: Default,
{
    if let Some((ns, name)) = key.split_once('/') {
        kube::runtime::reflector::ObjectRef::<K>::new(name).within(ns)
    } else {
        kube::runtime::reflector::ObjectRef::<K>::new(key)
    }
}

// ============================================================================
// FileSystem backend: self-maintained HashMap
// ============================================================================

/// FileSystem raw store — self-maintained `RwLock<HashMap<String, Arc<K>>>`.
///
/// `FileSystemResourceController` calls `upsert` / `remove` whenever it
/// successfully parses an `InitApply`, `Apply`, or `Delete` event.
pub struct FsRawStore<K> {
    data: RwLock<HashMap<String, Arc<K>>>,
}

impl<K> Default for FsRawStore<K> {
    fn default() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl<K> FsRawStore<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&self, key: String, obj: Arc<K>) {
        self.data.write().unwrap().insert(key, obj);
    }

    pub fn remove(&self, key: &str) {
        self.data.write().unwrap().remove(key);
    }
}

impl<K: Send + Sync + 'static> RawStore<K> for FsRawStore<K> {
    fn get(&self, key: &str) -> Option<Arc<K>> {
        self.data.read().unwrap().get(key).cloned()
    }
}
