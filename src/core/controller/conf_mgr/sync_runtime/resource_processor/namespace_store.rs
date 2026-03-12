//! Global Namespace Store (controller-only)
//!
//! Provides a global store for Kubernetes Namespace objects, populated by a
//! lightweight K8s Namespace watcher. Used by Selector namespace policy
//! evaluation in `route_utils::listener_allows_route_namespace()`.
//!
//! Design mirrors `SecretStore`: global `LazyLock`, `RwLock<HashMap>`,
//! controller-only (never synced to Gateway).

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

use k8s_openapi::api::core::v1::Namespace;
use kube::ResourceExt;

static NAMESPACE_STORE: LazyLock<NamespaceStore> = LazyLock::new(NamespaceStore::new);

pub fn get_namespace_store() -> &'static NamespaceStore {
    &NAMESPACE_STORE
}

pub struct NamespaceStore {
    store: RwLock<HashMap<String, Namespace>>,
}

impl NamespaceStore {
    fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }

    /// Insert or update a namespace.
    /// Returns true if the namespace's labels actually changed (triggers requeue).
    pub fn upsert(&self, ns: Namespace) -> bool {
        let name = ns.name_any();
        let new_labels = ns.labels().clone();
        let mut map = self.store.write().unwrap();
        let changed = map.get(&name).is_none_or(|old| old.labels() != &new_labels);
        map.insert(name, ns);
        changed
    }

    /// Remove a namespace from the store.
    pub fn remove(&self, name: &str) -> bool {
        self.store.write().unwrap().remove(name).is_some()
    }

    /// Get labels for a namespace (extracted on demand from stored object).
    pub fn get_labels(&self, name: &str) -> Option<BTreeMap<String, String>> {
        self.store
            .read()
            .unwrap()
            .get(name)
            .map(|ns| ns.labels().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    /// Get the full namespace object.
    pub fn get(&self, name: &str) -> Option<Namespace> {
        self.store.read().unwrap().get(name).cloned()
    }

    /// Replace all entries atomically (used during init/full-sync).
    pub fn replace_all(&self, namespaces: Vec<Namespace>) {
        let new_map: HashMap<String, Namespace> = namespaces.into_iter().map(|ns| (ns.name_any(), ns)).collect();
        *self.store.write().unwrap() = new_map;
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
mod tests {
    use super::*;

    // ---- LabelSelector matching tests ----

    #[test]
    fn test_empty_selector_matches_all() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        assert!(label_selector_matches(&serde_json::json!({}), &labels));
        assert!(label_selector_matches(&serde_json::Value::Null, &labels));
    }

    #[test]
    fn test_match_labels() {
        let labels = BTreeMap::from([("env".into(), "prod".into()), ("team".into(), "platform".into())]);

        let hit = serde_json::json!({ "matchLabels": { "env": "prod" } });
        assert!(label_selector_matches(&hit, &labels));

        let miss = serde_json::json!({ "matchLabels": { "env": "staging" } });
        assert!(!label_selector_matches(&miss, &labels));
    }

    #[test]
    fn test_match_labels_missing_key() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({ "matchLabels": { "tier": "frontend" } });
        assert!(!label_selector_matches(&selector, &labels));
    }

    #[test]
    fn test_match_expressions_in() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{
                "key": "env", "operator": "In", "values": ["prod", "staging"]
            }]
        });
        assert!(label_selector_matches(&selector, &labels));
    }

    #[test]
    fn test_match_expressions_in_miss() {
        let labels = BTreeMap::from([("env".into(), "dev".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{
                "key": "env", "operator": "In", "values": ["prod", "staging"]
            }]
        });
        assert!(!label_selector_matches(&selector, &labels));
    }

    #[test]
    fn test_match_expressions_not_in() {
        let labels = BTreeMap::from([("env".into(), "dev".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{
                "key": "env", "operator": "NotIn", "values": ["prod", "staging"]
            }]
        });
        assert!(label_selector_matches(&selector, &labels));
    }

    #[test]
    fn test_match_expressions_not_in_miss() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{
                "key": "env", "operator": "NotIn", "values": ["prod", "staging"]
            }]
        });
        assert!(!label_selector_matches(&selector, &labels));
    }

    #[test]
    fn test_match_expressions_exists() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{ "key": "env", "operator": "Exists" }]
        });
        assert!(label_selector_matches(&selector, &labels));

        let empty_labels = BTreeMap::new();
        assert!(!label_selector_matches(&selector, &empty_labels));
    }

    #[test]
    fn test_match_expressions_does_not_exist() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{ "key": "tier", "operator": "DoesNotExist" }]
        });
        assert!(label_selector_matches(&selector, &labels));

        let with_tier = BTreeMap::from([("tier".into(), "frontend".into())]);
        assert!(!label_selector_matches(&selector, &with_tier));
    }

    #[test]
    fn test_combined_match_labels_and_expressions() {
        let labels = BTreeMap::from([("env".into(), "prod".into()), ("team".into(), "platform".into())]);
        let selector = serde_json::json!({
            "matchLabels": { "env": "prod" },
            "matchExpressions": [{
                "key": "team", "operator": "In", "values": ["platform", "infra"]
            }]
        });
        assert!(label_selector_matches(&selector, &labels));

        let wrong_team = BTreeMap::from([("env".into(), "prod".into()), ("team".into(), "sales".into())]);
        assert!(!label_selector_matches(&selector, &wrong_team));
    }

    #[test]
    fn test_unknown_operator_returns_false() {
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        let selector = serde_json::json!({
            "matchExpressions": [{
                "key": "env", "operator": "GreaterThan", "values": ["1"]
            }]
        });
        assert!(!label_selector_matches(&selector, &labels));
    }

    // ---- NamespaceStore tests ----

    fn make_namespace(name: &str, labels: BTreeMap<String, String>) -> Namespace {
        Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(name.to_string()),
                labels: if labels.is_empty() { None } else { Some(labels) },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_store_upsert_and_get() {
        let store = NamespaceStore::new();
        let labels = BTreeMap::from([("env".into(), "prod".into())]);

        assert!(store.upsert(make_namespace("test-ns", labels.clone())));
        assert!(!store.upsert(make_namespace("test-ns", labels.clone())));
        assert_eq!(store.get_labels("test-ns"), Some(labels));
        assert_eq!(store.get_labels("unknown"), None);
    }

    #[test]
    fn test_store_upsert_detects_label_change() {
        let store = NamespaceStore::new();
        let labels_v1 = BTreeMap::from([("env".into(), "dev".into())]);
        let labels_v2 = BTreeMap::from([("env".into(), "prod".into())]);

        assert!(store.upsert(make_namespace("ns1", labels_v1)));
        assert!(store.upsert(make_namespace("ns1", labels_v2.clone())));
        assert_eq!(store.get_labels("ns1"), Some(labels_v2));
    }

    #[test]
    fn test_store_get_full_object() {
        let store = NamespaceStore::new();
        let labels = BTreeMap::from([("env".into(), "prod".into())]);
        store.upsert(make_namespace("test-ns", labels));

        let ns = store.get("test-ns");
        assert!(ns.is_some());
        assert_eq!(ns.unwrap().metadata.name.as_deref(), Some("test-ns"));
    }

    #[test]
    fn test_store_remove() {
        let store = NamespaceStore::new();
        store.upsert(make_namespace("test-ns", BTreeMap::new()));
        assert!(store.remove("test-ns"));
        assert!(!store.remove("test-ns"));
        assert!(store.get("test-ns").is_none());
    }

    #[test]
    fn test_store_replace_all() {
        let store = NamespaceStore::new();
        store.upsert(make_namespace("old-ns", BTreeMap::new()));

        let new_nss = vec![
            make_namespace("ns-a", BTreeMap::from([("a".into(), "1".into())])),
            make_namespace("ns-b", BTreeMap::from([("b".into(), "2".into())])),
        ];
        store.replace_all(new_nss);

        assert!(store.get("old-ns").is_none());
        assert!(store.get("ns-a").is_some());
        assert!(store.get("ns-b").is_some());
    }
}
