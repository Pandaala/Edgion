//! Generic Bidirectional Reference Manager
//!
//! Provides a reusable data structure for tracking many-to-many relationships
//! between "sources" (e.g., Secret, Service, namespace) and "values" (e.g.,
//! resources that depend on them).
//!
//! ## Data Model
//!
//! - Forward index: `source_key → Set<V>` (which values reference this source)
//! - Reverse index: `value_key → Set<source_key>` (which sources a value depends on)
//!
//! ## Concurrency design
//!
//! Both indices are protected by a **single** `RwLock<RefMaps<V>>`.  An earlier
//! design used two separate locks — one per index — which introduced an AB-BA
//! deadlock: `add_ref` acquired `refs` then `dependencies`, while
//! `clear_value_refs` acquired them in the opposite order.  The single-lock
//! design eliminates that hazard: every mutation is one atomic critical section,
//! so the indices are always consistent with each other.
//!
//! ## Usage
//!
//! Concrete managers are type aliases:
//! - `SecretRefManager = BidirectionalRefManager<ResourceRef>`
//! - `CrossNamespaceRefManager = BidirectionalRefManager<ResourceRef>`
//! - `ServiceRefManager = BidirectionalRefManager<ResourceRef>`

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use edgion_resources::ResourceKind;

// ---------------------------------------------------------------------------
// RefValue trait
// ---------------------------------------------------------------------------

/// Trait for values stored in the forward index of a `BidirectionalRefManager`.
///
/// Implementors must provide `ref_key()` which is used as the key in the
/// reverse index.
pub trait RefValue: Clone + Eq + Hash + std::fmt::Debug {
    /// Key used for the reverse index lookup.
    fn ref_key(&self) -> String;
}

// ---------------------------------------------------------------------------
// ResourceRef — unified resource reference type
// ---------------------------------------------------------------------------

/// Identifies a resource that participates in a reference relationship.
///
/// Used as the value type in `BidirectionalRefManager` for Secret, Service,
/// and cross-namespace reference tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl ResourceRef {
    pub fn new(kind: ResourceKind, namespace: Option<String>, name: String) -> Self {
        Self { kind, namespace, name }
    }

    /// Full key including kind: `"Kind/namespace/name"` or `"Kind//name"`.
    pub fn key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{:?}/{}/{}", self.kind, ns, self.name),
            None => format!("{:?}//{}", self.kind, self.name),
        }
    }

    /// Short key without kind: `"namespace/name"` or just `"name"`.
    pub fn resource_key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}/{}", ns, self.name),
            None => self.name.clone(),
        }
    }

    /// Resource kind as a static str (delegates to `ResourceKind::as_str`).
    pub fn kind_str(&self) -> &'static str {
        self.kind.as_str()
    }

    /// Parse a key in format `"Kind/namespace/name"` or `"Kind//name"` back
    /// into a `ResourceRef`.
    pub fn from_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(3, '/').collect();
        if parts.len() < 3 {
            return None;
        }

        let kind = ResourceKind::from_kind_name(parts[0])?;

        let namespace = if parts[1].is_empty() {
            None
        } else {
            Some(parts[1].to_string())
        };

        Some(Self {
            kind,
            namespace,
            name: parts[2].to_string(),
        })
    }
}

impl RefValue for ResourceRef {
    fn ref_key(&self) -> String {
        self.key()
    }
}

// ---------------------------------------------------------------------------
// RefManagerStats
// ---------------------------------------------------------------------------

/// Statistics snapshot for a `BidirectionalRefManager`.
#[derive(Debug, Clone)]
pub struct RefManagerStats {
    /// Number of unique source keys in the forward index
    pub source_count: usize,
    /// Number of unique values in the reverse index
    pub value_count: usize,
    /// Total number of reference relationships
    pub total_references: usize,
}

// ---------------------------------------------------------------------------
// RefMaps — inner state protected by a single lock
// ---------------------------------------------------------------------------

/// Inner state of a [`BidirectionalRefManager`], protected by one `RwLock`.
///
/// Keeping both maps under the same lock ensures every mutation is atomic:
/// readers always see a consistent view and there is no possibility of
/// AB-BA lock-order deadlocks.
struct RefMaps<V: RefValue> {
    /// Forward index: `source_key → Set<V>`
    refs: HashMap<String, HashSet<V>>,
    /// Reverse index: `value_key → Set<source_key>`
    dependencies: HashMap<String, HashSet<String>>,
}

