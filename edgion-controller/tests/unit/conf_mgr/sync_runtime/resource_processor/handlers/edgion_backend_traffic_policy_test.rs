use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use serde_json::json;
use std::sync::Arc;

const TEST_CONTROLLER: &str = "test.edgion.io/controller";

fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_handler() -> EdgionBackendTrafficPolicyHandler {
    EdgionBackendTrafficPolicyHandler::new(TEST_CONTROLLER.to_string())
}

/// Wire "route (ns/route) -> gateway (ns/gw)" and "svc -> route" so the
/// ancestor resolver finds one ancestor Gateway for the Service.
fn wire_service_to_gateway(rt: &ControllerRuntime, ns: &str, svc: &str, route: &str, gw: &str) {
    rt.service_ref_mgr.add_ref(
        format!("{ns}/{svc}"),
        ResourceRef::new(
            edgion_resources::ResourceKind::HTTPRoute,
            Some(ns.to_string()),
            route.to_string(),
        ),
    );
    rt.gateway_route_index.update_route(
        edgion_resources::ResourceKind::HTTPRoute,
        &format!("{ns}/{route}"),
        &[ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some(ns.to_string()),
            name: gw.to_string(),
            section_name: None,
            port: None,
        }],
        ns,
    );
}

fn wire_backend_to_gateway(rt: &ControllerRuntime, ns: &str, ai: &str, route: &str, gw: &str) {
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: ns.to_string(),
        name: ai.to_string(),
    };
    rt.backend_ref_mgr.add_ref(
        target.to_string(),
        ResourceRef::new(ResourceKind::HTTPRoute, Some(ns.to_string()), route.to_string()),
    );
    rt.gateway_route_index.update_route(
        ResourceKind::HTTPRoute,
        &format!("{ns}/{route}"),
        &[ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some(ns.to_string()),
            name: gw.to_string(),
            section_name: None,
            port: None,
        }],
        ns,
    );
}

/// Condition of type `type_` on the ancestor whose gateway name is `gw`.
fn ancestor_condition<'a>(
    p: &'a EdgionBackendTrafficPolicy,
    gw: &str,
    type_: &str,
) -> &'a edgion_resources::resources::common::Condition {
    p.status
        .as_ref()
        .map(|s| s.ancestors.as_slice())
        .unwrap_or_default()
        .iter()
        .find(|a| a.ancestor_ref.name == gw)
        .unwrap_or_else(|| panic!("ancestor for gateway {gw} present"))
        .conditions
        .iter()
        .find(|c| c.type_ == type_)
        .unwrap_or_else(|| panic!("condition {type_} present on ancestor {gw}"))
}

fn make_policy(name: &str, target_svc: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
    serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": {
            "name": name,
            "namespace": "ns",
            "creationTimestamp": creation_ts,
        },
        "spec": {
            "targetRefs": [ { "kind": "Service", "name": target_svc } ],
        },
    }))
    .expect("valid EdgionBackendTrafficPolicy JSON")
}

fn make_ai_policy(name: &str, target: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
    serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": {
            "name": name,
            "namespace": "ns",
            "creationTimestamp": creation_ts,
        },
        "spec": {
            "targetRefs": [
                { "group": "edgion.io", "kind": "EdgionBackend", "name": target }
            ],
            "connection": { "connectTimeout": "2s" },
            "retryConstraint": {},
            "circuitBreaker": { "maxParallelRequests": 20 },
            "outlierDetection": { "consecutiveErrors": 5 }
        },
    }))
    .expect("valid AI-target EdgionBackendTrafficPolicy JSON")
}

fn install_raw_ai_backend(runtime: &Arc<ControllerRuntime>, name: &str) {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let backend: EdgionBackend = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackend",
        "metadata": {"namespace": "ns", "name": name},
        "spec": {"ai": {
            "provider": "OpenAI",
            "credentialPool": {"credentials": [{
                "name": "primary", "secretRef": {"name": "provider-key"}
            }]},
            "models": [{"name": "model-a"}]
        }}
    }))
    .unwrap();
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(format!("ns/{name}"), Arc::new(backend));
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);
}

#[tokio::test]
async fn same_named_service_and_backend_do_not_conflict() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "shared", "service-route", "gw");
    wire_backend_to_gateway(&rt, "ns", "shared", "ai-route", "gw");

    let service_policy = make_policy("service-policy", "shared", "2026-01-01T00:00:00Z");
    let ai_policy = make_ai_policy("ai-policy", "shared", "2026-06-01T00:00:00Z");
    let _ = handler.parse(service_policy.clone(), &ctx).await;
    let _ = handler.parse(ai_policy.clone(), &ctx).await;

    for mut policy in [service_policy, ai_policy] {
        handler.update_status(&mut policy, &ctx, &[]);
        let accepted = ancestor_condition(&policy, "gw", condition_types::ACCEPTED);
        assert_eq!(accepted.status, "True");
        assert_eq!(accepted.reason, condition_reasons::ACCEPTED);
    }
}

#[tokio::test]
async fn backend_policies_conflict_and_resolve_ancestors_by_typed_key() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    install_raw_ai_backend(&rt, "provider");
    wire_backend_to_gateway(&rt, "ns", "provider", "ai-route", "gw-ai");

    let older = make_ai_policy("older", "provider", "2026-01-01T00:00:00Z");
    let younger = make_ai_policy("younger", "provider", "2026-06-01T00:00:00Z");
    let _ = handler.parse(older, &ctx).await;
    let _ = handler.parse(younger.clone(), &ctx).await;

    let mut younger = younger;
    handler.update_status(&mut younger, &ctx, &[]);
    let accepted = ancestor_condition(&younger, "gw-ai", condition_types::ACCEPTED);
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
    assert!(accepted.message.contains("edgion.io/EdgionBackend/ns/provider"));
}

#[tokio::test]
async fn missing_backend_sets_target_not_found() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use edgion_resources::prelude_resources::EdgionBackend;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_backend_to_gateway(&rt, "ns", "missing", "ai-route", "gw-ai");
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        128,
        Arc::new(EdgionBackendHandler::new()),
        rt.clone(),
    ));
    rt.processor_registry.register(processor);

    let policy = make_ai_policy("policy", "missing", "2026-01-01T00:00:00Z");
    let _ = handler.parse(policy.clone(), &ctx).await;
    let mut policy = policy;
    handler.update_status(&mut policy, &ctx, &[]);

    let resolved = ancestor_condition(&policy, "gw-ai", condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, ebtp_reasons::TARGET_NOT_FOUND);
    assert!(resolved.message.contains("edgion.io/EdgionBackend/ns/missing"));
}

