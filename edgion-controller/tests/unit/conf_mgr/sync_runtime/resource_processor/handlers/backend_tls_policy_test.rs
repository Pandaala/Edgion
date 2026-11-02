use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation, WellKnownCACertificates,
};
use edgion_resources::ResourceKind;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::jiff::Timestamp;
use kube::api::ObjectMeta;
use std::sync::Arc;

// -------- should_requeue_siblings (pure decision) --------

#[test]
fn should_requeue_when_target_set_changes() {
    assert!(should_requeue_siblings(
        &["ns/svc-a".to_string()],
        &["ns/svc-b".to_string()],
        Some(dt("2024-01-01T00:00:00+00:00")),
        Some(dt("2024-01-01T00:00:00+00:00")),
    ));
}

#[test]
fn should_requeue_when_timestamp_changes_with_same_targets() {
    // BTP delete+recreated within one PollWatcher tick — same name and
    // targets, but a new creation_timestamp must trigger sibling re-evaluation.
    let targets = ["ns/my-svc".to_string()];
    assert!(should_requeue_siblings(
        &targets,
        &targets,
        Some(dt("2024-01-01T00:00:00+00:00")),
        Some(dt("2024-06-01T00:00:00+00:00")),
    ));
}

#[test]
fn should_not_requeue_on_noop_update() {
    // Steady state: identical targets, identical timestamp (e.g. a
    // resourceVersion bump from a status write). Skipping requeue is
    // what prevents the A→B→A→B ping-pong between conflicting policies.
    let targets = ["ns/my-svc".to_string()];
    assert!(!should_requeue_siblings(
        &targets,
        &targets,
        Some(dt("2024-01-01T00:00:00+00:00")),
        Some(dt("2024-01-01T00:00:00+00:00")),
    ));
}

#[test]
fn should_requeue_when_old_timestamp_was_missing() {
    // First registration after a controller restart: old timestamp is
    // None, new timestamp is Some. Treat as a precedence-relevant change.
    let targets = ["ns/my-svc".to_string()];
    assert!(should_requeue_siblings(
        &targets,
        &targets,
        None,
        Some(dt("2024-01-01T00:00:00+00:00")),
    ));
}

// -------- parse_policy delete+recreate path (state-level) --------

fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

/// Parse an RFC3339 literal into the `Timestamp` the precedence store holds.
fn dt(s: &str) -> Timestamp {
    s.parse().expect("valid RFC3339")
}

fn make_btp(ns: &str, name: &str, target_svc: &str, creation_ts: &str) -> BackendTLSPolicy {
    let ts: Timestamp = creation_ts.parse().expect("valid RFC3339 timestamp in test");
    BackendTLSPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            creation_timestamp: Some(Time(ts)),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: None,
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: "".to_string(),
                kind: "Service".to_string(),
                name: target_svc.to_string(),
                section_name: None,
            }],
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: "backend.example.com".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: Some(WellKnownCACertificates::System),
            },
            options: None,
            resolved_target_class: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: false,
        },
        status: None,
    }
}

fn install_backend_processor(
    runtime: &Arc<ControllerRuntime>,
    backend: EdgionBackend,
) -> (
    Arc<crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor<EdgionBackend>>,
    Arc<crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore<EdgionBackend>>,
) {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;

    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let key = format!(
        "{}/{}",
        backend.metadata.namespace.as_deref().unwrap_or("default"),
        backend.metadata.name.as_deref().unwrap_or_default()
    );
    let raw = Arc::new(crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore::new());
    raw.upsert(key, Arc::new(backend.clone()));
    processor.set_raw_store(raw.clone());
    processor.save(backend, None);
    runtime.processor_registry.register(processor.clone());
    (processor, raw)
}

