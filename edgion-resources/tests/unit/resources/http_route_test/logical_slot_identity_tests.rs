use super::*;

fn backend(name: &str, namespace: Option<&str>) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        port: None,
        weight: Some(1),
        group: Some("edgion.io".to_string()),
        kind: Some("EdgionBackend".to_string()),
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

#[test]
fn logical_identity_normalizes_namespace_and_counts_equal_tuple_occurrences() {
    let refs = vec![
        backend("same", None),
        backend("other", None),
        backend("same", None),
        backend("same", Some("elsewhere")),
    ];
    let identities = logical_backend_slot_identities(&refs, "route-ns");
    assert_eq!(identities[0].namespace, "route-ns");
    assert_eq!(identities[0].occurrence, 0);
    assert_eq!(identities[2].occurrence, 1);
    assert_eq!(identities[3].namespace, "elsewhere");
    assert_eq!(identities[3].occurrence, 0);
    assert!(
        identities[2].matches_ai_selector(&crate::resources::edgion_plugins::AiBackendSlotSelector {
            group: "edgion.io".to_string(),
            kind: "EdgionBackend".to_string(),
            namespace: "route-ns".to_string(),
            name: "same".to_string(),
            section: None,
            occurrence: 1,
        })
    );
}