#[tokio::test]
async fn forged_resolved_target_class_is_cleared_before_staging() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        128,
        Arc::new(EdgionBackendHandler::new()),
        rt.clone(),
    ));
    rt.processor_registry.register(processor);

    let mut forged = make_ai_policy("forged", "missing", "2026-01-01T00:00:00Z");
    forged.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Ai);
    let parsed = match handler.parse(forged, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };

    assert!(parsed.spec.resolved_target_class.is_none());
    assert!(rt
        .edgion_backend_traffic_policy
        .target_mgr()
        .get_dependencies(&EdgionBackendTrafficPolicyHandler::resource_ref(&parsed).key())
        .is_empty());
}

#[tokio::test]
async fn complete_ai_policy_sets_resolved_target_class() {
    let (ctx, rt) = make_ctx();
    install_raw_ai_backend(&rt, "provider");
    let handler = make_handler();
    let policy = make_ai_policy("complete", "provider", "2026-01-01T00:00:00Z");
    let parsed = match handler.parse(policy, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert_eq!(parsed.spec.resolved_target_class, Some(ResolvedPolicyTargetClass::Ai));
}

/// `validate()` returns the spec's validation errors verbatim.
#[test]
fn validate_returns_spec_validation_errors_verbatim() {
    let (ctx, _rt) = make_ctx();
    let handler = make_handler();

    // Empty targetRefs — spec is invalid.
    let mut invalid = make_policy("bad", "svc", "2026-01-01T00:00:00Z");
    invalid.spec.target_refs.clear();
    let expected = invalid.spec.get_validation_errors();
    assert!(!expected.is_empty(), "empty targetRefs must be a validation error");
    assert_eq!(handler.validate(&invalid, &ctx), expected);

    // Valid spec — no errors.
    let valid = make_policy("good", "svc", "2026-01-01T00:00:00Z");
    assert!(handler.validate(&valid, &ctx).is_empty());
}

#[tokio::test]
async fn health_check_zero_port_withdraws_attachment_and_recovers_on_update() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "svc", "route", "gw");
    let mut policy = make_policy("health-port", "svc", "2026-01-01T00:00:00Z");

    for port in [Some(9090), Some(0), None, Some(65535)] {
        policy.spec.health_check = Some(serde_json::from_value(json!({"active": {"port": port}})).unwrap());
        let errors = handler.validate(&policy, &ctx);
        let valid = port != Some(0);
        assert_eq!(errors.is_empty(), valid);
        if !valid {
            assert_eq!(errors, ["healthCheck: port must be >= 1"]);
        }
        policy = match handler.parse(policy, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(
            policy.spec.resolved_target_class,
            valid.then_some(ResolvedPolicyTargetClass::Service)
        );
        let owner = EdgionBackendTrafficPolicyHandler::resource_ref(&policy);
        assert_eq!(
            rt.edgion_backend_traffic_policy
                .target_mgr()
                .get_dependencies(&owner.key())
                .len(),
            usize::from(valid)
        );
        assert_eq!(rt.service_ref_mgr.get_dependencies(&owner.key()).len(), 1);
        handler.update_status(&mut policy, &ctx, &errors);
        let accepted = ancestor_condition(&policy, "gw", condition_types::ACCEPTED);
        assert_eq!(accepted.status, if valid { "True" } else { "False" });
        assert_eq!(
            accepted.reason,
            if valid {
                condition_reasons::ACCEPTED
            } else {
                condition_reasons::INVALID
            }
        );
        if !valid {
            assert!(accepted.message.contains("healthCheck: port must be >= 1"));
        }
    }
}

/// Three policies target the same Service with mixed timestamps: the winner is
/// the `precedence_key` minimum (oldest); the two losers are flagged
/// `Accepted=False/Conflicted` on the ancestor the shared Service reaches.
#[tokio::test]
async fn conflict_evaluation_oldest_wins_others_flagged() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

    // `b` is the oldest (min precedence_key), so it wins; `a` and `c` lose.
    let a = make_policy("a", "payments", "2026-03-01T00:00:00Z");
    let b = make_policy("b", "payments", "2026-01-01T00:00:00Z");
    let c = make_policy("c", "payments", "2026-06-01T00:00:00Z");

    for p in [a.clone(), b.clone(), c.clone()] {
        let _ = handler.parse(p, &ctx).await;
    }

    let check = |mut p: EdgionBackendTrafficPolicy, expect_conflicted: bool| {
        handler.update_status(&mut p, &ctx, &[]);
        let accepted = ancestor_condition(&p, "gw-1", condition_types::ACCEPTED);
        if expect_conflicted {
            assert_eq!(
                accepted.status,
                "False",
                "{} should be conflicted",
                p.metadata.name.as_deref().unwrap()
            );
            assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
        } else {
            assert_eq!(accepted.status, "True", "winner must not be conflicted");
            assert_eq!(accepted.reason, condition_reasons::ACCEPTED);
        }
    };

    check(b, false); // oldest wins
    check(a, true); // loser
    check(c, true); // loser
}

