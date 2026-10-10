use super::super::ReferenceGrantStore;
use super::*;
use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
use edgion_resources::resources::refs::route_fields::{AuthoredRouteTarget, HttpRouteBackendRefs};
use edgion_resources::resources::refs::{GrantQuery, ReferenceError};
use edgion_resources::resources::{gateway::SecretObjectReference, http_route::BackendObjectReference};
use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
use std::sync::Arc;

#[test]
fn tcp_and_udp_multi_rule_routes_are_invalid_after_api_server_conversion() {
    let spec = serde_json::json!({
        "rules": [
            {"backendRefs": [{"name": "first", "port": 8080}]},
            {"backendRefs": [{"name": "second", "port": 8080}]}
        ]
    });
    for kind in ["TCPRoute", "UDPRoute"] {
        let route = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": kind,
            "metadata": {"name": "route", "namespace": "default"},
            "spec": spec.clone()
        });
        if kind == "TCPRoute" {
            let mut route: edgion_resources::TCPRoute = serde_json::from_value(route).unwrap();
            assert_eq!(validate_tcp_route_if_enabled(&route).len(), 1);
            route.spec.rules.as_mut().unwrap().pop();
            assert!(validate_tcp_route_if_enabled(&route).is_empty());
        } else {
            let mut route: edgion_resources::UDPRoute = serde_json::from_value(route).unwrap();
            assert_eq!(validate_udp_route_if_enabled(&route).len(), 1);
            route.spec.rules.as_mut().unwrap().pop();
            assert!(validate_udp_route_if_enabled(&route).is_empty());
        }
    }
}
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::HashMap;

// ---- Test helpers --------------------------------------------------------

#[test]
fn http_header_validation_ignores_only_later_equivalent_names() {
    for invalid in [
        serde_json::json!({"name": "x-mode", "type": "RegularExpression", "value": "["}),
        serde_json::json!({"name": "x-mode", "type": "Unknown", "value": "two"}),
    ] {
        for invalid_first in [false, true] {
            let mut headers = vec![serde_json::json!({"name": "X-Mode", "value": "one"}), invalid.clone()];
            if invalid_first {
                headers.reverse();
            }
            let route = serde_json::from_value(serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
                "metadata": {"name": "header-validation", "namespace": "default"},
                "spec": {"rules": [{"matches": [{"headers": headers}]}]}
            }))
            .unwrap();
            assert_eq!(validate_http_route_if_enabled(&route).is_empty(), !invalid_first);
        }
    }
}

#[test]
fn grpc_header_validation_ignores_only_later_equivalent_names() {
    for invalid in [
        serde_json::json!({"name": "x-mode", "type": "RegularExpression", "value": "["}),
        serde_json::json!({"name": "x-mode", "type": "Unknown", "value": "two"}),
    ] {
        for invalid_first in [false, true] {
            let mut headers = vec![serde_json::json!({"name": "X-Mode", "value": "one"}), invalid.clone()];
            if invalid_first {
                headers.reverse();
            }
            let route = serde_json::from_value(serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
                "metadata": {"name": "header-validation", "namespace": "default"},
                "spec": {"rules": [{"matches": [{"headers": headers}]}]}
            }))
            .unwrap();
            assert_eq!(validate_grpc_route_if_enabled(&route).is_empty(), !invalid_first);
        }
    }
}

/// Build a ReferenceGrant with a single From and a single To entry.
// Keeping the complete From/To tuple visible makes authorization fixtures auditable.
#[allow(clippy::too_many_arguments)]
fn make_grant(
    ns: &str,
    name: &str,
    from_ns: &str,
    from_group: &str,
    from_kind: &str,
    to_group: &str,
    to_kind: &str,
    to_name: Option<&str>,
) -> ReferenceGrant {
    ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some(ns.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: from_group.to_string(),
                kind: from_kind.to_string(),
                namespace: from_ns.to_string(),
            }],
            to: vec![ReferenceGrantTo {
                group: to_group.to_string(),
                kind: to_kind.to_string(),
                name: to_name.map(|s| s.to_string()),
            }],
        },
    }
}