#[tokio::test]
async fn ai_target_publishes_typed_marker_and_dependency() {
    let (ctx, runtime) = make_ctx();
    let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackend",
        "metadata": {"name": "models", "namespace": "ns"},
        "spec": {"ai": {
            "provider": "OpenAI",
            "credentialPool": {"credentials": [{
                "name": "primary", "secretRef": {"name": "provider-key"}
            }]},
            "models": [{"name": "model-a"}]
        }}
    }))
    .unwrap();
    install_backend_processor(&runtime, backend);
    let handler = BackendTlsPolicyHandler::default();
    let mut policy = make_btp("ns", "ai-tls", "models", "2026-01-01T00:00:00+00:00");
    policy.spec.target_refs[0].group = "edgion.io".to_string();
    policy.spec.target_refs[0].kind = "EdgionBackend".to_string();

    let ProcessResult::Continue(parsed) = handler.parse_policy(policy, &ctx).await else {
        panic!("policy should continue");
    };
    assert_eq!(parsed.spec.resolved_target_class, Some(ResolvedPolicyTargetClass::Ai));
    let target = parsed.spec.target_refs[0].canonical_key("ns").unwrap();
    assert_eq!(
        runtime.backend_ref_mgr.get_dependencies("BackendTLSPolicy/ns/ai-tls"),
        vec![target.to_string()]
    );
    assert_eq!(
        runtime
            .backend_tls_policy
            .target_mgr()
            .get_dependencies("BackendTLSPolicy/ns/ai-tls"),
        vec![target.to_string()]
    );
}

#[test]
fn ai_target_rejects_section_and_wrong_variant_identity() {
    let (ctx, _) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();
    let mut policy = make_btp("ns", "ai-tls", "models", "2026-01-01T00:00:00+00:00");
    policy.spec.target_refs[0].group = "edgion.io".to_string();
    policy.spec.target_refs[0].kind = "EdgionBackend".to_string();
    policy.spec.target_refs[0].section_name = Some("unused".to_string());
    assert!(handler
        .validate(&policy, &ctx)
        .iter()
        .any(|error| error.contains("sectionName is not supported")));

    policy.spec.target_refs[0].section_name = None;
    policy.spec.target_refs[0].group = "example.io".to_string();
    assert!(handler
        .validate(&policy, &ctx)
        .iter()
        .any(|error| error == "unsupported reference target"));
}

#[test]
fn duplicate_direct_targets_are_rejected() {
    let (ctx, _) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();
    let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
    policy.spec.validation.well_known_ca_certificates = None;
    policy.spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: "Secret".to_string(),
        name: "ca".to_string(),
    }]);
    policy.spec.target_refs.push(policy.spec.target_refs[0].clone());
    let errors = handler.validate(&policy, &ctx);
    assert!(errors.iter().any(|error| error.contains("exactly one")));
}

#[tokio::test]
async fn unsupported_well_known_ca_gets_invalid_status_and_no_runtime_target() {
    let (ctx, runtime) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();
    runtime.service_ref_mgr.add_ref(
        "ns/svc".to_string(),
        ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "route".to_string()),
    );
    runtime.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        "ns/route",
        &[ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some("ns".to_string()),
            name: "gateway".to_string(),
            section_name: None,
            port: None,
        }],
        "ns",
    );
    let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
    policy.spec.validation.well_known_ca_certificates =
        Some(WellKnownCACertificates::Other("example.com/Custom".to_string()));
    let errors = handler.validate(&policy, &ctx);
    assert!(errors.iter().any(|error| error.contains("only System")));
    let ProcessResult::Continue(mut rejected) = handler.parse(policy, &ctx).await else {
        panic!("unsupported CA name must remain available for status");
    };
    assert!(!rejected.spec.use_system_ca);
    assert!(rejected.spec.resolved_target_class.is_none());
    handler.update_status(&mut rejected, &ctx, &errors);
    assert!(rejected.status.as_ref().unwrap().ancestors.iter().any(|ancestor| {
        ancestor.conditions.iter().any(|condition| {
            condition.type_ == "Accepted" && condition.status == "False" && condition.message.contains("only System")
        })
    }));

    rejected.spec.validation.well_known_ca_certificates = Some(WellKnownCACertificates::System);
    let errors = handler.validate(&rejected, &ctx);
    assert!(errors.is_empty());
    let ProcessResult::Continue(mut corrected) = handler.parse(rejected, &ctx).await else {
        panic!("corrected CA name must parse");
    };
    assert!(corrected.spec.use_system_ca);
    handler.update_status(&mut corrected, &ctx, &errors);
    assert!(corrected.status.as_ref().unwrap().ancestors.iter().any(|ancestor| {
        ancestor
            .conditions
            .iter()
            .any(|condition| condition.type_ == "Accepted" && condition.status == "True")
    }));
}

