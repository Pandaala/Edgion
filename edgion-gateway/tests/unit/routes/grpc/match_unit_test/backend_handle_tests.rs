use super::*;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
};
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind;
use edgion_resources::resources::{BackendTLSPolicy, RefDenied};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

#[test]
fn malformed_backend_filter_remains_backend_scoped() {
    let rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({
        "backendRefs": [
            {
                "name": "invalid",
                "port": 8080,
                "filters": [{"type": "ExtensionRef"}]
            },
            {"name": "valid", "port": 8080}
        ]
    }))
    .unwrap();

    let runtime = crate::routes::grpc::match_unit::compile_grpc_fixture(&rule, "route-ns");

    assert!(runtime.backends[0].plugin_runtime.rejects_invalid_config());
    assert!(!runtime.backends[1].plugin_runtime.rejects_invalid_config());
    assert!(!runtime.plugin_runtime.rejects_invalid_config());
}

fn compiled(name: &str, weight: i32, runtime: Arc<PluginRuntime>) -> CompiledGrpcBackend {
    crate::routes::grpc::match_unit::compile_grpc_backend_fixture(
        GRPCBackendRef {
            name: name.to_string(),
            namespace: Some("backend-ns".to_string()),
            port: Some(8080),
            weight: Some(weight),
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "route-ns",
        runtime,
    )
}

fn compile_one_backend(schema: GRPCBackendRef, route_namespace: &str) -> Arc<[CompiledGrpcBackend]> {
    crate::routes::grpc::match_unit::compile_grpc_fixture(
        &GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: Some(vec![schema]),
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_allow_non_idempotent_retry: None,
        },
        route_namespace,
    )
    .backends
}

fn backend_ref(group: Option<&str>, kind: Option<&str>, namespace: Option<&str>, denied: bool) -> GRPCBackendRef {
    GRPCBackendRef {
        name: "api".to_string(),
        namespace: namespace.map(str::to_string),
        port: Some(8080),
        weight: Some(1),
        group: group.map(str::to_string),
        kind: kind.map(str::to_string),
        filters: None,
        backend_tls_policy: None,
        ref_denied: denied.then(|| RefDenied {
            target_namespace: namespace.unwrap_or("route-ns").to_string(),
            target_name: "api".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }),
    }
}

fn tls_policy(name: &str, hostname: &str) -> Arc<BackendTLSPolicy> {
    Arc::new(BackendTLSPolicy {
        metadata: ObjectMeta {
            namespace: Some("backend-ns".to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: None,
                },
            ),
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: String::new(),
                kind: "Service".to_string(),
                name: "api".to_string(),
                section_name: None,
            }],
            resolved_target_class: None,
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: hostname.to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: true,
        },
        status: None,
    })
}

#[test]
fn grpc_backend_handles_stay_within_hot_path_budget() {
    assert_eq!(std::mem::size_of::<GrpcBackendHandle>(), 24);
    assert_eq!(std::mem::size_of::<SelectedGrpcBackend>(), 40);
    assert_eq!(std::mem::size_of::<Option<SelectedGrpcBackend>>(), 40);
}

#[test]
fn handle_projects_exact_duplicate_slot_and_pins_generation_runtime() {
    let old_runtime = Arc::new(PluginRuntime::new());
    let generation = Arc::from(vec![
        compiled("api", 0, Arc::new(PluginRuntime::new())),
        compiled("api", 1, Arc::clone(&old_runtime)),
    ]);
    let logical = GrpcBackendHandle::new(Arc::clone(&generation), 1).unwrap();
    let selected = SelectedGrpcBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap();
    let replacement = Arc::from(vec![compiled("new", 1, Arc::new(PluginRuntime::new()))]);
    drop(generation);

    assert_eq!(selected.slot(), 1);
    assert_eq!(selected.name, "api");
    assert_eq!(selected.logical().policy_target_key().unwrap().name, "api");
    assert!(Arc::ptr_eq(&selected.plugin_runtime(), &old_runtime));
    assert!(!Arc::ptr_eq(&selected.logical.generation, &replacement));
    let replacement_handle = GrpcBackendHandle::new(replacement, 0).unwrap();
    assert_eq!(replacement_handle.policy_target_key().unwrap().name, "new");
}

#[test]
fn handle_rejects_out_of_range_index() {
    let generation = Arc::from(vec![compiled("api", 1, Arc::new(PluginRuntime::new()))]);
    assert!(GrpcBackendHandle::new(generation, 1).is_none());
}

#[test]
fn selector_returns_original_slot_and_filters_weight_zero_only_for_new_requests() {
    let generation = vec![
        compiled("sticky", 0, Arc::new(PluginRuntime::new())),
        compiled("active", 1, Arc::new(PluginRuntime::new())),
    ];
    let selector = GrpcRouteRuleUnit::build_backend_selector(&generation);

    for _ in 0..8 {
        assert_eq!(selector.select().unwrap(), 1);
    }
}

