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
//! ## Usage
//!
//! Concrete managers are type aliases:
//! - `SecretRefManager = BidirectionalRefManager<ResourceRef>`
//! - `CrossNamespaceRefManager = BidirectionalRefManager<ResourceRef>`
//! - `ServiceRefManager = BidirectionalRefManager<ResourceRef>`

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::RwLock;

use crate::types::ResourceKind;

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
// BidirectionalRefManager<V>
// ---------------------------------------------------------------------------

/// Thread-safe bidirectional many-to-many index.
///
/// - **Forward**: `source_key → HashSet<V>` — which values reference the source
/// - **Reverse**: `value_key → HashSet<source_key>` — which sources a value depends on
///
/// All mutating operations maintain both indices in sync.
pub struct BidirectionalRefManager<V: RefValue> {
    refs: RwLock<HashMap<String, HashSet<V>>>,
    dependencies: RwLock<HashMap<String, HashSet<String>>>,
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
            refs: RwLock::new(HashMap::new()),
            dependencies: RwLock::new(HashMap::new()),
            component,
        }
    }

    /// Add a reference: `value` depends on `source_key`.
    ///
    /// Idempotent — adding the same pair multiple times is safe.
    pub fn add_ref(&self, source_key: String, value: V) {
        let value_key = value.ref_key();

        {
            let mut refs = self.refs.write().unwrap();
            refs.entry(source_key.clone()).or_default().insert(value.clone());
        }

        {
            let mut deps = self.dependencies.write().unwrap();
            deps.entry(value_key.clone()).or_default().insert(source_key.clone());
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
            let mut refs = self.refs.write().unwrap();
            if let Some(value_set) = refs.get_mut(source_key) {
                value_set.remove(value);
                if value_set.is_empty() {
                    refs.remove(source_key);
                }
            }
        }

        {
            let mut deps = self.dependencies.write().unwrap();
            if let Some(source_set) = deps.get_mut(&value_key) {
                source_set.remove(source_key);
                if source_set.is_empty() {
                    deps.remove(&value_key);
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
        let refs = self.refs.read().unwrap();
        refs.get(source_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all source keys that a value depends on.
    pub fn get_dependencies(&self, value_key: &str) -> Vec<String> {
        let deps = self.dependencies.read().unwrap();
        deps.get(value_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove all references for a value (e.g., when the resource is deleted
    /// or updated — callers typically clear-then-re-add).
    ///
    /// Returns the list of source keys that were referenced.
    pub fn clear_value_refs(&self, value: &V) -> Vec<String> {
        let value_key = value.ref_key();

        let source_keys = {
            let mut deps = self.dependencies.write().unwrap();
            deps.remove(&value_key).unwrap_or_default()
        };

        {
            let mut refs = self.refs.write().unwrap();
            for source_key in &source_keys {
                if let Some(value_set) = refs.get_mut(source_key) {
                    value_set.remove(value);
                    if value_set.is_empty() {
                        refs.remove(source_key);
                    }
                }
            }
        }

        if !source_keys.is_empty() {
            tracing::info!(
                component = self.component,
                value = %value_key,
                source_count = source_keys.len(),
                "Cleared all references for value"
            );
        }

        source_keys.into_iter().collect()
    }

    /// Backward-compatible alias for [`clear_value_refs`](Self::clear_value_refs).
    pub fn clear_resource_refs(&self, value: &V) -> Vec<String> {
        self.clear_value_refs(value)
    }

    /// Return all source keys present in the forward index.
    pub fn all_source_keys(&self) -> Vec<String> {
        let refs = self.refs.read().unwrap();
        refs.keys().cloned().collect()
    }

    /// Get a statistics snapshot.
    pub fn stats(&self) -> RefManagerStats {
        let refs = self.refs.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        RefManagerStats {
            source_count: refs.len(),
            value_count: deps.len(),
            total_references: refs.values().map(|set| set.len()).sum(),
        }
    }

    /// Clear all references. Used during reload / re-election to reset state.
    pub fn clear(&self) {
        {
            let mut refs = self.refs.write().unwrap();
            refs.clear();
        }
        {
            let mut deps = self.dependencies.write().unwrap();
            deps.clear();
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
}