#[test]
fn status_rebuild_preserves_foreign_controller_ancestors() {
    let (ctx, _) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();
    let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
    let foreign = PolicyAncestorStatus {
        ancestor_ref: ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some("ns".to_string()),
            name: "foreign".to_string(),
            section_name: None,
            port: None,
        },
        controller_name: "example.io/other-controller".to_string(),
        conditions: Vec::new(),
    };
    policy.status = Some(BackendTLSPolicyStatus {
        ancestors: vec![foreign.clone()],
    });

    handler.update_status(&mut policy, &ctx, &[]);

    assert!(policy
        .status
        .unwrap()
        .ancestors
        .iter()
        .any(|ancestor| ancestor.controller_name == foreign.controller_name));
}

/// Regression test for the precedence-staleness bug.
///
/// Setup: two BTPs (winner, sibling) both target the same Service.
/// `winner` has the older creation_timestamp, so it wins; `sibling` is
/// `Conflicted`.
///
/// Then `winner` is delete+recreated within one PollWatcher tick — same
/// name, same targets, but a NEWER creation_timestamp than `sibling`.
/// After processing, `sibling` should now win, and `parse_policy` must
/// have observed the precedence flip via the `should_requeue_siblings`
/// helper so siblings get requeued.
///
/// Without the fix, the timestamp-only change is ignored and `sibling`
/// keeps a stale `Conflicted` status until the next unrelated event.
///
/// Scope: this asserts on the **state-level outcome** (precedence in the
/// `BackendTLSPolicyState` flips correctly).  It does not directly observe
/// `ctx.requeue` invocation, because the test runtime has no processor
/// registered for `BackendTLSPolicy` (so requeue is a no-op log).  The
/// `should_requeue_siblings` helper is independently covered by the four
/// pure-decision tests above.
#[tokio::test]
async fn parse_policy_requeues_siblings_when_winner_replaced_with_newer_timestamp() {
    let (ctx, runtime) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();

    // 1. Initial state: winner is OLDER, sibling is YOUNGER.
    let winner = make_btp("ns", "winner", "my-svc", "2024-01-01T00:00:00+00:00");
    let sibling = make_btp("ns", "sibling", "my-svc", "2024-06-01T00:00:00+00:00");

    let _ = handler.parse_policy(winner, &ctx).await;
    let _ = handler.parse_policy(sibling, &ctx).await;

    let target_key = make_target_conflict_key(&service_target_key("ns", "my-svc"), None);
    assert!(
        runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
        "older winner should be the precedence winner before the recreate"
    );
    assert!(
        !runtime.backend_tls_policy.is_winner("ns/sibling", &target_key),
        "younger sibling should be conflicted before the recreate"
    );

    // 2. Sanity: a no-op reparse of the winner does NOT change precedence
    //    state and should be detected as not-requeue-worthy by the helper.
    //    (Verified by re-checking is_winner — same result.)
    let winner_noop = make_btp("ns", "winner", "my-svc", "2024-01-01T00:00:00+00:00");
    let _ = handler.parse_policy(winner_noop, &ctx).await;
    assert!(
        runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
        "noop reparse must not flip precedence"
    );

    // 3. Bug scenario: winner is deleted and recreated within one tick
    //    with the SAME name and SAME targets but a NEWER timestamp than
    //    `sibling`. After parse, the timestamp store reflects the new
    //    timestamp, and `sibling` should now win on creation_timestamp
    //    ordering.
    let winner_recreated = make_btp("ns", "winner", "my-svc", "2024-12-01T00:00:00+00:00");
    let old_ts_before_recreate = runtime.backend_tls_policy.timestamp_of("ns/winner");
    let _ = handler.parse_policy(winner_recreated, &ctx).await;
    let new_ts_after_recreate = runtime.backend_tls_policy.timestamp_of("ns/winner");

    assert_ne!(
        old_ts_before_recreate, new_ts_after_recreate,
        "timestamp store must reflect the recreated BTP's new creation_timestamp"
    );

    // The recreated winner is now YOUNGER than sibling, so sibling must
    // now win the target on precedence — confirming the precedence flip
    // that should_requeue_siblings detects via the timestamp delta.
    assert!(
        runtime.backend_tls_policy.is_winner("ns/sibling", &target_key),
        "sibling should now win after the winner was recreated with a newer timestamp"
    );
    assert!(
        !runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
        "recreated winner is now the younger policy and should be conflicted"
    );

    // 4. Direct check on the decision helper: with same target set but a
    //    different timestamp, siblings MUST be requeued. This is the
    //    specific code path that was missing before the fix.
    let targets = vec![target_key.clone()];
    assert!(should_requeue_siblings(
        &targets,
        &targets,
        old_ts_before_recreate,
        new_ts_after_recreate,
    ));
}

