use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::BidirectionalRefManager;

fn btp_ref(ns: &str, name: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::BackendTLSPolicy, Some(ns.to_string()), name.to_string())
}

/// `btp_refs_for_service` returns the BackendTLSPolicies registered under a
/// Service key and nothing else. Routes and EBTPs register under the same
/// keys, so the kind filter is what keeps a route from requeuing itself.
#[test]
fn btp_refs_for_service_returns_only_backend_tls_policies() {
    let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");

    let policy = btp_ref("ns", "btp-a");
    let other_policy = btp_ref("ns", "btp-b");
    mgr.add_ref("ns/svc".to_string(), policy.clone());
    mgr.add_ref("ns/svc".to_string(), other_policy.clone());
    mgr.add_ref(
        "ns/svc".to_string(),
        ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "route".to_string()),
    );
    mgr.add_ref(
        "ns/svc".to_string(),
        ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            Some("ns".to_string()),
            "ebtp".to_string(),
        ),
    );

    let mut got: Vec<String> = btp_refs_for_service(&mgr, "ns/svc")
        .iter()
        .map(|r| r.resource_key())
        .collect();
    got.sort();

    assert_eq!(got, vec!["ns/btp-a", "ns/btp-b"]);
}

/// A Service with no BTP targets yields no refs, and a same-prefix Service
/// name must not over-match.
#[test]
fn btp_refs_for_service_no_match_is_empty() {
    let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");
    mgr.add_ref("ns/svc".to_string(), btp_ref("ns", "btp-a"));

    assert!(btp_refs_for_service(&mgr, "ns/nomatch").is_empty());
    assert!(btp_refs_for_service(&mgr, "ns/svc-other").is_empty());
}

/// The shared ancestor resolver walks Service -> Routes (service_ref_mgr)
/// -> Gateways (gateway_route_index), maps each gateway to the target
/// Service keys that reach it, and ignores non-route refs (policies also
/// register under Service keys).
#[test]
fn resolve_policy_ancestor_gateways_maps_gateways_to_services() {
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::ResourceKind;
    use std::sync::Arc;

    let rt = Arc::new(ControllerRuntime::default());
    let ctx = crate::conf_mgr::sync_runtime::resource_processor::HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        5,
        20,
        rt.clone(),
    );

    let gw_parent = |name: &str| ParentReference {
        group: Some("gateway.networking.k8s.io".to_string()),
        kind: Some("Gateway".to_string()),
        namespace: Some("ns".to_string()),
        name: name.to_string(),
        section_name: None,
        port: None,
    };

    // svc-a reaches gw-a via route-a; svc-b reaches gw-b via route-b.
    for (svc, route, gw) in [("svc-a", "route-a", "gw-a"), ("svc-b", "route-b", "gw-b")] {
        rt.service_ref_mgr.add_ref(
            format!("ns/{svc}"),
            ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), route.to_string()),
        );
        rt.gateway_route_index
            .update_route(ResourceKind::HTTPRoute, &format!("ns/{route}"), &[gw_parent(gw)], "ns");
    }
    // A policy ref registered under the same Service key must be ignored.
    rt.service_ref_mgr.add_ref(
        "ns/svc-a".to_string(),
        ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            Some("ns".to_string()),
            "some-policy".to_string(),
        ),
    );

    let map = resolve_policy_ancestor_gateways("ns", &["svc-a", "svc-b"], "ns/p", &ctx);
    assert_eq!(map.len(), 2);
    assert!(map["ns/gw-a"].contains("ns/svc-a"));
    assert!(!map["ns/gw-a"].contains("ns/svc-b"));
    assert!(map["ns/gw-b"].contains("ns/svc-b"));

    // A target Service nobody routes to contributes no ancestor.
    let map = resolve_policy_ancestor_gateways("ns", &["svc-unrouted"], "ns/p", &ctx);
    assert!(map.is_empty());
}