/// Carry-forward: re-running `update_status` on an unchanged world must keep
/// each surviving ancestor's condition `lastTransitionTime` stable, and a
/// state flip must move only the condition that actually flipped. Uses a
/// sentinel timestamp because `now_rfc3339` is second-granularity, so two
/// fast consecutive runs would otherwise collide and mask a lost carry-forward.
#[tokio::test]
async fn condition_last_transition_time_carried_forward() {
    const SENTINEL: &str = "1970-01-01T00:00:00Z";

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

    // `p` alone is the winner (Accepted=True, ResolvedRefs=True).
    let p = make_policy("p", "payments", "2026-06-01T00:00:00Z");
    let _ = handler.parse(p.clone(), &ctx).await;

    let mut p = p;
    handler.update_status(&mut p, &ctx, &[]);
    // Stamp every condition with a sentinel so a lost carry-forward (fresh
    // `now_rfc3339`) is visibly distinguishable from a preserved timestamp.
    for a in &mut p.status.as_mut().unwrap().ancestors {
        for c in &mut a.conditions {
            c.last_transition_time = SENTINEL.to_string();
        }
    }

    // Unchanged world: both conditions keep the sentinel (carry-forward +
    // no status flip). A `Vec::new()`-per-ancestor rebuild would drop the
    // sentinel and restamp with the current time.
    handler.update_status(&mut p, &ctx, &[]);
    assert_eq!(
        ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).last_transition_time,
        SENTINEL,
        "unchanged Accepted must keep its lastTransitionTime"
    );
    assert_eq!(
        ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS).last_transition_time,
        SENTINEL,
        "unchanged ResolvedRefs must keep its lastTransitionTime"
    );

    // Introduce an older valid policy targeting the same Service: `p` now
    // loses arbitration, so its Accepted condition flips True -> False.
    let older = make_policy("older", "payments", "2026-01-01T00:00:00Z");
    let _ = handler.parse(older, &ctx).await;

    handler.update_status(&mut p, &ctx, &[]);
    assert_ne!(
        ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).last_transition_time,
        SENTINEL,
        "flipped Accepted must get a fresh lastTransitionTime"
    );
    assert_eq!(
        ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).reason,
        condition_reasons::POLICY_CONFLICTED
    );
    assert_eq!(
        ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS).last_transition_time,
        SENTINEL,
        "non-flipped ResolvedRefs must still keep its lastTransitionTime"
    );
}

#[tokio::test]
async fn mixed_policy_registers_dependencies_but_publishes_no_subset() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    let policy: EdgionBackendTrafficPolicy = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": { "name": "mixed", "namespace": "ns" },
        "spec": {
            "targetRefs": [
                { "kind": "Service", "name": "service" },
                { "group": "edgion.io", "kind": "EdgionBackend", "name": "backend" }
            ],
            "connection": { "connectTimeout": "2s" }
        }
    }))
    .expect("mixed policy fixture parses under lenient admission");
    let parsed = match handler.parse(policy, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    let owner = EdgionBackendTrafficPolicyHandler::resource_ref(&parsed);

    assert!(parsed.spec.resolved_target_class.is_none());
    assert!(rt
        .edgion_backend_traffic_policy
        .target_mgr()
        .get_dependencies(&owner.key())
        .is_empty());
    assert_eq!(rt.service_ref_mgr.get_dependencies(&owner.key()).len(), 1);
    assert_eq!(rt.backend_ref_mgr.get_dependencies(&owner.key()).len(), 1);
}

/// Build an INVALID policy (ConsistentHash without a `consistentHash` section)
/// targeting `target_svc` — mirrors `Accepted=False/Invalid` admission.
fn make_invalid_policy(name: &str, target_svc: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
    serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": {
            "name": name,
            "namespace": "ns",
            "creationTimestamp": creation_ts,
        },
        "spec": {
            "targetRefs": [ { "kind": "Service", "name": target_svc } ],
            "loadBalancer": { "type": "ConsistentHash" },
        },
    }))
    .expect("valid EdgionBackendTrafficPolicy JSON")
}

/// Controller conflict participation must match the gateway: an INVALID older
/// policy is a data-plane no-op, so it must NOT compete for oldest-wins. A VALID
/// newer policy targeting the same Service therefore wins (`Accepted=True`).
/// When the older policy is later edited to become valid, the newer one flips to
/// `Accepted=False/Conflicted`.
#[tokio::test]
async fn invalid_older_policy_does_not_conflict_valid_newer() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

    // A is older but INVALID; B is newer and valid; both target `payments`.
    let a_invalid = make_invalid_policy("a", "payments", "2026-01-01T00:00:00Z");
    let b = make_policy("b", "payments", "2026-06-01T00:00:00Z");
    assert!(
        !a_invalid.spec.get_validation_errors().is_empty(),
        "A fixture must be invalid"
    );
    assert!(b.spec.get_validation_errors().is_empty(), "B fixture must be valid");

    let _ = handler.parse(a_invalid, &ctx).await;
    let _ = handler.parse(b.clone(), &ctx).await;

    // B wins: the invalid older policy does not participate in the conflict index.
    let mut b_status = b.clone();
    handler.update_status(&mut b_status, &ctx, &[]);
    let accepted = ancestor_condition(&b_status, "gw-1", condition_types::ACCEPTED);
    assert_eq!(
        accepted.status, "True",
        "valid newer policy must win over an invalid older one"
    );
    assert_eq!(accepted.reason, condition_reasons::ACCEPTED);

    // Edit A to a valid spec (update path) — it now participates and, being older,
    // wins; B must flip to Accepted=False/Conflicted.
    let a_valid = make_policy("a", "payments", "2026-01-01T00:00:00Z");
    assert!(a_valid.spec.get_validation_errors().is_empty());
    let _ = handler.parse(a_valid, &ctx).await;

    let mut b_status = b.clone();
    handler.update_status(&mut b_status, &ctx, &[]);
    let accepted = ancestor_condition(&b_status, "gw-1", condition_types::ACCEPTED);
    assert_eq!(
        accepted.status, "False",
        "now-valid older policy wins; newer becomes conflicted"
    );
    assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
}

#[tokio::test]
async fn duplicate_service_targets_do_not_requeue_unchanged_siblings() {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use std::sync::atomic::Ordering;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    let processor = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        128,
        Arc::new(make_handler()),
        rt.clone(),
    ));
    rt.processor_registry.register(processor.clone());

    let duplicate_target = |mut policy: EdgionBackendTrafficPolicy| {
        policy.spec.target_refs.push(policy.spec.target_refs[0].clone());
        policy
    };
    let older = duplicate_target(make_policy("older", "payments", "2026-01-01T00:00:00Z"));
    let younger = duplicate_target(make_policy("younger", "payments", "2026-06-01T00:00:00Z"));
    assert!(older.spec.get_validation_errors().is_empty());
    assert!(younger.spec.get_validation_errors().is_empty());

    let _ = handler.parse(older.clone(), &ctx).await;
    let _ = handler.parse(younger, &ctx).await;
    let scheduled_after_initial_arbitration = processor.workqueue().metrics().delayed_total.load(Ordering::Relaxed);

    let _ = handler.parse(older, &ctx).await;

    assert_eq!(
        processor.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        scheduled_after_initial_arbitration,
        "an unchanged duplicate Service target set must not requeue its sibling"
    );
}