/// Locks the coalescing guarantee for BackendTLSPolicy -> EdgionBackendTrafficPolicy:
/// a delete->recreate that retargets the policy to a different Service must
/// requeue the EBTPs bound to the DROPPED target Service, even though
/// `on_delete_policy` never runs. `on_change_policy`'s `requeue_dependent_ebtps`
/// only sees the current targetRefs, so the dropped target's EBTPs rely on the
/// return-delta requeue added to `parse_policy`.
#[tokio::test]
async fn parse_policy_requeues_dropped_target_ebtps_on_retarget() {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;

    struct EbtpNoop;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionBackendTrafficPolicy> for EbtpNoop {}

    let runtime = Arc::new(ControllerRuntime::default());

    // Register an EBTP processor so ctx.requeue("EdgionBackendTrafficPolicy", ...)
    // has an observable queue.
    let ebtp_processor = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        100,
        Arc::new(EbtpNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(ebtp_processor.clone());

    // An EBTP depends on ns/svc-a, registered under its typed Service key.
    let ebtp_ref = ResourceRef::new(
        ResourceKind::EdgionBackendTrafficPolicy,
        Some("ns".to_string()),
        "ebtp-a".to_string(),
    );
    runtime
        .edgion_backend_traffic_policy
        .target_mgr()
        .add_ref(service_target_key("ns", "svc-a").to_string(), ebtp_ref.clone());

    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let handler = BackendTlsPolicyHandler::default();

    // Generation 1: BTP targets svc-a.
    let _ = handler
        .parse_policy(make_btp("ns", "btp1", "svc-a", "2024-01-01T00:00:00+00:00"), &ctx)
        .await;
    while tokio::time::timeout(
        std::time::Duration::from_millis(500),
        ebtp_processor.workqueue().dequeue(),
    )
    .await
    .is_ok()
    {}

    // Coalesced delete->recreate: the worker only sees generation 2, now
    // targeting svc-b (svc-a dropped). on_delete_policy never runs. The EBTP
    // on the dropped svc-a must still be requeued.
    let _ = handler
        .parse_policy(make_btp("ns", "btp1", "svc-b", "2024-02-01T00:00:00+00:00"), &ctx)
        .await;

    let mut seen = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !seen.contains("ns/ebtp-a") {
        if let Ok(Some(item)) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            ebtp_processor.workqueue().dequeue(),
        )
        .await
        {
            seen.insert(item.key.clone());
            ebtp_processor.workqueue().done(&item.key);
        }
    }
    assert!(
        seen.contains("ns/ebtp-a"),
        "EBTP on dropped target svc-a was not requeued; seen: {:?}",
        seen
    );
}

#[test]
fn get_siblings_returns_other_policies_targeting_same_service() {
    let runtime = ControllerRuntime::default();
    let target_key = make_target_conflict_key(&service_target_key("ns", "my-svc"), None);

    let winner = ResourceRef::new(
        ResourceKind::BackendTLSPolicy,
        Some("ns".to_string()),
        "winner".to_string(),
    );
    let sibling = ResourceRef::new(
        ResourceKind::BackendTLSPolicy,
        Some("ns".to_string()),
        "sibling".to_string(),
    );

    runtime
        .backend_tls_policy
        .target_mgr()
        .add_ref(target_key.clone(), winner.clone());
    runtime
        .backend_tls_policy
        .target_mgr()
        .add_ref(target_key, sibling.clone());

    let siblings_of_winner = runtime.backend_tls_policy.get_siblings(&winner);
    assert_eq!(siblings_of_winner, vec![sibling]);
}

