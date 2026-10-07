use super::*;
use crate::backends::policy::backend_tls::BackendTLSPolicyStore;
use crate::plugins::PluginRuntime;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
};
use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
use edgion_resources::resources::BackendTLSPolicy;
use edgion_resources::HTTPBackendRef;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::HashMap;

fn compiled(name: &str, slot: usize, weight: Option<i32>) -> CompiledBackend {
    let schema = HTTPBackendRef {
        name: name.to_string(),
        namespace: Some("backend-ns".to_string()),
        port: Some(8080),
        weight,
        group: None,
        kind: Some("Service".to_string()),
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    };
    crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        "route-ns",
        slot,
        Arc::new(PluginRuntime::new()),
    )
}

fn logical(compiled: CompiledBackend) -> LogicalBackendHandle {
    let generation = Arc::from(vec![compiled]);
    LogicalBackendHandle::new(generation, 0).expect("single-slot fixture must be valid")
}

fn tls_policy(name: &str, hostname: &str) -> Arc<BackendTLSPolicy> {
    Arc::new(BackendTLSPolicy {
        metadata: ObjectMeta {
            namespace: Some("backend-ns".to_string()),
            name: Some(name.to_string()),
            uid: Some(format!("uid-{name}")),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: String::new(),
                kind: "Service".to_string(),
                name: "api".to_string(),
                section_name: None,
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
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
fn logical_handle_preserves_slot_identity_target_and_declared_weight() {
    let generation = Arc::from(vec![compiled("api", 0, Some(0))]);
    let logical = LogicalBackendHandle::new(generation, 0).unwrap();
    assert_eq!(logical.slot(), 0);
    assert_eq!(logical.declared_weight(), 0);
    assert!(logical.is_authorized());
    assert!(matches!(
        logical.target(),
        CompiledBackendTarget::Service(target)
            if target.namespace() == "backend-ns" && target.name() == "api"
    ));
    assert_eq!(logical.identity().namespace, "backend-ns");
    assert_eq!(logical.identity().name, "api");
    assert_eq!(
        logical.policy_target_key(),
        Some(&PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "backend-ns".to_string(),
            name: "api".to_string(),
        })
    );
}

#[test]
fn invalid_slot_cannot_materialize_selected_service() {
    let mut schema = compiled("api", 0, None).schema().clone();
    schema.group = Some("example.io".to_owned());
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );
    assert!(SelectedServiceBackend::new(logical(backend), BackendTlsPolicyLookup::Absent).is_none());
}

#[test]
fn denied_service_cannot_materialize_selected_service() {
    let mut schema = compiled("api", 0, Some(1)).schema().clone();
    schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
        target_namespace: "backend-ns".to_string(),
        target_name: "api".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );
    let logical = logical(backend);
    assert!(logical.policy_target_key().is_none());
    assert!(SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).is_none());
}

#[test]
fn logical_handle_rejects_invalid_generation_indexes() {
    let generation = Arc::from(vec![compiled("api", 0, None)]);
    assert!(LogicalBackendHandle::new(Arc::clone(&generation), 1).is_none());

    let mismatched = Arc::from(vec![compiled("api", 3, None)]);
    assert!(LogicalBackendHandle::new(mismatched, 0).is_none());
}

#[test]
fn compact_backend_state_stays_within_hot_path_budget() {
    assert_eq!(std::mem::size_of::<LogicalBackendHandle>(), 24);
    assert_eq!(std::mem::size_of::<SelectedServiceBackend>(), 40);
    assert_eq!(std::mem::size_of::<Option<SelectedHttpBackend>>(), 40);
}