/// ResolvedRefs is `False`/`TargetNotFound` when the target Service is not in
/// the cache (no Service processor registered in the test runtime → `service_exists`
/// returns `true`, so we assert the resolved branch here) — the policy still syncs.
#[tokio::test]
async fn resolved_refs_true_when_service_lookup_unavailable() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");
    let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
    let _ = handler.parse(p.clone(), &ctx).await;
    let mut p = p;
    handler.update_status(&mut p, &ctx, &[]);
    let resolved = ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "True");
    assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
}

/// With a Service processor registered but its cache EMPTY, `service_exists`
/// genuinely returns `false`, so ResolvedRefs is `False`/`TargetNotFound`.
/// Adding the target Service to the cache and re-running flips it to `True`.
#[tokio::test]
async fn resolved_refs_false_then_true_when_service_appears() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use k8s_openapi::api::core::v1::Service;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

    // Register a Service processor with an empty cache: now
    // `route_utils::service_exists` consults it (instead of the no-processor
    // short-circuit that returns true) and finds the target absent.
    let svc_proc = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        128,
        Arc::new(ServiceHandler::new()),
        rt.clone(),
    ));
    rt.processor_registry.register(svc_proc.clone());
    let raw = Arc::new(LocalRawStore::<Service>::new());
    svc_proc.set_raw_store(raw.clone());

    let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
    let _ = handler.parse(p.clone(), &ctx).await;

    // Target Service not in cache -> ResolvedRefs=False/TargetNotFound.
    let mut p_missing = p.clone();
    handler.update_status(&mut p_missing, &ctx, &[]);
    let resolved = ancestor_condition(&p_missing, "gw-1", condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False", "absent Service => ResolvedRefs False");
    assert_eq!(resolved.reason, ebtp_reasons::TARGET_NOT_FOUND);

    // Add the Service to the cache (key = "ns/payments") and re-run: flips True.
    let svc = Service {
        metadata: ObjectMeta {
            name: Some("payments".to_string()),
            namespace: Some("ns".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    raw.upsert("ns/payments".to_string(), Arc::new(svc.clone()));
    svc_proc.save(svc, None);

    let mut p_present = p.clone();
    handler.update_status(&mut p_present, &ctx, &[]);
    let resolved = ancestor_condition(&p_present, "gw-1", condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "True", "Service now present => ResolvedRefs True");
    assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
}

/// An invalid spec is admitted leniently: Accepted=False/Invalid but the
/// resource still produces a status (it will still sync to the gateway).
///
/// Deviation from a literal translation of the pre-GEP-2648 fixture: the
/// original cleared `target_refs` entirely to make the spec invalid, but
/// under GEP-2648 zero targetRefs resolves zero ancestor Gateways (there
/// is no Service to walk Service->Route->Gateway through), so
/// `status.ancestors` would legitimately be empty and there would be no
/// ancestor to assert on. Using `make_invalid_policy` instead keeps a real
/// targetRef (`svc`) while still producing a spec-local validation error
/// (ConsistentHash without a `consistentHash` section), so an ancestor can
/// be wired and the Accepted=False/Invalid condition asserted on it.
#[test]
fn invalid_spec_is_accepted_false_invalid() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "svc", "r1", "gw-1");
    let mut p = make_invalid_policy("bad", "svc", "2026-01-01T00:00:00Z");
    let errors = handler.validate(&p, &ctx);
    assert!(!errors.is_empty(), "fixture must be spec-locally invalid");
    handler.update_status(&mut p, &ctx, &errors);
    let accepted = ancestor_condition(&p, "gw-1", condition_types::ACCEPTED);
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, condition_reasons::INVALID);
}

/// A BackendTLSPolicy targeting the same Service with a hostname outside the
/// authority pattern is an admission error: that hostname is either a stale SNI
/// or a certificate that cannot cover the tenant subdomains.
#[tokio::test]
async fn backend_tls_hostname_outside_pattern_is_rejected() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::prelude_resources::BackendTLSPolicy;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    install_raw_ai_backend(&rt, "sandbox-gw");

    let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
        "BackendTLSPolicy",
        128,
        Arc::new(BackendTlsPolicyHandler::new(
            crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
        )),
        rt.clone(),
    ));
    rt.processor_registry.register(btls_proc.clone());

    // `backend_tls_hostnames_for_service` mirrors `RuntimeDelegationLookup::get_route`
    // (http_route.rs): `list_keys()`/visibility come from the ServerCache (`save`),
    // but the typed value comes from the raw store (`get_raw_any`), so both must be
    // populated for the lookup to see the fixture.
    let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
    btls_proc.set_raw_store(btls_raw.clone());

    let btls: BackendTLSPolicy = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "BackendTLSPolicy",
        "metadata": { "name": "tls", "namespace": "ns" },
        "spec": {
            "targetRefs": [ { "group": "", "kind": "Service", "name": "sandbox-gw" } ],
            "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
        },
    }))
    .expect("valid BackendTLSPolicy JSON");
    btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
    btls_proc.save(btls, None);

    let mut p = make_policy("p", "sandbox-gw", "2026-01-01T00:00:00Z");
    p.spec.upstream_authority = Some(
        edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.sandbox.internal".to_string(),
            health_check_host: None,
        },
    );

    let errors = handler.validate(&p, &ctx);
    assert_eq!(errors.len(), 1, "got: {errors:?}");
    assert!(errors[0].contains("stale.other.com"), "got: {}", errors[0]);
    assert!(errors[0].contains("*.sandbox.internal"), "got: {}", errors[0]);
    // Minor 1: the offending BackendTLSPolicy's resource key must be named in the
    // error so an operator with several BackendTLSPolicies on one Service can tell
    // which object to fix.
    assert!(errors[0].contains("ns/tls"), "got: {}", errors[0]);

    // The cross-object hostname rule is Service-only. An AI-target policy
    // already rejects upstreamAuthority through its field matrix, but must
    // not additionally inspect a same-named Service's BackendTLSPolicy.
    let mut ai_policy = make_ai_policy("ai", "sandbox-gw", "2026-01-01T00:00:00Z");
    ai_policy.spec.upstream_authority = p.spec.upstream_authority.clone();
    let ai_errors = handler.validate(&ai_policy, &ctx);
    assert_eq!(ai_errors.len(), 1, "got: {ai_errors:?}");
    assert!(ai_errors[0].contains("not supported for AI EdgionBackend"));
    assert!(!ai_errors[0].contains("stale.other.com"));

    // An in-pattern hostname is accepted.
    let btls_ok: BackendTLSPolicy = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "BackendTLSPolicy",
        "metadata": { "name": "tls", "namespace": "ns" },
        "spec": {
            "targetRefs": [ { "group": "", "kind": "Service", "name": "sandbox-gw" } ],
            "validation": { "hostname": "default.sandbox.internal", "wellKnownCACertificates": "System" }
        },
    }))
    .expect("valid BackendTLSPolicy JSON");
    btls_raw.upsert("ns/tls".to_string(), Arc::new(btls_ok.clone()));
    btls_proc.save(btls_ok, None);
    assert!(handler.validate(&p, &ctx).is_empty());
}

