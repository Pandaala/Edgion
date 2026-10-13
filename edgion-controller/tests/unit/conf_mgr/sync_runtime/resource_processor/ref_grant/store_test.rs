use super::*;
use edgion_resources::resources::{ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

#[test]
fn bound_authorization_revision_tracks_replacement_and_revocation() {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
        "metadata":{"name":"auth", "namespace":"source"},
        "spec":{"requestPlugins":[{"type":"Csrf", "config":{"secretRef":{"name":"key", "namespace":"target"}}}]}
    }))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let token = references.csrf_secret().unwrap();
    let query = token.validated().unwrap().grant_query();
    let store = ReferenceGrantStore::new();
    assert!(!store.check_query(query));
    assert_eq!(store.query_authorization_revision(query), None);
    let mut grant = create_test_grant("target", "allow", "source", "EdgionPlugins", "Secret");
    grant.spec.from[0].group = "edgion.io".to_string();
    grant.metadata.uid = Some("uid-a".to_string());
    grant.metadata.resource_version = Some("1".to_string());
    store.upsert("target/allow".to_string(), Arc::new(grant.clone()));
    assert!(store.check_query(query));
    assert_eq!(store.query_authorization_revision(query).as_deref(), Some("uid-a:1"));
    grant.metadata.uid = Some("uid-b".to_string());
    grant.metadata.resource_version = Some("2".to_string());
    store.upsert("target/allow".to_string(), Arc::new(grant.clone()));
    assert_eq!(store.query_authorization_revision(query).as_deref(), Some("uid-b:2"));
    let mut second = grant.clone();
    second.metadata.uid = Some("uid-a".into());
    second.metadata.resource_version = Some("3".into());
    store.upsert("another-raw-key".into(), Arc::new(second));
    assert_eq!(
        store.query_authorization_revision(query).as_deref(),
        Some("uid-a:3,uid-b:2")
    );
    store.remove("another-raw-key");
    grant.spec.to[0].name = Some("other".to_string());
    store.upsert("target/allow".to_string(), Arc::new(grant));
    assert!(!store.check_query(query));
    assert_eq!(store.query_authorization_revision(query), None);
    store.remove("target/allow");
    assert_eq!(store.query_authorization_revision(query), None);
}

fn create_test_grant(
    namespace: &str,
    name: &str,
    from_namespace: &str,
    from_kind: &str,
    to_kind: &str,
) -> ReferenceGrant {
    ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: "gateway.networking.k8s.io".to_string(),
                kind: from_kind.to_string(),
                namespace: from_namespace.to_string(),
            }],
            to: vec![ReferenceGrantTo {
                group: "".to_string(),
                kind: to_kind.to_string(),
                name: None,
            }],
        },
    }
}

#[test]
fn test_basic_crud() {
    let store = ReferenceGrantStore::new();
    let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");

    let mut grants = HashMap::new();
    grants.insert("ns-target/test-grant".to_string(), Arc::new(grant));

    store.replace_all(grants);

    // Test get
    assert!(store.get("ns-target/test-grant").is_some());
    assert!(store.get("ns-target/nonexistent").is_none());

    // Test get_by_ns_name
    assert!(store.get_by_ns_name("ns-target", "test-grant").is_some());
    assert!(store.get_by_ns_name("ns-target", "nonexistent").is_none());
}

#[test]
fn test_upsert_and_remove() {
    let store = ReferenceGrantStore::new();
    let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");

    // Upsert
    store.upsert("ns-target/test-grant".to_string(), Arc::new(grant));
    assert!(store.get("ns-target/test-grant").is_some());
    assert_eq!(store.get_by_to_namespace("ns-target").len(), 1);

    // Remove
    let removed = store.remove("ns-target/test-grant");
    assert!(removed.is_some());
    assert!(store.get("ns-target/test-grant").is_none());
    assert_eq!(store.get_by_to_namespace("ns-target").len(), 0);
}

#[test]
fn test_index_query() {
    let store = ReferenceGrantStore::new();

    let grant1 = create_test_grant("ns1", "grant1", "ns-source", "HTTPRoute", "Service");
    let grant2 = create_test_grant("ns1", "grant2", "ns-source2", "TCPRoute", "Service");
    let grant3 = create_test_grant("ns2", "grant3", "ns-source", "HTTPRoute", "Secret");

    let mut grants = HashMap::new();
    grants.insert("ns1/grant1".to_string(), Arc::new(grant1));
    grants.insert("ns1/grant2".to_string(), Arc::new(grant2));
    grants.insert("ns2/grant3".to_string(), Arc::new(grant3));

    store.replace_all(grants);

    // Query by to_namespace
    let ns1_grants = store.get_by_to_namespace("ns1");
    assert_eq!(ns1_grants.len(), 2);

    let ns2_grants = store.get_by_to_namespace("ns2");
    assert_eq!(ns2_grants.len(), 1);

    let ns_nonexistent = store.get_by_to_namespace("ns-nonexistent");
    assert_eq!(ns_nonexistent.len(), 0);
}