#[test]
fn compiled_policy_target_key_accepts_only_authorized_service_refs() {
    struct Case {
        label: &'static str,
        group: Option<&'static str>,
        kind: Option<&'static str>,
        namespace: Option<&'static str>,
        denied: bool,
        expected_namespace: Option<&'static str>,
    }

    let cases = [
        Case {
            label: "defaults",
            group: None,
            kind: None,
            namespace: None,
            denied: false,
            expected_namespace: Some("route-ns"),
        },
        Case {
            label: "empty-group-default-kind",
            group: Some(""),
            kind: None,
            namespace: None,
            denied: false,
            expected_namespace: Some("route-ns"),
        },
        Case {
            label: "core-group-default-kind",
            group: Some("core"),
            kind: None,
            namespace: None,
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "default-group-service-kind",
            group: None,
            kind: Some("Service"),
            namespace: None,
            denied: false,
            expected_namespace: Some("route-ns"),
        },
        Case {
            label: "empty-group-service-kind",
            group: Some(""),
            kind: Some("Service"),
            namespace: None,
            denied: false,
            expected_namespace: Some("route-ns"),
        },
        Case {
            label: "core-service-explicit-namespace",
            group: Some("core"),
            kind: Some("Service"),
            namespace: Some("backend-ns"),
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "empty-group-service-explicit-namespace",
            group: Some(""),
            kind: Some("Service"),
            namespace: Some("backend-ns"),
            denied: false,
            expected_namespace: Some("backend-ns"),
        },
        Case {
            label: "group-is-case-sensitive",
            group: Some("Core"),
            kind: Some("Service"),
            namespace: None,
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "kind-is-case-sensitive",
            group: Some(""),
            kind: Some("service"),
            namespace: None,
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "unsupported-kind",
            group: None,
            kind: Some("Other"),
            namespace: None,
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "grpc-route-delegation",
            group: Some("gateway.networking.k8s.io"),
            kind: Some("GRPCRoute"),
            namespace: Some("child-ns"),
            denied: false,
            expected_namespace: None,
        },
        Case {
            label: "denied-service",
            group: None,
            kind: Some("Service"),
            namespace: Some("backend-ns"),
            denied: true,
            expected_namespace: None,
        },
    ];

    for case in cases {
        let generation = compile_one_backend(
            backend_ref(case.group, case.kind, case.namespace, case.denied),
            "route-ns",
        );
        let handle = GrpcBackendHandle::new(generation, 0).unwrap();
        match (handle.policy_target_key(), case.expected_namespace) {
            (Some(target), Some(expected_namespace)) => {
                assert_eq!(target.target_kind, PolicyTargetKind::Service, "{}", case.label);
                assert_eq!(target.namespace, expected_namespace, "{}", case.label);
                assert_eq!(target.name, "api", "{}", case.label);
            }
            (None, None) => {}
            (actual, expected) => panic!("{}: target {actual:?}, expected namespace {expected:?}", case.label),
        }
    }
}