impl<V: RefValue> RefMaps<V> {
    fn new() -> Self {
        Self {
            refs: HashMap::new(),
            dependencies: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BidirectionalRefManager<V>
// ---------------------------------------------------------------------------

/// Thread-safe bidirectional many-to-many index.
///
/// - **Forward**: `source_key → HashSet<V>` — which values reference the source
/// - **Reverse**: `value_key → HashSet<source_key>` — which sources a value depends on
///
/// All mutating operations maintain both indices in sync within a single
/// critical section, eliminating split-state windows and lock-order deadlocks.
pub struct BidirectionalRefManager<V: RefValue> {
    inner: RwLock<RefMaps<V>>,
    component: &'static str,
}

impl<V: RefValue> BidirectionalRefManager<V> {
    /// Create a new manager with a default component name.
    pub fn new() -> Self {
        Self::with_component("ref_manager")
    }

    /// Create a new manager with the given component name (used in tracing).
    pub fn with_component(component: &'static str) -> Self {
        Self {
            inner: RwLock::new(RefMaps::new()),
            component,
        }
    }

    /// Add a reference: `value` depends on `source_key`.
    ///
    /// Idempotent — adding the same pair multiple times is safe.
    pub fn add_ref(&self, source_key: String, value: V) {
        let value_key = value.ref_key();
        {
            let mut maps = self.inner.write();
            maps.refs.entry(source_key.clone()).or_default().insert(value.clone());
            maps.dependencies
                .entry(value_key.clone())
                .or_default()
                .insert(source_key.clone());
        }
        tracing::debug!(
            component = self.component,
            source_key = %source_key,
            value = %value_key,
            "Added reference"
        );
    }

    /// Remove a specific reference pair.
    pub fn remove_ref(&self, source_key: &str, value: &V) {
        let value_key = value.ref_key();
        {
            let mut maps = self.inner.write();
            if let Some(value_set) = maps.refs.get_mut(source_key) {
                value_set.remove(value);
                if value_set.is_empty() {
                    maps.refs.remove(source_key);
                }
            }
            if let Some(source_set) = maps.dependencies.get_mut(&value_key) {
                source_set.remove(source_key);
                if source_set.is_empty() {
                    maps.dependencies.remove(&value_key);
                }
            }
        }
        tracing::debug!(
            component = self.component,
            source_key = %source_key,
            value = %value_key,
            "Removed reference"
        );
    }

    /// Get all values that reference a given source.
    pub fn get_refs(&self, source_key: &str) -> Vec<V> {
        let maps = self.inner.read();
        maps.refs
            .get(source_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all source keys that a value depends on.
    pub fn get_dependencies(&self, value_key: &str) -> Vec<String> {
        let maps = self.inner.read();
        maps.dependencies
            .get(value_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove all references for a value (e.g., when the resource is deleted
    /// or updated — callers typically clear-then-re-add).
    ///
    /// Returns the list of source keys that were referenced.
    pub fn clear_value_refs(&self, value: &V) -> Vec<String> {
        let value_key = value.ref_key();
        let source_keys: Vec<String> = {
            let mut maps = self.inner.write();
            let source_keys: Vec<String> = maps
                .dependencies
                .remove(&value_key)
                .map(|s| s.into_iter().collect())
                .unwrap_or_default();
            for source_key in &source_keys {
                if let Some(value_set) = maps.refs.get_mut(source_key) {
                    value_set.remove(value);
                    if value_set.is_empty() {
                        maps.refs.remove(source_key);
                    }
                }
            }
            source_keys
        };
        if !source_keys.is_empty() {
            tracing::info!(
                component = self.component,
                value = %value_key,
                source_count = source_keys.len(),
                "Cleared all references for value"
            );
        }
        source_keys
    }

    /// Backward-compatible alias for [`clear_value_refs`](Self::clear_value_refs).
    pub fn clear_resource_refs(&self, value: &V) -> Vec<String> {
        self.clear_value_refs(value)
    }

    /// Remove all references for a source key (e.g., when the source itself is
    /// deleted). Symmetric to [`clear_value_refs`]: removes the forward entry
    /// and scrubs the source key from every affected value's reverse entry.
    ///
    /// Returns the list of values that referenced this source.
    pub fn clear_source_refs(&self, source_key: &str) -> Vec<V> {
        let values: Vec<V> = {
            let mut maps = self.inner.write();
            let values: Vec<V> = maps
                .refs
                .remove(source_key)
                .map(|s| s.into_iter().collect())
                .unwrap_or_default();
            for value in &values {
                let value_key = value.ref_key();
                if let Some(source_set) = maps.dependencies.get_mut(&value_key) {
                    source_set.remove(source_key);
                    if source_set.is_empty() {
                        maps.dependencies.remove(&value_key);
                    }
                }
            }
            values
        };
        if !values.is_empty() {
            tracing::info!(
                component = self.component,
                source_key = %source_key,
                value_count = values.len(),
                "Cleared all references for source"
            );
        }
        values
    }

    /// Return all source keys present in the forward index.
    pub fn all_source_keys(&self) -> Vec<String> {
        let maps = self.inner.read();
        maps.refs.keys().cloned().collect()
    }

    /// Get a statistics snapshot.
    ///
    /// Because both maps are read under the same lock, `source_count`,
    /// `value_count`, and `total_references` are guaranteed to be mutually
    /// consistent.
    pub fn stats(&self) -> RefManagerStats {
        let maps = self.inner.read();
        RefManagerStats {
            source_count: maps.refs.len(),
            value_count: maps.dependencies.len(),
            total_references: maps.refs.values().map(|s| s.len()).sum(),
        }
    }

    /// Clear all references. Used during reload / re-election to reset state.
    pub fn clear(&self) {
        {
            let mut maps = self.inner.write();
            maps.refs.clear();
            maps.dependencies.clear();
        }
        tracing::info!(component = self.component, "Cleared all references");
    }
}

impl<V: RefValue> Default for BidirectionalRefManager<V> {
    fn default() -> Self {
        Self::with_component("ref_manager")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ref(kind: ResourceKind, ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(kind, Some(ns.to_string()), name.to_string())
    }

    // -- ResourceRef tests --------------------------------------------------

    #[test]
    fn test_resource_ref_key() {
        let r = make_ref(ResourceKind::EdgionTls, "default", "my-tls");
        assert_eq!(r.key(), "EdgionTls/default/my-tls");
        assert_eq!(r.resource_key(), "default/my-tls");
        assert_eq!(r.kind_str(), "EdgionTls");

        let r2 = ResourceRef::new(ResourceKind::EdgionTls, None, "cluster-tls".to_string());
        assert_eq!(r2.key(), "EdgionTls//cluster-tls");
        assert_eq!(r2.resource_key(), "cluster-tls");
    }

    #[test]
    fn test_resource_ref_from_key() {
        let r = ResourceRef::from_key("EdgionTls/default/my-tls").unwrap();
        assert_eq!(r.kind, ResourceKind::EdgionTls);
        assert_eq!(r.namespace, Some("default".to_string()));
        assert_eq!(r.name, "my-tls");

        let r2 = ResourceRef::from_key("EdgionTls//cluster-tls").unwrap();
        assert_eq!(r2.namespace, None);
        assert_eq!(r2.name, "cluster-tls");

        assert!(ResourceRef::from_key("bad").is_none());
    }

    // -- BidirectionalRefManager tests --------------------------------------

    #[test]
    fn test_add_and_get_ref() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

        mgr.add_ref("default/my-cert".to_string(), resource.clone());

        let refs = mgr.get_refs("default/my-cert");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], resource);

        let deps = mgr.get_dependencies(&resource.ref_key());
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], "default/my-cert");
    }

    #[test]
    fn test_remove_ref() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

        mgr.add_ref("default/my-cert".to_string(), resource.clone());
        mgr.remove_ref("default/my-cert", &resource);

        assert!(mgr.get_refs("default/my-cert").is_empty());
        assert!(mgr.get_dependencies(&resource.ref_key()).is_empty());
    }

    #[test]
    fn test_clear_value_refs() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

        mgr.add_ref("default/cert1".to_string(), resource.clone());
        mgr.add_ref("default/cert2".to_string(), resource.clone());

        let cleared = mgr.clear_value_refs(&resource);
        assert_eq!(cleared.len(), 2);
        assert!(cleared.contains(&"default/cert1".to_string()));
        assert!(cleared.contains(&"default/cert2".to_string()));

        assert!(mgr.get_refs("default/cert1").is_empty());
        assert!(mgr.get_refs("default/cert2").is_empty());
    }

    #[test]
    fn test_clear_source_refs() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let r1 = make_ref(ResourceKind::EdgionTls, "default", "tls1");
        let r2 = make_ref(ResourceKind::EdgionTls, "default", "tls2");

        mgr.add_ref("default/my-cert".to_string(), r1.clone());
        mgr.add_ref("default/my-cert".to_string(), r2.clone());
        // r1 also depends on a second cert; its reverse entry must be partially trimmed
        mgr.add_ref("default/other-cert".to_string(), r1.clone());

        let cleared = mgr.clear_source_refs("default/my-cert");
        assert_eq!(cleared.len(), 2);
        assert!(cleared.contains(&r1));
        assert!(cleared.contains(&r2));

        // forward entry removed
        assert!(mgr.get_refs("default/my-cert").is_empty());

        // r1's reverse entry still has "other-cert"; r2's reverse entry gone
        let r1_deps = mgr.get_dependencies(&r1.ref_key());
        assert_eq!(r1_deps, vec!["default/other-cert".to_string()]);
        assert!(mgr.get_dependencies(&r2.ref_key()).is_empty());

        // idempotent
        let again = mgr.clear_source_refs("default/my-cert");
        assert!(again.is_empty());
    }

    #[test]
    fn test_multiple_values_same_source() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let r1 = make_ref(ResourceKind::EdgionTls, "default", "tls1");
        let r2 = make_ref(ResourceKind::EdgionTls, "default", "tls2");

        mgr.add_ref("default/my-cert".to_string(), r1);
        mgr.add_ref("default/my-cert".to_string(), r2);

        assert_eq!(mgr.get_refs("default/my-cert").len(), 2);
    }