// -------- Critical 2 regression: BackendTLSPolicy change/delete requeues the
// -------- dependent EdgionBackendTrafficPolicy --------

/// Build a minimal, spec-locally-valid EdgionBackendTrafficPolicy targeting
/// `namespace/service` and register it into the target-conflict index via
/// `EdgionBackendTrafficPolicyHandler::parse` — mirroring what a real EBTP reconcile does
/// (`edgion_backend_traffic_policy.rs::parse` registers a valid spec under
/// the typed `PolicyTargetKey` identity).
async fn register_ebtp_target(namespace: &str, name: &str, service: &str, ctx: &HandlerContext) {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler;
    use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
    use serde_json::json;

    let ebtp: EdgionBackendTrafficPolicy = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "targetRefs": [ { "kind": "Service", "name": service } ] },
    }))
    .expect("valid EdgionBackendTrafficPolicy JSON");
    let _ = EdgionBackendTrafficPolicyHandler::new("test.edgion.io/controller".to_string())
        .parse(ebtp, ctx)
        .await;
}

/// CRITICAL regression test: applying/changing a BackendTLSPolicy must requeue every
/// EdgionBackendTrafficPolicy that targets the same Service. Without this edge, the EBTP
/// `upstreamAuthority` cross-object admission rule — which runs only inside EBTP's own
/// `validate()` — is order-dependent: applying the offending BackendTLSPolicy AFTER the
/// EBTP already exists (the canonical ops sequence this rule is meant to catch) would
/// otherwise never re-trigger the EBTP's reconcile, leaving a stale `Accepted=True` forever.
#[tokio::test]
async fn backend_tls_policy_change_requeues_dependent_ebtp() {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
    use std::sync::atomic::Ordering;

    let (ctx, rt) = make_ctx();

    // Register an EdgionBackendTrafficPolicy processor so `ctx.requeue("EdgionBackendTrafficPolicy", ..)`
    // has somewhere to land; its workqueue metrics are the test's observation point.
    let ebtp_proc = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        128,
        Arc::new(
            crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler::new(
                "test.edgion.io/controller".to_string(),
            ),
        ),
        rt.clone(),
    ));
    rt.processor_registry.register(ebtp_proc.clone());

    register_ebtp_target("ns", "ebtp", "payments", &ctx).await;

    assert_eq!(
        ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        0,
        "sanity: nothing requeued yet"
    );

    // Apply a BackendTLSPolicy targeting the same Service, THEN drive on_change (mirrors
    // the reconcile pipeline: parse() followed unconditionally by on_change()).
    let handler = BackendTlsPolicyHandler::default();
    let btp = make_btp("ns", "tls", "payments", "2026-01-01T00:00:00+00:00");
    let _ = handler.parse(btp.clone(), &ctx).await;
    handler.on_change(&btp, &ctx).await;

    assert_eq!(
        ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        1,
        "BackendTLSPolicy on_change must requeue the dependent EdgionBackendTrafficPolicy"
    );
}

/// CRITICAL regression test: deleting a BackendTLSPolicy must requeue every
/// EdgionBackendTrafficPolicy that targeted the same Service, otherwise a stale
/// `Accepted=False` (from the now-removed offending hostname) outlives the object that
/// caused it.
#[tokio::test]
async fn backend_tls_policy_delete_requeues_dependent_ebtp() {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
    use std::sync::atomic::Ordering;

    let (ctx, rt) = make_ctx();

    let ebtp_proc = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        128,
        Arc::new(
            crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler::new(
                "test.edgion.io/controller".to_string(),
            ),
        ),
        rt.clone(),
    ));
    rt.processor_registry.register(ebtp_proc.clone());

    register_ebtp_target("ns", "ebtp", "payments", &ctx).await;

    let handler = BackendTlsPolicyHandler::default();
    let btp = make_btp("ns", "tls", "payments", "2026-01-01T00:00:00+00:00");
    let _ = handler.parse(btp.clone(), &ctx).await;

    assert_eq!(
        ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        0,
        "sanity: parse() alone (no on_delete yet) must not requeue the EBTP"
    );

    handler.on_delete(&btp, &ctx).await;

    assert_eq!(
        ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        1,
        "BackendTLSPolicy on_delete must requeue the dependent EdgionBackendTrafficPolicy"
    );
}