#[test]
fn typed_ancestor_resolver_separates_same_named_service_and_backend() {
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::common::ParentReference;
    use std::sync::Arc;

    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = crate::conf_mgr::sync_runtime::resource_processor::HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        5,
        20,
        runtime.clone(),
    );
    let service_route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "service".to_string());
    let ai_route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "ai".to_string());
    let service_target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "ns".to_string(),
        name: "shared".to_string(),
    };
    let ai_target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "ns".to_string(),
        name: "shared".to_string(),
    };
    runtime
        .service_ref_mgr
        .add_ref("ns/shared".to_string(), service_route.clone());
    runtime.backend_ref_mgr.add_ref(ai_target.to_string(), ai_route.clone());

    let parent = |name: &str| ParentReference {
        group: Some("gateway.networking.k8s.io".to_string()),
        kind: Some("Gateway".to_string()),
        namespace: Some("ns".to_string()),
        name: name.to_string(),
        section_name: None,
        port: None,
    };
    runtime.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        &service_route.resource_key(),
        &[parent("service-gw")],
        "ns",
    );
    runtime.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        &ai_route.resource_key(),
        &[parent("ai-gw")],
        "ns",
    );

    let map =
        resolve_policy_ancestor_gateways_for_targets(&[service_target.clone(), ai_target.clone()], "ns/policy", &ctx);
    assert_eq!(map["ns/service-gw"], BTreeSet::from([service_target]));
    assert_eq!(map["ns/ai-gw"], BTreeSet::from([ai_target]));
}

/// `ebtp_refs_for_service` returns only EdgionBackendTrafficPolicy refs
/// registered under a Service key. Routes register under the same keys and
/// MUST be excluded — requeuing them from a route-change path would make a
/// route requeue itself (an infinite route->route loop).
#[test]
fn ebtp_refs_for_service_filters_out_route_refs() {
    let mgr = BidirectionalRefManager::with_component("test_service_ref");
    let policy = ResourceRef::new(
        ResourceKind::EdgionBackendTrafficPolicy,
        Some("ns".to_string()),
        "p1".to_string(),
    );
    let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "r1".to_string());
    mgr.add_ref("ns/svc".to_string(), policy.clone());
    mgr.add_ref("ns/svc".to_string(), route);

    let refs = ebtp_refs_for_service(&mgr, "ns/svc");
    assert_eq!(refs, vec![policy]);
    assert!(ebtp_refs_for_service(&mgr, "ns/other").is_empty());
}

#[test]
fn remaining_raw_route_owners_use_semantic_gateway_ancestry_without_mirror_aliases() {
    use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::common::ParentReference;
    use std::sync::Arc;
    for kind in [
        ResourceKind::GRPCRoute,
        ResourceKind::TCPRoute,
        ResourceKind::TLSRoute,
        ResourceKind::UDPRoute,
    ] {
        for namespace in [None, Some("default".to_string())] {
            let runtime = Arc::new(ControllerRuntime::default());
            let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
            let owner = ResourceRef::new(kind, namespace, "route".into());
            let parent: ParentReference = serde_json::from_value(serde_json::json!({"name":"gateway"})).unwrap();
            runtime
                .gateway_route_index
                .update_route(kind, "default/route", &[parent], "default");
            runtime.service_ref_mgr.add_ref("default/service".into(), owner.clone());
            runtime
                .service_ref_mgr
                .add_ref(mirror_service_key("default", "mirror"), owner.clone());
            let map = resolve_policy_ancestor_gateways("default", &["service", "mirror"], "default/policy", &ctx);
            assert_eq!(map.len(), 1);
            assert_eq!(map["default/gateway"], BTreeSet::from(["default/service".to_string()]));
            assert_eq!(
                crate::conf_mgr::sync_runtime::resource_processor::service_ref::forwarding_service_dependencies(
                    &runtime.service_ref_mgr,
                    &owner.key()
                ),
                vec!["default/service"]
            );
        }
    }
}
