use super::*;
use crate::resources::*;
use kube::Resource;
use std::collections::HashSet;

fn typed_identity<T: Resource<DynamicType = ()>>() -> (String, String, String) {
    (
        T::group(&()).to_string(),
        T::version(&()).to_string(),
        T::kind(&()).to_string(),
    )
}

#[test]
fn canonical_inventory_matches_the_actual_rust_resource_types() {
    let types = [
        typed_identity::<GatewayClass>(),
        typed_identity::<Gateway>(),
        typed_identity::<HTTPRoute>(),
        typed_identity::<GRPCRoute>(),
        typed_identity::<TLSRoute>(),
        typed_identity::<ReferenceGrant>(),
        typed_identity::<TCPRoute>(),
        typed_identity::<UDPRoute>(),
        typed_identity::<BackendTLSPolicy>(),
    ];
    let inventory: HashSet<_> = canonical_sources()
        .map(|source| (GROUP.to_string(), source.version.to_string(), source.kind.to_string()))
        .collect();
    assert_eq!(inventory.len(), types.len());
    assert_eq!(inventory, HashSet::from(types));
}

#[test]
fn alternate_sources_are_not_advertised_as_complete() {
    assert!(SOURCES
        .iter()
        .filter(|source| source.version == "v1beta1")
        .all(|source| source.state != SourceState::Canonical));
    let beta = SOURCES
        .iter()
        .find(|source| source.kind == "ReferenceGrant" && source.version == "v1beta1")
        .unwrap();
    assert_eq!(ReferenceGrantBeta::version(&()), beta.version);
    assert_eq!(ReferenceGrantBeta::kind(&()), "ReferenceGrant");
    assert_eq!(beta.paths.kubernetes_watch, PathState::Wired);
    assert_eq!(beta.paths.kubernetes_crud, PathState::Wired);
    assert_eq!(beta.paths.status_transport, PathState::NotApplicable);
    assert_eq!(beta.paths.raw_type_meta_guard, PathState::Wired);
    assert_eq!(beta.paths.converter, PathState::Wired);
    let route = SOURCES
        .iter()
        .find(|source| source.kind == "HTTPRoute" && source.version == "v1beta1")
        .unwrap();
    assert_eq!(HTTPRouteBeta::version(&()), route.version);
    assert_eq!(route.state, SourceState::ImplementedUnverified);
    assert_eq!(route.paths.status_transport, PathState::Wired);
}

#[test]
fn reference_grant_beta_conversion_preserves_identity_and_rules() {
    let beta: ReferenceGrantBeta = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1",
        "kind": "ReferenceGrant",
        "metadata": {"name":"allow", "namespace":"source", "resourceVersion":"17"},
        "spec": {
            "from": [{"group":"gateway.networking.k8s.io", "kind":"HTTPRoute", "namespace":"client"}],
            "to": [{"group":"", "kind":"Service", "name":"backend"}]
        }
    }))
    .unwrap();
    let expected_metadata = beta.metadata.clone();
    let expected_from = beta.spec.from.clone();
    let expected_to = beta.spec.to.clone();
    let canonical: ReferenceGrant = beta.into();
    assert_eq!(canonical.metadata, expected_metadata);
    assert_eq!(canonical.spec.from, expected_from);
    assert_eq!(canonical.spec.to, expected_to);
}

#[test]
fn inventory_cannot_promote_an_incomplete_source_path() {
    let mut identities = HashSet::new();
    for source in SOURCES {
        assert!(identities.insert((source.kind, source.version)));
        if source.state == SourceState::Canonical {
            assert_eq!(source.paths.kubernetes_watch, PathState::Wired);
            assert_eq!(source.paths.kubernetes_crud, PathState::Wired);
            assert_eq!(source.paths.raw_type_meta_guard, PathState::Wired);
        } else if source.state == SourceState::SupportedAlternate {
            assert_eq!(source.paths.kubernetes_watch, PathState::Wired);
            assert_eq!(source.paths.kubernetes_crud, PathState::Wired);
            assert!(matches!(
                source.paths.status_transport,
                PathState::Wired | PathState::NotApplicable
            ));
            assert_eq!(source.paths.raw_type_meta_guard, PathState::Wired);
            assert_eq!(source.paths.converter, PathState::Wired);
            assert!(!source.evidence.standard_bundles.is_empty() || !source.evidence.experimental_bundles.is_empty());
            assert!(source.evidence.conformance_suite.is_some());
        } else {
            assert!(source.evidence.conformance_suite.is_none());
        }
    }
    assert_eq!(
        declared_supported_features(),
        crate::resources::gateway_class::SUPPORTED_FEATURES
    );
    assert_eq!(complete_alternate_sources().count(), 0);
}