/// Minor 2: only core-group `Service` targetRefs are considered, mirroring
/// `EdgionBackendTrafficPolicySpec::get_validation_errors`'s own targetRef acceptance.
/// A BackendTLSPolicy whose targetRef has a non-core group must NOT be treated as
/// targeting the Service, even if `kind` and `name` match.
#[tokio::test]
async fn backend_tls_policy_with_non_core_group_targetref_is_ignored() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::prelude_resources::BackendTLSPolicy;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();

    let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
        "BackendTLSPolicy",
        128,
        Arc::new(BackendTlsPolicyHandler::new(
            crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
        )),
        rt.clone(),
    ));
    rt.processor_registry.register(btls_proc.clone());

    let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
    btls_proc.set_raw_store(btls_raw.clone());

    // Non-core group targetRef with a hostname that WOULD be rejected if it were
    // considered — proves the group filter, not just an absent-target coincidence.
    let btls: BackendTLSPolicy = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "BackendTLSPolicy",
        "metadata": { "name": "tls", "namespace": "ns" },
        "spec": {
            "targetRefs": [ { "group": "example.io", "kind": "Service", "name": "sandbox-gw" } ],
            "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
        },
    }))
    .expect("valid BackendTLSPolicy JSON");
    btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
    btls_proc.save(btls, None);

    let mut p = make_policy("p", "sandbox-gw", "2026-01-01T00:00:00Z");
    p.spec.upstream_authority = Some(
        edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.sandbox.internal".to_string(),
            health_check_host: None,
        },
    );

    assert!(
        handler.validate(&p, &ctx).is_empty(),
        "a non-core-group targetRef must not be treated as targeting the Service"
    );
}

/// IMPORTANT regression test: a policy that is spec-locally VALID but fails the
/// cross-object `upstreamAuthority` rule must still participate in the conflict index
/// (its target keys are registered in `parse`, gated on `spec.get_validation_errors()`).
///
/// Priority within `Accepted` is Invalid > Conflicted > Accepted, and cross-object
/// errors land in the widened `validation_errors` list, so while those errors are
/// present the reported reason is `Invalid` (by design — this MASKS `Conflicted`).
/// The test asserts that masking, then proves the policy still *participates* in
/// arbitration underneath the mask: re-running `update_status` with the cross-object
/// errors stripped away flips the reason to `Conflicted`, so a
/// conflicted-and-cross-object-invalid policy is never silently reported as a plain
/// winner once its cross-object error is fixed independently of the conflict.
#[tokio::test]
async fn cross_object_validation_cannot_be_bypassed_by_forged_empty_errors() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::prelude_resources::BackendTLSPolicy;

    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

    // Register a BackendTLSPolicy whose hostname is OUTSIDE the pattern used below, so the
    // younger EBTP's `validate()` produces a cross-object error.
    let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
        "BackendTLSPolicy",
        128,
        Arc::new(BackendTlsPolicyHandler::new(
            crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
        )),
        rt.clone(),
    ));
    rt.processor_registry.register(btls_proc.clone());
    let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
    btls_proc.set_raw_store(btls_raw.clone());
    let btls: BackendTLSPolicy = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "BackendTLSPolicy",
        "metadata": { "name": "tls", "namespace": "ns" },
        "spec": {
            "targetRefs": [ { "group": "", "kind": "Service", "name": "payments" } ],
            "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
        },
    }))
    .expect("valid BackendTLSPolicy JSON");
    btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
    btls_proc.save(btls, None);

    // Two EBTPs on the same Service, both with `upstreamAuthority`; `older` is the
    // precedence winner, `younger` is the loser.
    let mut older = make_policy("older", "payments", "2026-01-01T00:00:00Z");
    older.spec.upstream_authority = Some(
        edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.sandbox.internal".to_string(),
            health_check_host: None,
        },
    );
    let mut younger = make_policy("younger", "payments", "2026-06-01T00:00:00Z");
    younger.spec.upstream_authority = older.spec.upstream_authority.clone();

    assert!(
        older.spec.get_validation_errors().is_empty(),
        "fixture must be spec-locally valid"
    );
    assert!(
        younger.spec.get_validation_errors().is_empty(),
        "fixture must be spec-locally valid"
    );

    let _ = handler.parse(older.clone(), &ctx).await;
    let _ = handler.parse(younger.clone(), &ctx).await;

    // `younger` loses oldest-wins AND fails the cross-object rule.
    let younger_errors = handler.validate(&younger, &ctx);
    assert_eq!(younger_errors.len(), 1, "got: {younger_errors:?}");

    // With the cross-object error present, Invalid masks Conflicted (priority
    // Invalid > Conflicted > Accepted).
    let mut younger_status = younger.clone();
    handler.update_status(&mut younger_status, &ctx, &younger_errors);
    let accepted = ancestor_condition(&younger_status, "gw-1", condition_types::ACCEPTED);
    assert_eq!(accepted.status, "False");
    assert_eq!(
        accepted.reason,
        condition_reasons::INVALID,
        "cross-object validation_errors takes priority over the conflict reason"
    );

    // Passing a forged empty validation vector cannot bypass the canonical
    // evaluation: cross-object-invalid policies remain withdrawn and Invalid.
    handler.update_status(&mut younger_status, &ctx, &[]);
    let accepted = ancestor_condition(&younger_status, "gw-1", condition_types::ACCEPTED);
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, condition_reasons::INVALID);
    assert!(younger_status.spec.resolved_target_class.is_none());
}