#[tokio::test]
async fn bare_policy_keeps_raw_winner_identity_and_semantic_target_dependencies() {
    let (ctx, runtime) = make_ctx();
    let handler = BackendTlsPolicyHandler::default();
    runtime.service_ref_mgr.add_ref(
        "default/svc".into(),
        ResourceRef::new(ResourceKind::HTTPRoute, Some("default".into()), "route".into()),
    );
    runtime.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        "default/route",
        &[ParentReference {
            group: None,
            kind: None,
            namespace: None,
            name: "gateway".into(),
            section_name: None,
            port: None,
        }],
        "default",
    );
    let mut source = make_btp("default", "bare", "svc", "2026-01-01T00:00:00Z");
    source.metadata.namespace = None;
    let owner = ResourceRef::new(ResourceKind::BackendTLSPolicy, None, "bare".into());
    let ProcessResult::Continue(mut parsed) = handler.parse(source, &ctx).await else {
        panic!()
    };
    assert_eq!(
        parsed.spec.resolved_target_class,
        Some(ResolvedPolicyTargetClass::Service)
    );
    handler.update_status(&mut parsed, &ctx, &[]);
    assert!(parsed
        .status
        .as_ref()
        .unwrap()
        .ancestors
        .iter()
        .any(|a| a.conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True")));
    assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()), ["default/svc"]);
    parsed.spec.target_refs[0].name = "next".into();
    let ProcessResult::Continue(mut replaced) = handler.parse(parsed, &ctx).await else {
        panic!()
    };
    assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()), ["default/next"]);
    replaced.metadata.namespace = Some("".into());
    assert!(handler.evaluate_target(&replaced, &ctx).target.is_none());
    replaced.metadata.namespace = None;
    handler.on_delete(&replaced, &ctx).await;
    assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());
    assert!(runtime
        .backend_tls_policy
        .target_mgr()
        .get_dependencies(&owner.key())
        .is_empty());
}

fn material_policy(kind: &str, count: usize) -> BackendTLSPolicy {
    let mut policy = make_btp("apps", "material-policy", "backend", "2024-01-01T00:00:00Z");
    policy.spec.validation.well_known_ca_certificates = None;
    policy.spec.validation.ca_certificate_refs = Some(
        (0..count)
            .map(|i| BackendTLSPolicyCACertificateRef {
                group: String::new(),
                kind: kind.into(),
                name: format!("ca-{i}"),
            })
            .collect(),
    );
    policy
}

fn material_parse_observed(policy: BackendTLSPolicy, ctx: &HandlerContext) -> (BackendTLSPolicy, Vec<&'static str>) {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    super::super::super::credential::pipeline::observe_bound_reads(|| {
        let handler = BackendTlsPolicyHandler::default();
        let mut future = std::pin::pin!(handler.parse_policy(policy, ctx));
        let Poll::Ready(ProcessResult::Continue(parsed)) =
            future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("BTP parse must not suspend");
        };
        parsed
    })
}

