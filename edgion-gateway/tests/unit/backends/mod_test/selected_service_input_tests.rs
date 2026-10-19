use super::*;
use crate::plugins::PluginRuntime;
use edgion_resources::HTTPBackendRef;

fn selected_service(name: &str) -> SelectedServiceBackend {
    let compiled = crate::routes::http::match_unit::compile_http_backend_fixture(
        HTTPBackendRef {
            name: name.to_string(),
            namespace: Some("tenant".to_string()),
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
        Arc::new(PluginRuntime::new()),
    );
    let generation = Arc::from(vec![compiled]);
    let logical = LogicalBackendHandle::new(generation, 0).unwrap();
    SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap()
}

fn backend_context(service_key: &str) -> crate::BackendContext {
    crate::BackendContext {
        source: None,
        rsc: None,
        service_key: Some(service_key.to_string()),
        service_kind: Some(ServiceBackendKind::Service),
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: Some(0),
        effective_policy_captured: false,
        effective_policy: None,
    }
}

#[test]
fn selected_identity_is_the_http_service_resolution_source() {
    let selected = selected_service("service-a");
    let context = backend_context("tenant/service-a");

    let input = selected_http_service_input(&selected, Some(&context)).unwrap();

    assert_eq!(
        input.service.section(),
        &crate::backends::discovery::ServiceSectionKey::Number(std::num::NonZeroU16::new(8080).unwrap())
    );
    assert_eq!(input.service.key(), "tenant/service-a");
    assert_eq!(
        input.service.policy_target().target_kind,
        edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind::Service
    );
}

#[test]
fn mismatched_context_cannot_redirect_selected_service() {
    let selected = selected_service("service-a");
    let context = backend_context("tenant/service-b");

    assert_eq!(
        selected_http_service_input(&selected, Some(&context)).unwrap_err(),
        EdgionErrorCode::UpstreamNotBackendRefs
    );
}

#[test]
fn grpc_context_must_match_the_selected_service_identity_exactly() {
    use ServiceBackendKind::{Service, ServiceImport};

    let selected = selected_service("service-a");
    let target = selected.logical().service_target().unwrap();
    assert!(!service_context_matches(None, target));
    for (case, key, kind, expected) in [
        ("exact", Some("tenant/service-a"), Some(Service), true),
        ("missing key", None, Some(Service), false),
        ("missing kind", Some("tenant/service-a"), None, false),
        ("wrong kind", Some("tenant/service-a"), Some(ServiceImport), false),
        ("wrong namespace", Some("other/service-a"), Some(Service), false),
        ("wrong name", Some("tenant/service-b"), Some(Service), false),
        ("missing separator", Some("tenant"), Some(Service), false),
        ("extra separator", Some("tenant/service-a/extra"), Some(Service), false),
        ("empty namespace", Some("/service-a"), Some(Service), false),
        ("empty name", Some("tenant/"), Some(Service), false),
        ("empty segments", Some("/"), Some(Service), false),
        ("empty key", Some(""), Some(Service), false),
    ] {
        let mut context = backend_context("tenant/service-a");
        context.service_key = key.map(str::to_string);
        context.service_kind = kind;
        assert_eq!(service_context_matches(Some(&context), target), expected, "{case}");
    }
}

#[test]
fn selected_service_identity_and_final_input_borrow_without_allocations() {
    let selected = selected_service("service-a");
    let context = backend_context("tenant/service-a");
    let stored = selected.logical().service_target().unwrap();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        for _ in 0..16 {
            let input = selected_http_service_input(&selected, Some(&context)).unwrap();
            assert!(std::ptr::eq(input.service, stored));
            assert!(std::ptr::eq(
                selected.logical().policy_target_key().unwrap(),
                stored.policy_target()
            ));
            std::hint::black_box(input);
        }
    });
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

pub(super) fn named_captured_policy() -> Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy> {
    use crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy;
    use edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission;
    use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
    use edgion_resources::resources::lb_policy::ConsistentHashOn;

    Arc::new(EffectiveBackendTrafficPolicy {
        ancestor_admission: Some(PreparedPolicyAncestorAdmission::Unrestricted),
        resolved_target_class: ResolvedPolicyTargetClass::Service,
        winning_policy_key: "default/captured".to_owned(),
        load_balancer: Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
            "x-routing-key".into(),
        ))),
        degrade_threshold_override: None,
        upstream_authority: None,
        upstream_authority_config: None,
        retry_budget: None,
        retry_constraint: None,
        circuit_breaker_limit: None,
        connect_timeout: None,
        outlier_detection: None,
        ai_credential_outlier: None,
        logical_circuit_breaker: None,
    })
}

#[test]
fn selected_input_does_not_copy_named_captured_policy() {
    let selected = selected_service("service-a");
    let mut context = backend_context("tenant/service-a");
    let policy = named_captured_policy();
    context.effective_policy = Some(Arc::clone(&policy));
    context.effective_policy_captured = true;
    let (input, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        selected_http_service_input(&selected, Some(&context)).unwrap()
    });
    assert_eq!(input.service.key(), "tenant/service-a");
    assert!(Arc::ptr_eq(context.effective_policy.as_ref().unwrap(), &policy));
    assert_eq!(captured_lb_policy(Some(&context)), policy.load_balancer);
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}