/// Rebuilding owned ancestors preserves foreign entries and stable condition times.
#[test]
fn status_rebuild_preserves_foreign_ancestors_and_owned_transition_times() {
    for has_gateway in [false, true] {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        if has_gateway {
            wire_service_to_gateway(&rt, "ns", "payments", "route", "gateway");
        }
        let mut policy = make_policy("policy", "payments", "2026-01-01T00:00:00Z");
        let foreign: PolicyAncestorStatus = serde_json::from_value(json!({
                "ancestorRef": {"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "ns", "name": "gateway"},
                "controllerName": "foreign.example/controller",
                "conditions": [{"type": "Accepted", "status": "False", "reason": "ForeignReason",
                    "message": "Foreign controller decision", "lastTransitionTime": "2000-01-01T00:00:00Z"}]
            })).unwrap();
        let mut stale_owned = foreign.clone();
        stale_owned.controller_name = TEST_CONTROLLER.to_owned();
        stale_owned.ancestor_ref.name = "removed-gateway".to_owned();
        policy.status = Some(EdgionBackendTrafficPolicyStatus {
            ancestors: vec![foreign.clone(), stale_owned],
        });
        handler.update_status(&mut policy, &ctx, &[]);
        let ancestors = &mut policy.status.as_mut().unwrap().ancestors;
        assert_eq!(ancestors[0], foreign);
        assert_eq!(ancestors.len(), if has_gateway { 2 } else { 1 });
        if has_gateway {
            assert_eq!(ancestors[1].controller_name, TEST_CONTROLLER);
            assert_eq!(ancestors[1].ancestor_ref.name, "gateway");
            assert!(ancestors[1]
                .conditions
                .iter()
                .all(|condition| condition.reason != "ForeignReason"));
            for condition in &mut ancestors[1].conditions {
                condition.last_transition_time = "2000-01-01T00:00:00Z".to_owned();
            }
        }
        let prior = policy.status.clone().unwrap().ancestors;
        let prior_json = serde_json::to_string(&policy.status).unwrap();
        handler.update_status(&mut policy, &ctx, &[]);
        assert!(
            !crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed(
                "EdgionBackendTrafficPolicy",
                &prior_json,
                &serde_json::to_string(&policy.status).unwrap(),
                Some("ns"),
            )
        );
        assert_eq!(policy.status.unwrap().ancestors, prior);
    }
}

/// One Service reachable through two Gateways -> two owned ancestor entries.
#[tokio::test]
async fn two_gateways_yield_two_ancestor_entries() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "payments", "route-a", "gw-a");
    wire_service_to_gateway(&rt, "ns", "payments", "route-b", "gw-b");

    let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
    let _ = handler.parse(p.clone(), &ctx).await;
    let mut p = p;
    handler.update_status(&mut p, &ctx, &[]);

    let ancestors = &p.status.as_ref().unwrap().ancestors;
    assert_eq!(ancestors.len(), 2);
    let names: Vec<&str> = ancestors.iter().map(|a| a.ancestor_ref.name.as_str()).collect();
    assert_eq!(names, vec!["gw-a", "gw-b"], "sorted by gateway key");
    for a in ancestors {
        assert_eq!(a.controller_name, TEST_CONTROLLER);
        assert_eq!(a.ancestor_ref.kind.as_deref(), Some("Gateway"));
    }
}

/// A conflicted Service marks ONLY the ancestors it reaches: policy
/// targets svc-a (gw-a, unconflicted) + svc-b (gw-b, lost to an older
/// sibling) -> gw-a Accepted=True, gw-b Accepted=False/Conflicted.
#[tokio::test]
async fn conflict_marks_only_affected_ancestor() {
    let (ctx, rt) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&rt, "ns", "svc-a", "route-a", "gw-a");
    wire_service_to_gateway(&rt, "ns", "svc-b", "route-b", "gw-b");

    let older = make_policy("older", "svc-b", "2026-01-01T00:00:00Z");
    let _ = handler.parse(older, &ctx).await;

    let mut loser = serde_json::from_value::<EdgionBackendTrafficPolicy>(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackendTrafficPolicy",
        "metadata": { "name": "loser", "namespace": "ns", "creationTimestamp": "2026-06-01T00:00:00Z" },
        "spec": { "targetRefs": [
            { "kind": "Service", "name": "svc-a" },
            { "kind": "Service", "name": "svc-b" }
        ] },
    }))
    .unwrap();
    let _ = handler.parse(loser.clone(), &ctx).await;
    handler.update_status(&mut loser, &ctx, &[]);

    assert_eq!(
        ancestor_condition(&loser, "gw-a", condition_types::ACCEPTED).status,
        "True"
    );
    let on_b = ancestor_condition(&loser, "gw-b", condition_types::ACCEPTED);
    assert_eq!(on_b.status, "False");
    assert_eq!(on_b.reason, condition_reasons::POLICY_CONFLICTED);
    assert!(on_b.message.contains("ns/svc-b"));
}

#[test]
fn policy_status_message_bound_is_utf8_safe() {
    let message = bounded_status_message("π".repeat(700));
    assert!(message.len() <= STATUS_MESSAGE_MAX_BYTES);
    assert!(message.ends_with(STATUS_TRUNCATION_SUFFIX));
    assert!(std::str::from_utf8(message.as_bytes()).is_ok());
}
fn encrypted_policy(ca_kind: &str) -> EdgionBackendTrafficPolicy {
    let mut policy = make_policy("health", "payments", "2026-01-01T00:00:00Z");
    policy.spec.health_check = Some(serde_json::from_value(json!({"active": {
            "type": "https", "tls": {
                "validation": {"hostname": "health.example.com", "caCertificateRefs": [{"kind": ca_kind, "name": "health-ca"}]},
                "clientCertificateRef": {"name": "health-client"}
            }
        }})).unwrap());
    policy
}

fn health_active(
    policy: &EdgionBackendTrafficPolicy,
) -> &edgion_resources::resources::health_check::ActiveHealthCheckConfig {
    policy.spec.health_check.as_ref().unwrap().active.as_ref().unwrap()
}

async fn parse_health_policy(
    handler: &EdgionBackendTrafficPolicyHandler,
    policy: EdgionBackendTrafficPolicy,
    ctx: &HandlerContext,
) -> EdgionBackendTrafficPolicy {
    match handler.parse(policy, ctx).await {
        ProcessResult::Continue(policy) => policy,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    }
}