fn store_with(grants: Vec<(String, ReferenceGrant)>) -> Arc<ReferenceGrantStore> {
    let store = ReferenceGrantStore::new();
    let mut map = HashMap::new();
    for (key, grant) in grants {
        map.insert(key, Arc::new(grant));
    }
    store.replace_all(map);
    Arc::new(store)
}

fn backend_ref(group: &str, kind: Option<&str>, name: &str, namespace: Option<&str>) -> BackendObjectReference {
    BackendObjectReference {
        group: Some(group.to_string()),
        kind: kind.map(|s| s.to_string()),
        name: name.to_string(),
        namespace: namespace.map(|s| s.to_string()),
        port: Some(80),
        ref_denied: None,
    }
}

fn cert_ref(group: Option<&str>, kind: Option<&str>, name: &str, namespace: Option<&str>) -> SecretObjectReference {
    SecretObjectReference {
        group: group.map(|s| s.to_string()),
        kind: kind.map(|s| s.to_string()),
        name: name.to_string(),
        namespace: namespace.map(|s| s.to_string()),
    }
}

#[derive(Debug)]
struct DeniedQuery {
    source_namespace: String,
    source_group: String,
    source_kind: String,
    target_namespace: String,
    target_group: String,
    target_kind: String,
    target_name: String,
}
fn denied_query(store: &ReferenceGrantStore, query: GrantQuery<'_>) -> Option<DeniedQuery> {
    (!store.check_query(query)).then(|| DeniedQuery {
        source_namespace: query.source().scope().namespace().unwrap().into(),
        source_group: query.source().group_kind().group().into(),
        source_kind: query.source().group_kind().kind().into(),
        target_namespace: query.target().scope().namespace().unwrap().into(),
        target_group: query.target().group_kind().group().into(),
        target_kind: query.target().group_kind().kind().into(),
        target_name: query.target().name().into(),
    })
}
fn route_owner(namespace: &str, refs: &[BackendObjectReference]) -> edgion_resources::HTTPRoute {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"route","namespace":namespace},
        "spec":{"rules":[{"backendRefs":refs}]}}))
    .unwrap()
}
fn denied_route_targets(
    store: &ReferenceGrantStore,
    namespace: &str,
    refs: &[BackendObjectReference],
) -> Vec<DeniedQuery> {
    let owner = route_owner(namespace, refs);
    let fields = HttpRouteBackendRefs::from_route(&owner).unwrap();
    fields
        .authored()
        .filter_map(|field| {
            let AuthoredRouteTarget::Terminal(terminal) = field.validated().unwrap() else {
                panic!("expected terminal")
            };
            denied_query(store, terminal.grant_query())
        })
        .collect()
}
fn denied_gateway_targets(
    store: &ReferenceGrantStore,
    namespace: &str,
    refs: &[SecretObjectReference],
) -> Vec<DeniedQuery> {
    let owner: edgion_resources::Gateway = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"gateway","namespace":namespace},"spec":{"gatewayClassName":"public",
        "listeners":[{"name":"https","port":443,"protocol":"HTTPS","tls":{"certificateRefs":refs}}]}
    }))
    .unwrap();
    let fields = GatewayTlsRefs::from_gateway(&owner);
    (0..refs.len())
        .filter_map(|index| {
            let field = fields.serving(0, index).unwrap();
            let token = field.token();
            denied_query(store, token.validated().unwrap().grant_query())
        })
        .collect()
}

// ---- Route backend ref tests --------------------------------------------

#[test]
fn test_route_backend_same_namespace_is_always_allowed() {
    // No grants in store, but same-namespace refs must always pass.
    let store = store_with(vec![]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-route"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(
        errors.is_empty(),
        "same-namespace ref must be allowed, got: {:?}",
        errors
    );
}

