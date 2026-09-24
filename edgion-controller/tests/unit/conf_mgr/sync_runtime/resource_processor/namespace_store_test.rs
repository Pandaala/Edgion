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