#[test]
fn handle_policy_target_projection_is_borrowed_stable_and_allocation_free() {
    let generation = compile_one_backend(backend_ref(None, None, None, false), "route-ns");
    let stored = generation[0].permitted_service().unwrap().policy_target() as *const PolicyTargetKey;

    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let handle = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
        let first = handle.policy_target_key().unwrap();
        let second = handle.policy_target_key().unwrap();
        assert!(std::ptr::eq(first, second));
        assert_eq!(first as *const PolicyTargetKey, stored);
        std::hint::black_box(first);
    });

    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn selected_backend_pins_tls_policy_and_selection_is_allocation_free() {
    let runtime = Arc::new(PluginRuntime::new());
    let generation = Arc::from(vec![compiled("api", 1, Arc::clone(&runtime))]);
    let old_policy = tls_policy("old", "old.example.com");
    let current_policy = tls_policy("new", "new.example.com");

    let captured = BackendTlsPolicyLookup::effective_from_policy(Arc::clone(&old_policy));
    let logical = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
    let selected = SelectedGrpcBackend::new(logical, captured).unwrap();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let logical = GrpcBackendHandle::new(Arc::clone(&generation), 0).unwrap();
        let selected = SelectedGrpcBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap();
        let projected_runtime = selected.plugin_runtime();
        drop(projected_runtime);
        selected
    });

    let retained = selected
        .backend_tls_policy()
        .effective()
        .expect("fixture policy must stay effective");
    assert!(Arc::ptr_eq(&retained.policy, &old_policy));
    assert!(!Arc::ptr_eq(&retained.policy, &current_policy));
    assert_eq!(retained.policy.spec.validation.hostname, "old.example.com");
    assert!(selected.logical().compiled().schema.backend_tls_policy.is_none());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn actual_owner_gates_factories_and_effective_backend_proofs() {
    use serde_json::json;
    let baseline =
        json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[{"name":"api","port":80,"weight":7}]}]}});
    for (field, value) in [
        ("name", json!(null)),
        ("name", json!("")),
        ("name", json!("bad/name")),
        ("namespace", json!("")),
        ("namespace", json!("UPPER")),
    ] {
        let mut input = baseline.clone();
        input["metadata"][field] = value;
        let route: GRPCRoute = serde_json::from_value(input).unwrap();
        let mut rule_reads = 0;
        let mut backend_reads = 0;
        assert!(GrpcRouteRuleUnit::compile_with_factories(
            &route,
            0,
            |_, _| {
                rule_reads += 1;
                Arc::new(PluginRuntime::new())
            },
            |_, _, _| {
                backend_reads += 1;
                Arc::new(PluginRuntime::new())
            }
        )
        .is_none());
        assert_eq!((rule_reads, backend_reads), (0, 0));
    }
    for (field, value) in [
        ("group", json!("core")),
        ("group", json!("example.io")),
        ("kind", json!("ServiceImport")),
        ("kind", json!("ServiceExternalName")),
        ("kind", json!("service")),
        ("name", json!("bad/name")),
        ("namespace", json!("")),
        ("port", json!(null)),
        ("port", json!(0)),
        ("port", json!(-1)),
        ("port", json!(65536)),
        ("refDenied", json!({"targetNamespace":"default","targetName":"api"})),
    ] {
        let mut input = baseline.clone();
        input["spec"]["rules"][0]["backendRefs"][0][field] = value;
        let route: GRPCRoute = serde_json::from_value(input).unwrap();
        let mut rule_reads = 0;
        let mut backend_reads = 0;
        let compiled = GrpcRouteRuleUnit::compile_with_factories(
            &route,
            0,
            |_, _| {
                rule_reads += 1;
                Arc::new(PluginRuntime::new())
            },
            |_, _, _| {
                backend_reads += 1;
                Arc::new(PluginRuntime::new())
            },
        )
        .unwrap();
        assert_eq!((rule_reads, backend_reads), (1, 0), "{field}");
        assert_eq!(compiled.backends.len(), 1);
        assert_eq!(compiled.backends[0].weight, Some(7));
        assert!(compiled.backends[0].permitted_service().is_none());
        assert_eq!(
            GrpcRouteRuleUnit::build_backend_selector(&compiled.backends)
                .select()
                .unwrap(),
            0
        );
    }
    let mut route: GRPCRoute = serde_json::from_value(baseline).unwrap();
    let mut calls = 0;
    let authored = GrpcRouteRuleUnit::compile_with_factories(
        &route,
        0,
        |owner, _| {
            assert_eq!(owner.metadata.namespace.as_deref().unwrap_or("default"), "default");
            Arc::new(PluginRuntime::new())
        },
        |owner, _, _| {
            assert_eq!(owner.metadata.namespace.as_deref().unwrap_or("default"), "default");
            calls += 1;
            Arc::new(PluginRuntime::new())
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(authored.backends[0].permitted_service().unwrap().key(), "default/api");
    route.spec.resolved_rules = Some(vec![serde_json::from_value(json!({"backendRefs":[
        {"name":"api","namespace":"child","port":1,"weight":0},
        {"name":"api","namespace":"child","port":65535,"weight":3},
        {"name":"denied","namespace":"child","port":80,"weight":5,"refDenied":{"targetNamespace":"child","targetName":"denied"}}
    ]})).unwrap()]);
    calls = 0;
    let effective = GrpcRouteRuleUnit::compile_with_factories(
        &route,
        0,
        |_, _| Arc::new(PluginRuntime::new()),
        |_, _, _| {
            calls += 1;
            Arc::new(PluginRuntime::new())
        },
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(
        effective
            .backends
            .iter()
            .enumerate()
            .map(|(slot, b)| (slot, b.weight))
            .collect::<Vec<_>>(),
        vec![(0, Some(0)), (1, Some(3)), (2, Some(5))]
    );
    assert_eq!(effective.backends[0].permitted_service().unwrap().key(), "child/api");
    assert_ne!(
        effective.backends[0].permitted_service().unwrap().section(),
        effective.backends[1].permitted_service().unwrap().section()
    );
    assert!(effective.backends[2].permitted_service().is_none());
    assert_eq!(authored.backends[0].permitted_service().unwrap().key(), "default/api");
}