#[test]
fn service_selection_and_runtime_projection_are_allocation_free() {
    let generation = Arc::from(vec![compiled("api", 0, None)]);
    let (selected, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let logical = LogicalBackendHandle::new(Arc::clone(&generation), 0).unwrap();
        let selected =
            SelectedHttpBackend::Service(SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap());
        let runtime = selected.plugin_runtime();
        drop(runtime);
        selected
    });

    assert_eq!(selected.slot(), 0);
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn logical_handle_pins_generation_and_plugin_runtime() {
    let old_runtime = Arc::new(PluginRuntime::new());
    let old_generation = Arc::from(vec![crate::routes::http::match_unit::compile_http_backend_fixture(
        HTTPBackendRef {
            name: "old".to_string(),
            namespace: Some("backend-ns".to_string()),
            port: Some(8080),
            weight: Some(1),
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "route-ns",
        0,
        Arc::clone(&old_runtime),
    )]);
    let logical = LogicalBackendHandle::new(Arc::clone(&old_generation), 0).unwrap();
    let selected =
        SelectedHttpBackend::Service(SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap());

    let replacement = Arc::from(vec![compiled("new", 0, None)]);
    drop(old_generation);

    assert_eq!(selected.name, "old");
    assert!(!Arc::ptr_eq(
        &selected.routed().unwrap().logical().generation,
        &replacement
    ));
    assert!(Arc::ptr_eq(&selected.plugin_runtime(), &old_runtime));
}

#[test]
fn selected_service_pins_tls_policy_without_mutating_compiled_schema() {
    let store = BackendTLSPolicyStore::new();
    let old_policy = tls_policy("old", "old.example.com");
    store.replace_all(HashMap::from([("backend-ns/old".to_string(), Arc::clone(&old_policy))]));

    let generation = Arc::from(vec![compiled("api", 0, None)]);
    let logical = LogicalBackendHandle::new(Arc::clone(&generation), 0).unwrap();
    let target = logical.policy_target_key().unwrap().clone();
    let captured = store.snapshot().lookup(&target, None);
    let selected = SelectedServiceBackend::new(logical, captured).unwrap();

    let new_policy = tls_policy("new", "new.example.com");
    store.replace_all(HashMap::from([("backend-ns/new".to_string(), Arc::clone(&new_policy))]));
    let current = store
        .snapshot()
        .lookup(&target, None)
        .effective()
        .expect("replacement policy must stay effective")
        .clone();

    let retained = selected
        .backend_tls_policy()
        .effective()
        .expect("captured policy must stay effective");
    assert!(Arc::ptr_eq(&retained.policy, &old_policy));
    assert!(Arc::ptr_eq(&current.policy, &new_policy));
    assert_eq!(retained.policy.spec.validation.hostname, "old.example.com");
    assert_eq!(current.policy.spec.validation.hostname, "new.example.com");
    assert!(selected.logical().compiled().schema().backend_tls_policy.is_none());
}

#[test]
fn selected_service_debug_does_not_expose_tls_policy_secrets() {
    let mut policy = (*tls_policy("private", "private.example.com")).clone();
    let secret = k8s_openapi::api::core::v1::Secret {
        string_data: Some(std::collections::BTreeMap::from([(
            "tls.key".to_string(),
            "PRIVATE-KEY-SENTINEL".to_string(),
        )])),
        ..Default::default()
    };
    policy.spec.resolved_ca_certificates = Some(vec![secret.clone()]);
    policy.spec.resolved_client_certificate = Some(secret);

    let selected = SelectedServiceBackend::new(
        logical(compiled("api", 0, None)),
        BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy)),
    )
    .expect("authorized Service fixture must be selectable");
    let debug = format!("{:?}", SelectedHttpBackend::Service(selected));

    assert!(debug.contains("backend_tls_policy: \"effective\""));
    assert!(!debug.contains("PRIVATE-KEY-SENTINEL"));
    assert!(!debug.contains("resolved_ca_certificates"));
    assert!(!debug.contains("resolved_client_certificate"));
}
fn l4_generation(
    family: &str,
    owner: serde_json::Value,
    backends: Vec<serde_json::Value>,
) -> Result<CompiledL4Backends, ReferenceError> {
    let source = serde_json::json!({
        "metadata": owner,
        "spec": {"rules": [{"backendRefs": backends}, {"backendRefs": [{"name": "ignored", "port": 99}]}]}
    });
    match family {
        "TCP" => CompiledL4Backends::from_tcp(&serde_json::from_value(source).unwrap()),
        "TLS" => CompiledL4Backends::from_tls(&serde_json::from_value(source).unwrap()),
        "UDP" => CompiledL4Backends::from_udp(&serde_json::from_value(source).unwrap()),
        _ => unreachable!(),
    }
}

#[test]
fn l4_actual_owners_default_namespace_and_reject_invalid_sources() {
    for family in ["TCP", "TLS", "UDP"] {
        for owner in [
            serde_json::json!({"name": "route"}),
            serde_json::json!({"name": "route", "namespace": "default"}),
        ] {
            let generation =
                l4_generation(family, owner, vec![serde_json::json!({"name": "svc", "port": 80})]).unwrap();
            let target = generation.select().unwrap().service().unwrap();
            assert_eq!(target.key(), "default/svc");
        }
        for owner in [
            serde_json::json!({}),
            serde_json::json!({"name": ""}),
            serde_json::json!({"name": "Bad/Name"}),
            serde_json::json!({"name": "route", "namespace": ""}),
            serde_json::json!({"name": "route", "namespace": "Bad/Namespace"}),
        ] {
            assert!(l4_generation(family, owner, vec![serde_json::json!({"name": "svc", "port": 80})]).is_err());
        }
    }
}