    #[test]
    fn test_idempotent_add() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

        mgr.add_ref("default/my-cert".to_string(), resource.clone());
        mgr.add_ref("default/my-cert".to_string(), resource.clone());
        mgr.add_ref("default/my-cert".to_string(), resource.clone());

        assert_eq!(mgr.get_refs("default/my-cert").len(), 1);
    }

    #[test]
    fn test_all_source_keys() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::HTTPRoute, "app", "route1");

        mgr.add_ref("backend1".to_string(), resource.clone());
        mgr.add_ref("backend2".to_string(), resource);

        let keys = mgr.all_source_keys();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"backend1".to_string()));
        assert!(keys.contains(&"backend2".to_string()));
    }

    #[test]
    fn test_stats() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let r1 = make_ref(ResourceKind::HTTPRoute, "app1", "route1");
        let r2 = make_ref(ResourceKind::HTTPRoute, "app2", "route2");

        mgr.add_ref("backend1".to_string(), r1.clone());
        mgr.add_ref("backend2".to_string(), r1);
        mgr.add_ref("backend1".to_string(), r2);

        let stats = mgr.stats();
        assert_eq!(stats.source_count, 2);
        assert_eq!(stats.value_count, 2);
        assert_eq!(stats.total_references, 3);
    }

    #[test]
    fn test_clear() {
        let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
        let resource = make_ref(ResourceKind::EdgionTls, "default", "tls1");
        mgr.add_ref("s1".to_string(), resource);

        assert_eq!(mgr.stats().source_count, 1);
        mgr.clear();
        assert_eq!(mgr.stats().source_count, 0);
        assert_eq!(mgr.stats().value_count, 0);
    }

    /// Concurrent stress test: simulates multiple ResourceController workers doing
    /// the standard handler pattern (clear_value_refs → add_ref) concurrently.
    ///
    /// With two separate RwLocks and reversed acquisition order between
    /// `add_ref` (refs→deps) and `clear_value_refs` (deps→refs), this test
    /// will deadlock. After merging into a single lock it must complete promptly.
    #[test]
    fn test_concurrent_clear_and_readd_no_deadlock() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let mgr = Arc::new(BidirectionalRefManager::<ResourceRef>::new());

        // Pre-populate so there is something to clear immediately
        for i in 0..5u32 {
            let r = make_ref(ResourceKind::HTTPRoute, "ns", &format!("r{i}"));
            mgr.add_ref(format!("s{}", i % 3), r);
        }

        let (tx, rx) = mpsc::channel::<()>();

        // 4 threads each simulating a handler reconcile loop:
        //   clear old refs → add new refs   (high-contention, small key space)
        for t in 0..4u32 {
            let mgr = mgr.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                for i in 0..500u32 {
                    let resource = make_ref(ResourceKind::HTTPRoute, "ns", &format!("r{}", (t + i) % 5));
                    mgr.clear_value_refs(&resource);
                    mgr.add_ref(format!("s{}", i % 3), resource);
                }
                let _ = tx.send(());
            });
        }
        drop(tx); // drop sender so recv_timeout can detect all done

        // All 4 threads must finish within 5 seconds total.
        // With the buggy dual-lock ordering this test deadlocks here.
        let deadline = Duration::from_secs(5);
        let start = std::time::Instant::now();
        let mut finished = 0;
        while finished < 4 {
            let remaining = deadline.saturating_sub(std::time::Instant::now().saturating_duration_since(start));
            match rx.recv_timeout(remaining) {
                Ok(()) => finished += 1,
                Err(_) => panic!(
                    "Deadlock detected: concurrent add_ref / clear_value_refs \
                     did not complete within {:?}",
                    deadline
                ),
            }
        }
    }
}