fn install_health_material(runtime: &ControllerRuntime, ca_kind: &str) {
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap};
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["health.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let cert = params.self_signed(&key).unwrap();
    let cert_pem = cert.pem();
    let mut secrets = HashMap::new();
    secrets.insert(
        "ns/health-client".to_string(),
        Secret {
            metadata: kube::api::ObjectMeta {
                name: Some("health-client".into()),
                namespace: Some("ns".into()),
                ..Default::default()
            },
            type_: Some("kubernetes.io/tls".into()),
            data: Some(BTreeMap::from([
                ("tls.crt".into(), ByteString(cert_pem.as_bytes().to_vec())),
                ("tls.key".into(), ByteString(key.serialize_pem().into_bytes())),
            ])),
            ..Default::default()
        },
    );
    let metadata = kube::api::ObjectMeta {
        name: Some("health-ca".into()),
        namespace: Some("ns".into()),
        ..Default::default()
    };
    if ca_kind == "Secret" {
        secrets.insert(
            "ns/health-ca".into(),
            Secret {
                metadata,
                data: Some(BTreeMap::from([("ca.crt".into(), ByteString(cert_pem.into_bytes()))])),
                ..Default::default()
            },
        );
    } else {
        runtime.configmap_store.replace_all(HashMap::from([(
            "ns/health-ca".into(),
            ConfigMap {
                metadata,
                data: Some(BTreeMap::from([("ca.crt".into(), cert_pem)])),
                ..Default::default()
            },
        )]));
    }
    runtime.secret_store.replace_all(secrets);
}

#[tokio::test]
async fn encrypted_health_material_missing_rotated_deleted_and_restored_keeps_attachment() {
    for ca_kind in ["Secret", "ConfigMap"] {
        let (ctx, runtime) = make_ctx();
        let handler = make_handler();
        let ca_ref_key = if ca_kind == "ConfigMap" {
            "ConfigMap/ns/health-ca"
        } else {
            "ns/health-ca"
        };
        wire_service_to_gateway(&runtime, "ns", "payments", "route", "gw");
        let mut policy = parse_health_policy(&handler, encrypted_policy(ca_kind), &ctx).await;
        assert!(health_active(&policy).resolved_tls_error.is_some());
        assert_eq!(
            policy.spec.resolved_target_class,
            Some(ResolvedPolicyTargetClass::Service)
        );
        assert_eq!(ctx.secret_ref_manager().get_refs(ca_ref_key).len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("ns/health-client").len(), 1);
        assert!(runtime
            .edgion_backend_traffic_policy
            .is_winner("ns/health", "/Service/ns/payments"));
        handler.update_status(&mut policy, &ctx, &[]);
        assert_eq!(
            ancestor_condition(&policy, "gw", condition_types::RESOLVED_REFS).status,
            "False"
        );
        assert_eq!(
            ancestor_condition(&policy, "gw", condition_types::ACCEPTED).status,
            "True"
        );

        install_health_material(&runtime, ca_kind);
        policy = parse_health_policy(&handler, policy, &ctx).await;
        assert!(health_active(&policy).resolved_tls_error.is_none());
        let first_generation = health_active(&policy).clone();
        handler.update_status(&mut policy, &ctx, &[]);
        assert_eq!(
            ancestor_condition(&policy, "gw", condition_types::RESOLVED_REFS).status,
            "True"
        );
        install_health_material(&runtime, ca_kind);
        policy = parse_health_policy(&handler, policy, &ctx).await;
        assert_ne!(
            health_active(&policy),
            &first_generation,
            "CA and identity rotation must change active config identity"
        );

        runtime.secret_store.replace_all(Default::default());
        runtime.configmap_store.replace_all(Default::default());
        policy = parse_health_policy(&handler, policy, &ctx).await;
        assert!(health_active(&policy).resolved_tls_error.is_some());
        assert!(health_active(&policy).resolved_ca_certificates.is_none());
        assert!(health_active(&policy).resolved_client_certificate.is_none());
        assert_eq!(
            policy.spec.resolved_target_class,
            Some(ResolvedPolicyTargetClass::Service)
        );
        install_health_material(&runtime, ca_kind);
        policy = parse_health_policy(&handler, policy, &ctx).await;
        assert!(health_active(&policy).resolved_tls_error.is_none());
        handler.on_delete(&policy, &ctx).await;
        assert!(ctx.secret_ref_manager().get_refs(ca_ref_key).is_empty());
        assert!(ctx.secret_ref_manager().get_refs("ns/health-client").is_empty());
    }
}

#[tokio::test]
async fn encrypted_health_invalid_tls_and_dependency_replacement_preserve_winner() {
    let (ctx, runtime) = make_ctx();
    let handler = make_handler();
    wire_service_to_gateway(&runtime, "ns", "payments", "route", "gw");
    let mut policy = parse_health_policy(&handler, encrypted_policy("Secret"), &ctx).await;
    install_health_material(&runtime, "Secret");
    let mut material = runtime.secret_store.get(Some("ns"), "health-ca").unwrap();
    material.data.as_mut().unwrap().get_mut("ca.crt").unwrap().0 = b"invalid certificate".to_vec();
    let identity = runtime.secret_store.get(Some("ns"), "health-client").unwrap();
    runtime.secret_store.replace_all(std::collections::HashMap::from([
        ("ns/health-ca".into(), material),
        ("ns/health-client".into(), identity),
    ]));
    policy = parse_health_policy(&handler, policy, &ctx).await;
    assert_eq!(
        health_active(&policy).resolved_tls_error.as_deref(),
        Some("health check CA certificate material is invalid")
    );
    handler.update_status(&mut policy, &ctx, &[]);
    assert_eq!(
        ancestor_condition(&policy, "gw", condition_types::RESOLVED_REFS).status,
        "False"
    );

    policy
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .client_certificate_ref
        .as_mut()
        .unwrap()
        .name = "replacement".into();
    policy = parse_health_policy(&handler, policy, &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("ns/health-client").is_empty());
    assert_eq!(ctx.secret_ref_manager().get_refs("ns/replacement").len(), 1);
    policy
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .verify = Some(false);
    policy = parse_health_policy(&handler, policy, &ctx).await;
    handler.update_status(&mut policy, &ctx, &[]);
    assert_eq!(
        ancestor_condition(&policy, "gw", condition_types::ACCEPTED).status,
        "False"
    );
    assert_eq!(
        policy.spec.resolved_target_class,
        Some(ResolvedPolicyTargetClass::Service)
    );
    assert!(runtime
        .edgion_backend_traffic_policy
        .is_winner("ns/health", "/Service/ns/payments"));
    assert!(health_active(&policy).resolved_ca_certificates.is_none());
    assert!(ctx.secret_ref_manager().get_refs("ns/replacement").is_empty());

    policy.spec.health_check = None;
    let _ = parse_health_policy(&handler, policy, &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("ns/health-ca").is_empty());
}