#[test]
fn l4_descriptor_gate_precedes_actual_resolver_entry_with_positive_controls() {
    use serde_json::json;
    for family in ["TCP", "TLS", "UDP"] {
        let owner = json!({"name": "route", "namespace": "apps"});
        let good = json!({"name": "svc", "port": 80});
        for (field, values) in [
            ("group", vec![json!("core"), json!("apps"), json!("Core")]),
            (
                "kind",
                vec![
                    json!(""),
                    json!("service"),
                    json!("ServiceImport"),
                    json!("ServiceExternalName"),
                    json!("EdgionBackend"),
                ],
            ),
            ("name", vec![json!(""), json!("Bad/Name")]),
            ("namespace", vec![json!(""), json!("Bad/Namespace")]),
            ("port", vec![json!(null), json!(0), json!(-1), json!(65536)]),
        ] {
            for value in values {
                let mut reference = good.clone();
                reference[field] = value;
                for denied in [false, true] {
                    let mut reference = reference.clone();
                    if denied {
                        reference["refDenied"] = json!({"targetNamespace": "other", "targetName": "svc", "reason": "NoMatchingReferenceGrant"});
                    }
                    let generation = l4_generation(family, owner.clone(), vec![reference]).unwrap();
                    let reads = std::cell::Cell::new(0);
                    let result = generation.select().unwrap().with_service(|_| {
                        reads.set(reads.get() + 1);
                        Ok::<_, ()>(())
                    });
                    assert_eq!(reads.get(), 0, "{family}: {field}, denied={denied}");
                    assert_eq!(
                        result,
                        Err(L4ResolveError::Rejected(if denied {
                            L4BackendRejection::Denied
                        } else {
                            L4BackendRejection::Invalid
                        }))
                    );
                }
            }
        }
        for port in [1, 65535] {
            for explicit in [false, true] {
                let mut reference = json!({"name": "svc", "port": port});
                if explicit {
                    reference["group"] = json!("");
                    reference["kind"] = json!("Service");
                    reference["namespace"] = json!("other");
                }
                let generation = l4_generation(family, owner.clone(), vec![reference]).unwrap();
                let reads = std::cell::Cell::new(0);
                let selected = generation.select().unwrap();
                let (target, ()) = selected
                    .with_service(|target| {
                        reads.set(reads.get() + 1);
                        assert_eq!(target.key(), if explicit { "other/svc" } else { "apps/svc" });
                        assert_eq!(
                            target.section(),
                            &ServiceSectionKey::Number(std::num::NonZeroU16::new(port as u16).unwrap())
                        );
                        Ok::<_, ()>(())
                    })
                    .unwrap();
                assert_eq!(reads.get(), 1);
                assert!(std::ptr::eq(target, selected.service().unwrap()));
                assert_eq!(
                    selected.with_service(|_| Err::<(), _>("live failure")),
                    Err(L4ResolveError::Unavailable("live failure"))
                );
            }
        }
    }
}

#[test]
fn l4_original_slots_weights_duplicates_and_allocation_free_selection() {
    use serde_json::json;
    for family in ["TCP", "TLS", "UDP"] {
        let generation = l4_generation(family, json!({"name": "route"}), vec![
            json!({"name": "same", "port": 80, "weight": 0}),
            json!({"name": "same", "port": 81, "weight": 2}),
            json!({"name": "invalid", "port": null, "weight": 1}),
            json!({"name": "denied", "port": 82, "weight": 1, "refDenied": {"targetNamespace": "other", "targetName": "denied"}}),
        ]).unwrap();
        assert_eq!(generation.slots.len(), 4);
        assert_eq!(
            generation
                .slots
                .iter()
                .map(|slot| (slot.slot, slot.weight))
                .collect::<Vec<_>>(),
            [(0, Some(0)), (1, Some(2)), (2, Some(1)), (3, Some(1))]
        );
        let mut counts = [0; 4];
        for _ in 0..4 {
            counts[generation.select().unwrap().slot()] += 1;
        }
        assert_eq!(counts, [0, 2, 1, 1]);
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            for _ in 0..32 {
                let selected = generation.select().unwrap();
                let _ = selected.with_service(|target| {
                    assert_eq!(target.key(), "default/same");
                    Ok::<_, ()>(())
                });
            }
        });
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

impl SelectedL4Backend<'_> {
    pub(crate) fn slot(self) -> usize {
        self.slot.slot
    }
}