#[test]
fn backend_tls_actual_ca_kind_pairing_status_and_repair_are_readonly() {
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap, HashSet};
    let (ctx, runtime) = make_ctx();
    runtime.service_ref_mgr.add_ref(
        "apps/backend".into(),
        ResourceRef::new(ResourceKind::HTTPRoute, Some("apps".into()), "route".into()),
    );
    runtime.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        "apps/route",
        &[ParentReference {
            group: Some("gateway.networking.k8s.io".into()),
            kind: Some("Gateway".into()),
            namespace: Some("apps".into()),
            name: "gateway".into(),
            section_name: None,
            port: None,
        }],
        "apps",
    );
    let mut policy = material_policy("Secret", 1);
    policy
        .spec
        .validation
        .ca_certificate_refs
        .as_mut()
        .unwrap()
        .push(BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: "ConfigMap".into(),
            name: "ca-0".into(),
        });
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some("ca-0".into()),
            namespace: Some("apps".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("ca.crt".into(), ByteString(b"ca".to_vec()))])),
        ..Default::default()
    };
    runtime
        .secret_store
        .update(HashMap::from([("apps/ca-0".into(), secret)]), &HashSet::new());
    let (first, reads) = material_parse_observed(policy.clone(), &ctx);
    assert!(!reads.is_empty());
    assert!(!BackendTlsPolicyHandler::all_runtime_refs_resolved(&first));
    assert!(first.spec.resolved_target_class.is_none());
    let fields = BackendTlsRefs::from_policy(&first);
    assert!(fields.ca(0).unwrap().resolved());
    assert!(!fields.ca(1).unwrap().resolved());
    let owner = ResourceRef::new(
        ResourceKind::BackendTLSPolicy,
        Some("apps".into()),
        "material-policy".into(),
    );
    let dependencies: BTreeSet<_> = runtime
        .secret_ref_manager
        .get_dependencies(&owner.key())
        .into_iter()
        .collect();
    assert_eq!(dependencies.len(), 2);
    let mut status = None;
    let (_, status_reads) = super::super::super::credential::pipeline::observe_bound_reads(|| {
        BackendTlsPolicyHandler::default().update_policy_status(&first, &mut status, &ctx, &[]);
    });
    assert!(status_reads.is_empty());
    let ancestors = &status.as_ref().unwrap().ancestors;
    assert_eq!(ancestors.len(), 1);
    assert_eq!(ancestors[0].ancestor_ref.name, "gateway");
    for (kind, value, reason) in [
        ("ResolvedRefs", "False", "InvalidCACertificateRef"),
        ("Accepted", "True", "Accepted"),
    ] {
        let condition = ancestors[0].conditions.iter().find(|c| c.type_ == kind).unwrap();
        assert_eq!(condition.status, value);
        assert_eq!(condition.reason, reason);
    }
    assert_eq!(
        runtime
            .secret_ref_manager
            .get_dependencies(&owner.key())
            .into_iter()
            .collect::<BTreeSet<_>>(),
        dependencies
    );
    runtime.configmap_store.update(
        HashMap::from([(
            "apps/ca-0".into(),
            ConfigMap {
                metadata: ObjectMeta {
                    name: Some("ca-0".into()),
                    namespace: Some("apps".into()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([("ca.crt".into(), "configmap-ca".into())])),
                ..Default::default()
            },
        )]),
        &HashSet::new(),
    );
    let (repaired, _) = material_parse_observed(policy, &ctx);
    assert!(BackendTlsPolicyHandler::all_runtime_refs_resolved(&repaired));
    assert!(repaired.spec.resolved_target_class.is_some());
    assert_eq!(repaired.spec.resolved_ca_certificates.as_ref().unwrap().len(), 2);
    let (_, status_reads) = super::super::super::credential::pipeline::observe_bound_reads(|| {
        BackendTlsPolicyHandler::default().update_policy_status(&repaired, &mut status, &ctx, &[]);
    });
    assert!(status_reads.is_empty());
    assert_eq!(
        runtime
            .secret_ref_manager
            .get_dependencies(&owner.key())
            .into_iter()
            .collect::<BTreeSet<_>>(),
        dependencies
    );
    let ancestors = &status.as_ref().unwrap().ancestors;
    assert_eq!(ancestors.len(), 1);
    for kind in ["ResolvedRefs", "Accepted"] {
        let condition = ancestors[0].conditions.iter().find(|c| c.type_ == kind).unwrap();
        assert_eq!(condition.status, "True");
        assert_eq!(
            condition.reason,
            if kind == "ResolvedRefs" {
                "ResolvedRefs"
            } else {
                "Accepted"
            }
        );
    }
}

