//! Global Namespace Store (controller-only)
//!
//! Provides a global store for Kubernetes Namespace objects, populated by a
//! lightweight K8s Namespace watcher. Used by Selector namespace policy
//! evaluation in `route_utils::listener_allows_route_namespace()`.
//!
//! Design mirrors `SecretStore`: global `LazyLock`, `RwLock<HashMap>`,
//! controller-only (never synced to Gateway).

use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};

use k8s_openapi::api::core::v1::Namespace;
use kube::ResourceExt;

pub struct NamespaceStore {
    store: RwLock<HashMap<String, Namespace>>,
}

impl Default for NamespaceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl NamespaceStore {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }

    /// Insert or update a namespace.
    /// Returns true if the namespace's labels actually changed (triggers requeue).
    pub fn upsert(&self, ns: Namespace) -> bool {
        let name = ns.name_any();
        let new_labels = ns.labels().clone();
        let mut map = self.store.write();
        let changed = map.get(&name).is_none_or(|old| old.labels() != &new_labels);
        map.insert(name, ns);
        changed
    }

    /// Remove a namespace from the store.
    pub fn remove(&self, name: &str) -> bool {
        self.store.write().remove(name).is_some()
    }

    /// Get labels for a namespace (extracted on demand from stored object).
    pub fn get_labels(&self, name: &str) -> Option<BTreeMap<String, String>> {
        self.store
            .read()
            .get(name)
            .map(|ns| ns.labels().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    /// Get the full namespace object.
    pub fn get(&self, name: &str) -> Option<Namespace> {
        self.store.read().get(name).cloned()
    }

    /// Replace all entries atomically (used during init/full-sync).
    pub fn replace_all(&self, namespaces: Vec<Namespace>) {
        let new_map: HashMap<String, Namespace> = namespaces.into_iter().map(|ns| (ns.name_any(), ns)).collect();
        *self.store.write() = new_map;
    }
}

// ---------------------------------------------------------------------------
// LabelSelector matching
// ---------------------------------------------------------------------------

/// Evaluate a Kubernetes LabelSelector (as `serde_json::Value`) against a set
/// of labels.
///
/// LabelSelector semantics (all conditions ANDed):
/// - `matchLabels`: every key=value pair must match
/// - `matchExpressions`: every expression must match
///   - `In`: label value must be in the values set
///   - `NotIn`: label value must NOT be in the values set
///   - `Exists`: label key must exist
///   - `DoesNotExist`: label key must NOT exist
/// - Empty selector (`{}`) matches everything.
pub fn label_selector_matches(selector: &serde_json::Value, labels: &BTreeMap<String, String>) -> bool {
    if selector.is_null() {
        return true;
    }
    let Some(obj) = selector.as_object() else {
        return true;
    };
    if obj.is_empty() {
        return true;
    }

    if let Some(match_labels) = obj.get("matchLabels") {
        if let Some(ml) = match_labels.as_object() {
            for (key, val) in ml {
                let expected = val.as_str().unwrap_or("");
                match labels.get(key.as_str()) {
                    Some(actual) if actual == expected => {}
                    _ => return false,
                }
            }
        }
    }

    if let Some(match_expressions) = obj.get("matchExpressions") {
        if let Some(exprs) = match_expressions.as_array() {
            for expr in exprs {
                if !evaluate_expression(expr, labels) {
                    return false;
                }
            }
        }
    }

    true
}

fn evaluate_expression(expr: &serde_json::Value, labels: &BTreeMap<String, String>) -> bool {
    let key = expr.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let operator = expr.get("operator").and_then(|v| v.as_str()).unwrap_or("");
    let values: Vec<&str> = expr
        .get("values")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    match operator {
        "In" => labels.get(key).is_some_and(|v| values.contains(&v.as_str())),
        "NotIn" => labels.get(key).is_none_or(|v| !values.contains(&v.as_str())),
        "Exists" => labels.contains_key(key),
        "DoesNotExist" => !labels.contains_key(key),
        unknown => {
            tracing::warn!(operator = %unknown, key = %key, "Unknown LabelSelector operator");
            false
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/namespace_store_test.rs"]
mod tests;
