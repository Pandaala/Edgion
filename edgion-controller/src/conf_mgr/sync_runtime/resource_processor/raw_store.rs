//! Raw resource store: direct per-key lookup without handler filter
//!
//! Provides `RawStore<K>` trait and two implementations:
//! - `KubeRawStore<K>`: wraps kube's reflector `Store<K>` (K8s mode)
//! - `LocalRawStore<K>`: self-maintained `HashMap` (FileSystem and Etcd modes)
//!
//! The `ProcessorObj::get_raw_any` method exposes this to callers
//! in an object-safe way (returning `Option<Arc<dyn Any + Send + Sync>>`).
//!
//! Crucially, this bypasses the handler `filter()` step so cross-resource
//! lookups (e.g. Route → Gateway) can find objects that were excluded from
//! `ServerCache` by `gatewayClassName` or namespace filters.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use kube::runtime::reflector::Store;
use kube::Resource;

/// Raw data-source interface: look up a single resource by key, bypassing handler filter.
///
/// Key format follows `make_resource_key`: `"namespace/name"` for namespaced
/// resources, `"name"` for cluster-scoped.
pub trait RawStore<K>: Send + Sync {
    fn get(&self, key: &str) -> Option<Arc<K>>;

    /// Return every resource currently held by this store. Order is
    /// unspecified. Called by the resource-abnormal metrics background
    /// task once per tick.
    fn snapshot(&self) -> Vec<Arc<K>>;
}

// ============================================================================
// K8s backend: wraps reflector::Store<K>
// ============================================================================

/// K8s raw store — readers over one source or nonoverlapping namespace stores.
///
/// Native Store handles share the committed map. The union owns no object copies
/// and never reads an in-progress LIST buffer.
pub struct KubeRawStore<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    stores: KubeStores<K>,
}

enum KubeStores<K: Resource + Clone + 'static>
where
    K::DynamicType: Eq + Hash,
{
    Single(Store<K>),
    Namespaces(HashMap<String, Store<K>>),
}

impl<K> KubeRawStore<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    pub fn new(store: Store<K>) -> Self {
        Self {
            stores: KubeStores::Single(store),
        }
    }

    /// Readers for a fixed, nonoverlapping set of namespace sources.
    pub fn namespaced(stores: HashMap<String, Store<K>>) -> Self {
        Self {
            stores: KubeStores::Namespaces(stores),
        }
    }
}

impl<K> RawStore<K> for KubeRawStore<K>
where
    K: Resource + Clone + Send + Sync + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Send + Sync,
{
    fn get(&self, key: &str) -> Option<Arc<K>> {
        let obj_ref = parse_key_to_obj_ref::<K>(key);
        match &self.stores {
            KubeStores::Single(store) => store.get(&obj_ref),
            KubeStores::Namespaces(stores) => stores.get(obj_ref.namespace.as_deref()?)?.get(&obj_ref),
        }
    }

    fn snapshot(&self) -> Vec<Arc<K>> {
        match &self.stores {
            KubeStores::Single(store) => store.state(),
            KubeStores::Namespaces(stores) => stores.values().flat_map(Store::state).collect(),
        }
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

/// Local in-memory raw store — self-maintained `RwLock<HashMap<String, Arc<K>>>`.
///
/// Used by both FileSystem and Etcd backends. The resource controller calls
/// `upsert` / `remove` whenever it successfully parses an `InitApply`, `Apply`, or `Delete` event.
pub struct LocalRawStore<K> {
    data: RwLock<HashMap<String, Arc<K>>>,
}

impl<K> Default for LocalRawStore<K> {
    fn default() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl<K> LocalRawStore<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&self, key: String, obj: Arc<K>) {
        self.data.write().insert(key, obj);
    }

    pub fn remove(&self, key: &str) {
        self.data.write().remove(key);
    }
}

impl<K: Send + Sync + 'static> RawStore<K> for LocalRawStore<K> {
    fn get(&self, key: &str) -> Option<Arc<K>> {
        self.data.read().get(key).cloned()
    }

    fn snapshot(&self) -> Vec<Arc<K>> {
        self.data.read().values().cloned().collect()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/raw_store_test/snapshot_tests.rs"]
mod snapshot_tests;