#[test]
fn test_check_reference_allowed_same_namespace() {
    let store = ReferenceGrantStore::new();

    // Same namespace references should always be allowed
    assert!(store.check_reference_allowed(
        "ns1",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "ns1",
        "",
        "Service",
        Some("my-service")
    ));
}

#[test]
fn test_check_reference_allowed_cross_namespace() {
    let store = ReferenceGrantStore::new();

    // Create a grant that allows HTTPRoute from ns-source to access Service in ns-target
    let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");
    let mut grants = HashMap::new();
    grants.insert("ns-target/test-grant".to_string(), Arc::new(grant));
    store.replace_all(grants);

    // Should allow: grant exists
    assert!(store.check_reference_allowed(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "ns-target",
        "",
        "Service",
        Some("my-service")
    ));

    // Should deny: no grant for different source namespace
    assert!(!store.check_reference_allowed(
        "ns-other",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "ns-target",
        "",
        "Service",
        Some("my-service")
    ));
}

fn assert_index_matches_raw(store: &ReferenceGrantStore) {
    let state = store.state.read();
    let indexed_count: usize = state.by_to_namespace.values().map(HashMap::len).sum();
    assert_eq!(
        indexed_count,
        state.raw.values().filter(|grant| grant.namespace().is_some()).count()
    );
    assert!(state.by_to_namespace.values().all(|bucket| !bucket.is_empty()));
    for (namespace, bucket) in &state.by_to_namespace {
        for (key, grant) in bucket {
            assert_eq!(grant.namespace(), Some(namespace.as_str()));
            assert!(Arc::ptr_eq(grant, state.raw.get(key).unwrap()));
        }
    }
}

#[test]
fn exact_key_updates_preserve_aliases_and_relocate_namespace_membership() {
    let store = ReferenceGrantStore::new();
    let shared = Arc::new(create_test_grant("old", "shared", "source", "HTTPRoute", "Service"));
    let unrelated = Arc::new(create_test_grant("other", "unchanged", "source", "HTTPRoute", "Secret"));
    store.replace_all(HashMap::from([
        ("raw-a".into(), Arc::clone(&shared)),
        ("raw-alias".into(), Arc::clone(&shared)),
        ("unrelated".into(), Arc::clone(&unrelated)),
    ]));
    assert_eq!(store.get_by_to_namespace("old").len(), 2);
    assert_index_matches_raw(&store);

    let moved = Arc::new(create_test_grant("other", "moved", "source", "HTTPRoute", "Service"));
    store.upsert("raw-a".into(), Arc::clone(&moved));
    assert_index_matches_raw(&store);
    assert!(Arc::ptr_eq(&store.get_by_to_namespace("old")[0], &shared));
    assert_eq!(store.get_by_to_namespace("other").len(), 2);
    let mut no_namespace = (*shared).clone();
    no_namespace.metadata.namespace = None;
    store.upsert("raw-alias".into(), Arc::new(no_namespace));
    assert_index_matches_raw(&store);
    assert!(!store.state.read().by_to_namespace.contains_key("old"));
    assert!(store.get("raw-alias").is_some());
    store.upsert("raw-alias".into(), Arc::clone(&moved));
    assert_index_matches_raw(&store);
    assert_eq!(store.get_by_to_namespace("other").len(), 3);

    assert!(Arc::ptr_eq(&store.remove("raw-a").unwrap(), &moved));
    assert!(store.remove("raw-a").is_none());
    assert_index_matches_raw(&store);
    store.remove("raw-alias");
    assert!(Arc::ptr_eq(&store.get_by_to_namespace("other")[0], &unrelated));
    assert_index_matches_raw(&store);
    store.remove("unrelated");
    assert!(store.state.read().by_to_namespace.is_empty());
    assert_index_matches_raw(&store);
}

#[test]
fn full_replacement_rebuilds_only_current_namespaced_grants() {
    let store = ReferenceGrantStore::new();
    store.upsert(
        "old".into(),
        Arc::new(create_test_grant("old", "old", "source", "HTTPRoute", "Service")),
    );
    let named = Arc::new(create_test_grant("new", "named", "source", "HTTPRoute", "Service"));
    let mut unnamed = (*named).clone();
    unnamed.metadata.namespace = None;
    store.replace_all(HashMap::from([
        ("new-a".into(), Arc::clone(&named)),
        ("new-b".into(), named),
        ("unnamed".into(), Arc::new(unnamed)),
    ]));
    assert!(store.get("old").is_none());
    assert!(store.get_by_to_namespace("old").is_empty());
    assert_eq!(store.get_by_to_namespace("new").len(), 2);
    assert_index_matches_raw(&store);
    assert!(store.remove("unnamed").is_some());
    assert_index_matches_raw(&store);
    store.replace_all(HashMap::new());
    assert!(store.state.read().raw.is_empty());
    assert!(store.state.read().by_to_namespace.is_empty());
}

impl ReferenceGrantStore {
    /// Get a reference grant by namespace and name
    #[cfg(test)]
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<ReferenceGrant>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Get all grants (for testing/debugging)
    #[cfg(test)]
    pub fn get_all(&self) -> HashMap<String, Arc<ReferenceGrant>> {
        self.state.read().raw.clone()
    }
}