#[test]
fn backend_tls_bounds_and_invalid_fields_clear_forged_material_before_effects() {
    use k8s_openapi::api::core::v1::Secret;
    for case in ["eight", "nine", "core", "name", "owner-name", "owner-ns"] {
        let (ctx, runtime) = make_ctx();
        let mut policy = material_policy(
            "Secret",
            if case == "nine" {
                9
            } else if case == "eight" {
                8
            } else {
                1
            },
        );
        match case {
            "core" => policy.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].group = "core".into(),
            "name" => policy.spec.validation.ca_certificate_refs.as_mut().unwrap()[0]
                .name
                .clear(),
            "owner-name" => policy.metadata.name = Some("bad/name".into()),
            "owner-ns" => policy.metadata.namespace = Some(String::new()),
            _ => {}
        }
        if case == "nine" {
            policy.spec.options = Some([("edgion.io/client-certificate-ref".into(), "ca-0".into())].into());
        }
        let owner = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            policy.metadata.namespace.clone(),
            policy.metadata.name.clone().unwrap(),
        );
        policy.spec.resolved_ca_certificates = Some(vec![Secret::default(); 4096]);
        policy.spec.resolved_client_certificate = Some(Secret::default());
        let handler = BackendTlsPolicyHandler::default();
        let errors = handler.validate(&policy, &ctx);
        if case == "nine" {
            assert!(errors.iter().any(|e| e.contains("at most 8")));
            assert!(errors.len() < 5);
        }
        let (parsed, reads) = material_parse_observed(policy, &ctx);
        assert!(parsed.spec.resolved_ca_certificates.is_none());
        assert!(parsed.spec.resolved_client_certificate.is_none());
        if case == "eight" {
            assert_eq!(runtime.secret_ref_manager.get_dependencies(&owner.key()).len(), 8);
            assert!(!reads.is_empty());
        } else {
            assert!(reads.is_empty(), "{case}: {reads:?}");
            assert!(runtime.secret_ref_manager.get_dependencies(&owner.key()).is_empty());
        }
        let mut forged = parsed;
        forged.spec.resolved_ca_certificates = Some(vec![Secret::default(); 4096]);
        let (_, reads) = super::super::super::credential::pipeline::observe_bound_reads(|| {
            handler.update_policy_status(&forged, &mut None, &ctx, &[])
        });
        assert!(reads.is_empty());
        assert!(!BackendTlsRefs::from_policy(&forged).all_materials_resolved());
    }
}

#[test]
fn backend_tls_eight_actual_materials_and_system_client_option_publish_correctly() {
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::{HashMap, HashSet};
    let (ctx, runtime) = make_ctx();
    runtime.secret_store.update(
        (0..8)
            .map(|i| {
                (
                    format!("apps/ca-{i}"),
                    Secret {
                        metadata: ObjectMeta {
                            name: Some(format!("ca-{i}")),
                            namespace: Some("apps".into()),
                            ..Default::default()
                        },
                        data: Some(
                            [
                                ("ca.crt".into(), ByteString(b"ca".to_vec())),
                                ("tls.crt".into(), ByteString(b"crt".to_vec())),
                                ("tls.key".into(), ByteString(b"key".to_vec())),
                            ]
                            .into(),
                        ),
                        ..Default::default()
                    },
                )
            })
            .collect::<HashMap<_, _>>(),
        &HashSet::new(),
    );
    let (parsed, reads) = material_parse_observed(material_policy("Secret", 8), &ctx);
    assert!(!reads.is_empty());
    assert_eq!(parsed.spec.resolved_ca_certificates.as_ref().unwrap().len(), 8);
    assert!(BackendTlsPolicyHandler::all_runtime_refs_resolved(&parsed));
    assert!(parsed.spec.resolved_target_class.is_some());
    let mut system = make_btp("apps", "material-policy", "backend", "2024-01-01T00:00:00Z");
    system.spec.options = Some([("edgion.io/client-certificate-ref".into(), "  ca-0  ".into())].into());
    let (parsed, _) = material_parse_observed(system.clone(), &ctx);
    assert!(parsed.spec.resolved_ca_certificates.is_none());
    assert_eq!(
        parsed
            .spec
            .resolved_client_certificate
            .as_ref()
            .unwrap()
            .metadata
            .name
            .as_deref(),
        Some("ca-0")
    );
    assert!(BackendTlsPolicyHandler::all_runtime_refs_resolved(&parsed));
    system.spec.options = None;
    let (parsed, _) = material_parse_observed(system, &ctx);
    assert!(parsed.spec.resolved_client_certificate.is_none());
    let owner = ResourceRef::new(
        ResourceKind::BackendTLSPolicy,
        Some("apps".into()),
        "material-policy".into(),
    );
    assert!(runtime.secret_ref_manager.get_dependencies(&owner.key()).is_empty());
}