#[tokio::test]
async fn encrypted_health_cross_namespace_references_are_rejected_without_resolution() {
    let (ctx, runtime) = make_ctx();
    let handler = make_handler();
    install_health_material(&runtime, "Secret");
    let mut policy = encrypted_policy("Secret");
    policy
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()[0]
        .namespace = Some("other".into());
    let policy = parse_health_policy(&handler, policy, &ctx).await;
    assert!(health_active(&policy).resolved_ca_certificates.is_none());
    assert_eq!(
        health_active(&policy).resolved_tls_error.as_deref(),
        Some("health check TLS references must use the policy namespace")
    );
    assert!(!handler.validate(&policy, &ctx).is_empty());
    assert_eq!(
        policy.spec.resolved_target_class,
        Some(ResolvedPolicyTargetClass::Service)
    );
}

#[tokio::test]
async fn invalid_identity_withdraws_whole_policy_but_keeps_valid_recovery_target() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use k8s_openapi::api::core::v1::Service;
    let (ctx, runtime) = make_ctx();
    let handler = make_handler();
    let processor = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        16,
        Arc::new(ServiceHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    wire_service_to_gateway(&runtime, "default", "valid", "route", "gateway");
    let mut source = make_policy("bare", "valid", "2026-01-01T00:00:00Z");
    source.metadata.namespace = None;
    let owner = EdgionBackendTrafficPolicyHandler::resource_ref(&source);
    let mut invalid = source.spec.target_refs[0].clone();
    invalid.name = "bad/name".into();
    source.spec.target_refs.push(invalid);
    let evaluation = evaluate_policy(&source, &ctx);
    assert!(!evaluation.errors.is_empty());
    assert!(
        evaluation.missing_targets.is_empty(),
        "identity failure returns before even valid sibling existence reads"
    );
    let ProcessResult::Continue(mut parsed) = handler.parse(source, &ctx).await else {
        panic!()
    };
    assert!(parsed.spec.resolved_target_class.is_none());
    assert!(runtime
        .edgion_backend_traffic_policy
        .target_mgr()
        .get_dependencies(&owner.key())
        .is_empty());
    assert_eq!(
        runtime.service_ref_mgr.get_dependencies(&owner.key()),
        ["default/valid"]
    );
    handler.update_status(&mut parsed, &ctx, &[]);
    let accepted = ancestor_condition(&parsed, "gateway", "Accepted");
    assert_eq!((&*accepted.status, &*accepted.reason), ("False", "Invalid"));
    parsed.spec.target_refs.pop();
    let ProcessResult::Continue(mut pending) = handler.parse(parsed, &ctx).await else {
        panic!()
    };
    handler.update_status(&mut pending, &ctx, &[]);
    assert_eq!(
        ancestor_condition(&pending, "gateway", "ResolvedRefs").reason,
        "TargetNotFound"
    );
    raw.upsert(
        "default/valid".into(),
        Arc::new(serde_json::from_value(json!({"metadata":{"name":"valid","namespace":"default"},"spec":{}})).unwrap()),
    );
    let ProcessResult::Continue(mut repaired) = handler.parse(pending, &ctx).await else {
        panic!()
    };
    handler.update_status(&mut repaired, &ctx, &[]);
    assert_eq!(ancestor_condition(&repaired, "gateway", "Accepted").status, "True");
    assert!(runtime.edgion_backend_traffic_policy.is_winner(
        "bare",
        &PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "default".into(),
            name: "valid".into()
        }
        .to_string()
    ));
    repaired.spec.target_refs[0].name = "next".into();
    let ProcessResult::Continue(replaced) = handler.parse(repaired, &ctx).await else {
        panic!()
    };
    assert_eq!(runtime.service_ref_mgr.get_dependencies(&owner.key()), ["default/next"]);
    handler.on_delete(&replaced, &ctx).await;
    assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());
}

#[test]
fn bare_tls_owner_participates_in_default_service_authority_validation() {
    let policy = serde_json::from_value(json!({"metadata":{"name":"bare"},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],"validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}})).unwrap();
    let records = vec![BackendTlsPolicyRecord {
        key: "bare".into(),
        policy: Arc::new(policy),
    }];
    assert_eq!(
        backend_tls_hostnames_for_service("default", "backend", &records),
        [("bare", "backend.example")]
    );
    assert!(backend_tls_hostnames_for_service("other", "backend", &records).is_empty());
}

#[tokio::test]
async fn actual_health_invalid_owner_and_inactive_protocol_scrub_without_material_effects() {
    use edgion_resources::resources::health_check::HealthCheckType;
    for missing_owner in [false, true] {
        let (ctx, runtime) = make_ctx();
        install_health_material(&runtime, "Secret");
        let handler = make_handler();
        let mut policy = parse_health_policy(&handler, encrypted_policy("Secret"), &ctx).await;
        assert!(health_active(&policy).resolved_client_certificate.is_some());
        if missing_owner {
            policy.metadata.name = None;
        } else {
            policy
                .spec
                .health_check
                .as_mut()
                .unwrap()
                .active
                .as_mut()
                .unwrap()
                .r#type = HealthCheckType::Http;
        }
        let owner = EdgionBackendTrafficPolicyHandler::resource_ref(&policy);
        let (parsed, reads) = super::super::super::credential::pipeline::observe_bound_reads(|| {
            resolve_health_tls(&mut policy, &ctx, &owner);
            policy
        });
        assert!(reads.is_empty());
        assert!(health_active(&parsed).resolved_ca_certificates.is_none());
        assert!(health_active(&parsed).resolved_client_certificate.is_none());
        assert_eq!(health_active(&parsed).resolved_tls_error.is_some(), missing_owner);
        assert!(runtime.secret_ref_manager.get_dependencies(&owner.key()).is_empty());
    }
}