#[test]
fn test_route_backend_no_namespace_field_is_allowed() {
    // namespace=None means "same namespace as the route", which is always allowed.
    let store = store_with(vec![]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", None)];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(errors.is_empty());
}

#[test]
fn test_route_backend_cross_ns_allowed_by_exact_grant() {
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "any-svc", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(errors.is_empty(), "exact grant should allow ref, got: {:?}", errors);
}

#[test]
fn test_route_backend_cross_ns_rejected_when_no_store_entries() {
    // Empty store: every cross-namespace ref must be rejected.
    let store = store_with(vec![]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);

    assert_eq!(errors[0].source_namespace, "ns-route");
    assert_eq!(errors[0].target_namespace, "ns-target");
    assert_eq!(errors[0].target_name, "svc-a");
    assert_eq!(errors[0].source_group, "gateway.networking.k8s.io");
    assert_eq!(errors[0].target_group, "");
    assert_eq!(errors[0].source_kind, "HTTPRoute");
    assert_eq!(errors[0].target_kind, "Service");
}

#[test]
fn test_route_backend_rejected_when_from_namespace_mismatch() {
    // Grant trusts "ns-other", but route is in "ns-route".
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-other",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_route_backend_rejected_when_from_kind_mismatch() {
    // Grant trusts TCPRoute, but the caller is an HTTPRoute.
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "TCPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_route_backend_rejected_when_to_kind_mismatch() {
    // Grant trusts Secret, but the ref targets a Service.
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Secret",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_route_backend_rejected_when_to_name_mismatch() {
    // Grant restricts to "allowed-svc", but ref is for "other-svc".
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("allowed-svc"),
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "other-svc", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_route_backend_allowed_when_to_name_matches_explicit() {
    // Grant restricts to "allowed-svc"; ref targets exactly that name.
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("allowed-svc"),
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "allowed-svc", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(
        errors.is_empty(),
        "explicit name match should be allowed, got: {:?}",
        errors
    );
}

#[test]
fn test_route_backend_kind_default_is_service_when_none() {
    // backend.kind = None must be normalized to "Service".
    // Grant only allows Service, ref omits kind -> should still match.
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", None, "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(
        errors.is_empty(),
        "kind=None should default to Service, got: {:?}",
        errors
    );
}

#[test]
fn test_route_backend_multiple_grants_one_match_is_enough() {
    // Two grants in the same target namespace; the second one matches.
    let grant_no_match = make_grant(
        "ns-target",
        "g1",
        "ns-other",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let grant_match = make_grant(
        "ns-target",
        "g2",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![
        ("ns-target/g1".to_string(), grant_no_match),
        ("ns-target/g2".to_string(), grant_match),
    ]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert!(errors.is_empty());
}

#[test]
fn test_route_backend_mixed_refs_only_disallowed_reported() {
    // First ref: same ns (allowed). Second: cross-ns with grant (allowed).
    // Third: cross-ns to namespace with no grant (rejected).
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![
        backend_ref("", Some("Service"), "svc-local", Some("ns-route")),
        backend_ref("", Some("Service"), "svc-allowed", Some("ns-target")),
        backend_ref("", Some("Service"), "svc-blocked", Some("ns-forbidden")),
    ];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1, "only the third ref should fail, got: {:?}", errors);
    assert_eq!(errors[0].target_name, "svc-blocked");
    assert_eq!(errors[0].target_namespace, "ns-forbidden");
    assert_eq!(errors[0].source_group, "gateway.networking.k8s.io");
    assert_eq!(errors[0].target_group, "");
    assert_eq!(errors[0].source_kind, "HTTPRoute");
    assert_eq!(errors[0].target_kind, "Service");
    assert_eq!(errors[0].source_namespace, "ns-route");
}

#[test]
fn test_route_backend_grant_in_wrong_namespace_does_not_help() {
    // Grant lives in ns-other (not ns-target). Ref targets ns-target -> rejected.
    let grant = make_grant(
        "ns-other",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-other/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

// ---- Gateway certificate ref tests --------------------------------------

#[test]
fn test_gateway_cert_same_namespace_is_always_allowed() {
    let store = store_with(vec![]);

    let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-gw"))];
    let errors = denied_gateway_targets(&store, "ns-gw", &refs);

    assert!(errors.is_empty());
}

#[test]
fn test_gateway_cert_cross_ns_allowed_by_grant() {
    let grant = make_grant(
        "ns-certs",
        "g1",
        "ns-gw",
        "gateway.networking.k8s.io",
        "Gateway",
        "",
        "Secret",
        None,
    );
    let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);

    let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-certs"))];
    let errors = denied_gateway_targets(&store, "ns-gw", &refs);

    assert!(
        errors.is_empty(),
        "cross-ns cert ref should be allowed, got: {:?}",
        errors
    );
}

#[test]
fn test_gateway_cert_cross_ns_rejected_without_grant() {
    let store = store_with(vec![]);

    let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-certs"))];
    let errors = denied_gateway_targets(&store, "ns-gw", &refs);

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].source_kind, "Gateway");
    assert_eq!(errors[0].target_kind, "Secret");
    assert_eq!(errors[0].target_name, "tls-cert");
    assert_eq!(errors[0].target_namespace, "ns-certs");
    assert_eq!(errors[0].source_namespace, "ns-gw");
    assert_eq!(errors[0].source_group, "gateway.networking.k8s.io");
    assert_eq!(errors[0].target_group, "");
}

#[test]
fn test_gateway_cert_rejected_when_to_name_mismatch() {
    // Grant restricts to "allowed-cert", ref is for "other-cert".
    let grant = make_grant(
        "ns-certs",
        "g1",
        "ns-gw",
        "gateway.networking.k8s.io",
        "Gateway",
        "",
        "Secret",
        Some("allowed-cert"),
    );
    let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);

    let refs = vec![cert_ref(Some(""), Some("Secret"), "other-cert", Some("ns-certs"))];
    let errors = denied_gateway_targets(&store, "ns-gw", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_gateway_cert_group_and_kind_default_to_core_secret() {
    // SecretObjectReference: group=None and kind=None must default to ("", "Secret").
    let grant = make_grant(
        "ns-certs",
        "g1",
        "ns-gw",
        "gateway.networking.k8s.io",
        "Gateway",
        "",
        "Secret",
        None,
    );
    let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);

    let refs = vec![cert_ref(None, None, "tls-cert", Some("ns-certs"))];
    let errors = denied_gateway_targets(&store, "ns-gw", &refs);

    assert!(
        errors.is_empty(),
        "default group/kind should resolve to core/Secret, got: {:?}",
        errors
    );
}

#[test]
fn test_route_backend_grant_for_gateway_does_not_authorize_route() {
    // Grant only trusts Gateway as the from-kind. An HTTPRoute must not benefit.
    let grant = make_grant(
        "ns-target",
        "g1",
        "ns-route",
        "gateway.networking.k8s.io",
        "Gateway",
        "",
        "Service",
        None,
    );
    let store = store_with(vec![("ns-target/g1".to_string(), grant)]);

    let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
    let errors = denied_route_targets(&store, "ns-route", &refs);

    assert_eq!(errors.len(), 1);
}

#[test]
fn test_normalize_helpers_defaults() {
    for group in [None, Some("")] {
        let mut reference = backend_ref("", None, "service", None);
        reference.group = group.map(str::to_owned);
        let owner = route_owner("source", &[reference]);
        let fields = HttpRouteBackendRefs::from_route(&owner).unwrap();
        let AuthoredRouteTarget::Terminal(terminal) = fields.authored().next().unwrap().validated().unwrap() else {
            panic!("expected terminal")
        };
        assert_eq!(terminal.grant_query().target().group_kind().group(), "");
        assert_eq!(terminal.grant_query().target().group_kind().kind(), "Service");
    }
    for reference in [
        backend_ref("my.group", None, "service", None),
        backend_ref("", Some("Secret"), "service", None),
    ] {
        let owner = route_owner("source", &[reference]);
        let fields = HttpRouteBackendRefs::from_route(&owner).unwrap();
        assert!(matches!(
            fields.authored().next().unwrap().validated(),
            Err(ReferenceError::UnsupportedTarget)
        ));
    }
}

#[test]
fn dependency_is_visible_before_authorization_read() {
    let runtime = ControllerRuntime::default();
    let owner = ResourceRef::new(
        edgion_resources::ResourceKind::HTTPRoute,
        Some("app".to_string()),
        "route".to_string(),
    );

    let dependency_visible_during_read =
        read_after_registering_cross_ns_dependency(&runtime, "backend", &owner, || {
            runtime.cross_ns_ref_mgr.get_refs("backend").contains(&owner)
        });

    assert!(dependency_visible_during_read);
}
