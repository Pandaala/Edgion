use super::*;
use crate::backends::degrade_threshold::{configure_lb_degrade_default, resolve_degrade_threshold};
use edgion_resources::resources::edgion_backend_traffic_policy::{
    EdgionBackendTrafficPolicySpec, LoadBalancerConfig, LoadBalancerType, PolicyTargetRef,
};
use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
use edgion_resources::resources::outlier_detection::OutlierDetectionConfig;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

fn ts(rfc3339: &str) -> Time {
    Time(rfc3339.parse().expect("rfc3339"))
}

/// Build a policy in `ns` named `name`, targeting Service `svc`, with the
/// given LB type, degrade threshold, health-check and outlier-detection flags.
#[allow(clippy::too_many_arguments)]
fn policy(
    ns: &str,
    name: &str,
    svc: &str,
    creation: Option<Time>,
    lb_type: Option<LoadBalancerType>,
    degrade_threshold: Option<u8>,
    with_hc: bool,
    with_outlier: bool,
) -> Arc<EdgionBackendTrafficPolicy> {
    let load_balancer = lb_type.map(|t| LoadBalancerConfig {
        r#type: t,
        consistent_hash: None,
        degrade_threshold,
    });
    let health_check = with_hc.then(|| ServiceHealthCheck {
        active: Some(ActiveHealthCheckConfig::default()),
    });
    let outlier_detection = with_outlier.then_some(OutlierDetectionConfig {
        consecutive_errors: 3,
        consecutive_gateway_errors: None,
        consecutive_local_origin_failures: None,
        ejection_time: "30s".to_string(),
        max_ejection_time: None,
        max_ejection_percent: 50,
    });
    Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            creation_timestamp: creation,
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: svc.to_string(),
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
            retry_constraint: None,
            load_balancer,
            health_check,
            outlier_detection,
            upstream_authority: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    })
}

fn ai_policy(ns: &str, name: &str, target: &str, percent: u8, timeout: &str) -> Arc<EdgionBackendTrafficPolicy> {
    use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;
    use edgion_resources::resources::edgion_backend_traffic_policy::{
        BudgetDetails, ConnectionOverride, RetryConstraintConfig,
    };

    Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            creation_timestamp: Some(ts("2026-01-01T00:00:00Z")),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some("edgion.io".to_string()),
                kind: "EdgionBackend".to_string(),
                name: target.to_string(),
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Ai),
            retry_constraint: Some(RetryConstraintConfig {
                budget: BudgetDetails {
                    percent,
                    interval: "10s".to_string(),
                },
                min_retry_rate: None,
            }),
            load_balancer: None,
            health_check: None,
            outlier_detection: Some(OutlierDetectionConfig {
                consecutive_errors: 4,
                consecutive_gateway_errors: Some(5),
                consecutive_local_origin_failures: Some(6),
                ejection_time: "30s".to_string(),
                max_ejection_time: Some("5m".to_string()),
                max_ejection_percent: 50,
            }),
            upstream_authority: None,
            circuit_breaker: Some(CircuitBreakerConfig {
                max_parallel_requests: 7,
            }),
            connection: Some(ConnectionOverride {
                connect_timeout: Some(timeout.to_string()),
            }),
        },
        status: None,
    })
}

fn service_target(ns: &str, name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: ns.to_string(),
        name: name.to_string(),
    }
}

#[test]
fn single_policy_populates_all_stores() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6a/svc";
    store.full_set_policies(HashMap::from([(
        "bt6a/p1".to_string(),
        policy(
            "bt6a",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(40),
            true,
            true,
        ),
    )]));

    assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));
    assert_eq!(resolve_degrade_threshold(key), 40);
    assert!(get_hc_config_store().resolve_policy(key).is_some(), "HC config present");
    assert!(
        get_outlier_detection_store().keys().contains(&key.to_string()),
        "CB config present"
    );
    assert_eq!(
        store
            .get_effective(&service_target("bt6a", "svc"))
            .expect("effective")
            .winning_policy_key,
        "bt6a/p1"
    );

    store.full_set_policies(HashMap::new());
}

#[test]
fn oldest_wins_then_promotes_on_delete() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6b/svc";
    let older = policy(
        "bt6b",
        "older",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::Ewma),
        None,
        false,
        false,
    );
    let newer = policy(
        "bt6b",
        "newer",
        "svc",
        Some(ts("2026-06-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    store.full_set_policies(HashMap::from([
        ("bt6b/older".to_string(), older),
        ("bt6b/newer".to_string(), newer),
    ]));
    assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::Ewma), "older wins");

    store.partial_update_policies(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["bt6b/older".to_string()]),
    );
    assert_eq!(
        store.get_lb_policy(key),
        Some(ParsedLBPolicy::LeastConn),
        "newer promoted after older deleted"
    );

    store.full_set_policies(HashMap::new());
}

#[test]
fn last_removal_clears_all_stores() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6c/svc";
    store.full_set_policies(HashMap::from([(
        "bt6c/p1".to_string(),
        policy(
            "bt6c",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(55),
            true,
            true,
        ),
    )]));
    assert!(store.get_lb_policy(key).is_some());

    store.partial_update_policies(HashMap::new(), HashMap::new(), HashSet::from(["bt6c/p1".to_string()]));

    assert_eq!(store.get_lb_policy(key), None, "LB entry removed");
    assert_eq!(
        resolve_degrade_threshold(key),
        0,
        "degrade override cleared to global default"
    );
    assert!(get_hc_config_store().resolve_policy(key).is_none(), "HC config cleared");
    assert!(
        !get_outlier_detection_store().keys().contains(&key.to_string()),
        "CB config cleared"
    );
}

#[test]
fn reapply_reports_change_on_lb_flip() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6e/svc";
    let first = policy(
        "bt6e",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    assert_eq!(store.snapshot().generation(), 0);
    store.full_set_policies(HashMap::from([("bt6e/p1".to_string(), first.clone())]));
    assert_eq!(store.snapshot().generation(), 1, "first apply is a change");
    assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));

    store.full_set_policies(HashMap::from([("bt6e/p1".to_string(), first)]));
    assert_eq!(store.snapshot().generation(), 1, "equivalent republish does not bump");

    store.full_set_policies(HashMap::from([(
        "bt6e/p1".to_string(),
        policy(
            "bt6e",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        ),
    )]));
    assert_eq!(store.snapshot().generation(), 2, "LB flip is a change");
    assert_eq!(
        store.get_lb_policy(key),
        Some(ParsedLBPolicy::Ewma),
        "store entry flipped"
    );

    store.full_set_policies(HashMap::new());
}

#[test]
fn invalid_policy_is_skipped() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6d/svc";
    let invalid = Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some("bad".to_string()),
            namespace: Some("bt6d".to_string()),
            creation_timestamp: Some(ts("2026-01-01T00:00:00Z")),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: "svc".to_string(),
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
            retry_constraint: None,
            load_balancer: Some(LoadBalancerConfig {
                r#type: LoadBalancerType::ConsistentHash,
                consistent_hash: None,
                degrade_threshold: None,
            }),
            health_check: None,
            outlier_detection: None,
            upstream_authority: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    });
    assert!(
        !invalid.spec.get_validation_errors().is_empty(),
        "fixture must be invalid"
    );
    store.full_set_policies(HashMap::from([("bt6d/bad".to_string(), invalid)]));
    assert_eq!(store.get_lb_policy(key), None, "invalid policy contributes nothing");
    assert!(get_hc_config_store().resolve_policy(key).is_none());
    assert!(
        store.snapshot().candidates(&service_target("bt6d", "svc")).is_empty(),
        "invalid policy is absent from reverse_index"
    );
}

#[test]
fn health_check_zero_port_is_ineligible_and_clears_previous_runtime_config() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let service_key = "health-port/svc";
    let policy_key = "health-port/check".to_string();
    let mut check = policy(
        "health-port",
        "check",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::Ewma),
        None,
        true,
        false,
    );
    Arc::make_mut(&mut check)
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .port = Some(0);
    // Keep a forged valid target marker: the Gateway must independently reject zero.
    store.full_set_policies(HashMap::from([(policy_key.clone(), check.clone())]));
    assert!(store.get_effective(&service_target("health-port", "svc")).is_none());
    assert!(get_hc_config_store().resolve_policy(service_key).is_none());

    for port in [Some(1), Some(0), None, Some(65535)] {
        Arc::make_mut(&mut check)
            .spec
            .health_check
            .as_mut()
            .unwrap()
            .active
            .as_mut()
            .unwrap()
            .port = port;
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([(policy_key.clone(), check.clone())]),
            HashSet::new(),
        );
        let valid = port != Some(0);
        assert_eq!(store.get_lb_policy(service_key), valid.then_some(ParsedLBPolicy::Ewma));
        assert_eq!(
            store
                .snapshot()
                .candidates(&service_target("health-port", "svc"))
                .is_empty(),
            !valid
        );
        let health = get_hc_config_store().resolve_policy(service_key);
        assert_eq!(health.is_some(), valid);
        if let Some(health) = health {
            assert_eq!(health.config().port, port);
        }
    }
    store.full_set_policies(HashMap::new());
}

#[test]
fn resolved_marker_withdrawal_promotes_sibling_and_restore_reinstates_oldest() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let target = "marker/svc";
    let older = policy(
        "marker",
        "older",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    let newer = policy(
        "marker",
        "newer",
        "svc",
        Some(ts("2026-02-01T00:00:00Z")),
        Some(LoadBalancerType::Ewma),
        None,
        false,
        false,
    );
    store.full_set_policies(HashMap::from([
        ("marker/older".to_string(), older.clone()),
        ("marker/newer".to_string(), newer),
    ]));
    assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::LeastConn));

    let mut withdrawn = (*older).clone();
    withdrawn.spec.resolved_target_class = None;
    store.partial_update_policies(
        HashMap::new(),
        HashMap::from([("marker/older".to_string(), Arc::new(withdrawn))]),
        HashSet::new(),
    );
    assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::Ewma));

    store.partial_update_policies(
        HashMap::new(),
        HashMap::from([("marker/older".to_string(), older)]),
        HashSet::new(),
    );
    assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::LeastConn));
}

#[test]
fn forged_mismatched_resolved_marker_contributes_nothing() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let mut forged = (*policy(
        "marker-mismatch",
        "forged",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    ))
    .clone();
    forged.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Ai);
    assert_eq!(
        store.full_set_policies(HashMap::from([(
            "marker-mismatch/forged".to_string(),
            Arc::new(forged),
        )])),
        1,
        "the typed target is affected even though the forged marker publishes nothing"
    );
    assert_eq!(store.get_lb_policy("marker-mismatch/svc"), None);
}

#[test]
fn full_set_policies_clears_stale_and_populates_new() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let store = BackendTrafficPolicyStore::new();
    let key_a = "bt6f/svca";
    let key_b = "bt6g/svcb";

    let mut initial = HashMap::new();
    initial.insert(
        "bt6f/p1".to_string(),
        policy(
            "bt6f",
            "p1",
            "svca",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(40),
            true,
            true,
        ),
    );
    store.full_set_policies(initial);

    assert_eq!(store.get_lb_policy(key_a), Some(ParsedLBPolicy::LeastConn));
    assert_eq!(resolve_degrade_threshold(key_a), 40);
    assert!(get_hc_config_store().resolve_policy(key_a).is_some(), "A HC present");
    assert!(
        get_outlier_detection_store().keys().contains(&key_a.to_string()),
        "A CB present"
    );

    let mut next = HashMap::new();
    next.insert(
        "bt6g/p2".to_string(),
        policy(
            "bt6g",
            "p2",
            "svcb",
            Some(ts("2026-02-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            Some(60),
            true,
            true,
        ),
    );
    store.full_set_policies(next);

    assert_eq!(store.get_lb_policy(key_a), None, "stale A LB cleared");
    assert_eq!(resolve_degrade_threshold(key_a), 0, "falls back to global default");
    assert!(get_hc_config_store().resolve_policy(key_a).is_none(), "A HC cleared");
    assert!(
        !get_outlier_detection_store().keys().contains(&key_a.to_string()),
        "A CB cleared"
    );

    assert_eq!(store.get_lb_policy(key_b), Some(ParsedLBPolicy::Ewma), "B LB set");
    assert_eq!(resolve_degrade_threshold(key_b), 60);
    assert!(get_hc_config_store().resolve_policy(key_b).is_some(), "B HC present");
    assert!(
        get_outlier_detection_store().keys().contains(&key_b.to_string()),
        "B CB present"
    );

    store.full_set_policies(HashMap::new());
    assert_eq!(store.get_lb_policy(key_b), None, "B LB cleared on empty resync");
    assert_eq!(resolve_degrade_threshold(key_b), 0, "B falls back to global default");
    assert!(get_hc_config_store().resolve_policy(key_b).is_none(), "B HC cleared");
    assert!(
        !get_outlier_detection_store().keys().contains(&key_b.to_string()),
        "B CB cleared"
    );
}

#[test]
fn upstream_authority_resolves_and_clears() {
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6h/svc";

    let mut p = policy(
        "bt6h",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
        pattern: "*.sandbox.internal".to_string(),
        template: "${ctx:tenant}.sandbox.internal".to_string(),
        health_check_host: None,
    });
    store.full_set_policies(HashMap::from([("bt6h/p1".to_string(), p)]));
    let resolved = store.get_authority(key).expect("authority resolved");
    assert_eq!(resolved.suffix, ".sandbox.internal");
    assert_eq!(resolved.compose("tenant-a"), "tenant-a.sandbox.internal");

    store.full_set_policies(HashMap::new());
    assert!(store.get_authority(key).is_none(), "cleared on removal");
}

#[test]
fn retry_budget_resolves_with_keep_if_unchanged() {
    use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6i/svc";

    let mut p = policy(
        "bt6i",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p).unwrap().spec.retry_constraint = Some(RetryConstraintConfig {
        budget: BudgetDetails {
            percent: 20,
            interval: "10s".to_string(),
        },
        min_retry_rate: None,
    });
    store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p.clone())]));
    let first = store.get_retry_budget(key).expect("retry budget resolved");

    store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p)]));
    let second = store.get_retry_budget(key).expect("retry budget still present");
    assert!(
        Arc::ptr_eq(&first, &second),
        "unchanged config must keep the same Arc (live counters preserved)"
    );

    let mut p2 = policy(
        "bt6i",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p2).unwrap().spec.retry_constraint = Some(RetryConstraintConfig {
        budget: BudgetDetails {
            percent: 50,
            interval: "10s".to_string(),
        },
        min_retry_rate: None,
    });
    store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p2)]));
    let third = store.get_retry_budget(key).expect("retry budget resolved after change");
    assert!(!Arc::ptr_eq(&second, &third), "changed config must build a new Arc");

    store.full_set_policies(HashMap::new());
    assert!(store.get_retry_budget(key).is_none(), "cleared on removal");
}

#[test]
fn circuit_breaker_limit_resolves_and_clears() {
    use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6j/svc";

    let mut p = policy(
        "bt6j",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p).unwrap().spec.circuit_breaker = Some(CircuitBreakerConfig {
        max_parallel_requests: 5,
    });
    store.full_set_policies(HashMap::from([("bt6j/p1".to_string(), p)]));
    assert_eq!(store.get_circuit_breaker_limit(key), Some(5));

    store.full_set_policies(HashMap::new());
    assert_eq!(store.get_circuit_breaker_limit(key), None, "cleared on removal");
}

#[test]
fn connect_timeout_resolves_and_clears() {
    use edgion_resources::resources::edgion_backend_traffic_policy::ConnectionOverride;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key = "bt6k/svc";

    let mut p = policy(
        "bt6k",
        "p1",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p).unwrap().spec.connection = Some(ConnectionOverride {
        connect_timeout: Some("250ms".to_string()),
    });
    store.full_set_policies(HashMap::from([("bt6k/p1".to_string(), p)]));
    assert_eq!(
        store.get_connect_timeout(key),
        Some(std::time::Duration::from_millis(250))
    );

    store.full_set_policies(HashMap::new());
    assert_eq!(store.get_connect_timeout(key), None, "cleared on removal");
}

#[test]
fn typed_service_and_ai_targets_with_same_name_do_not_collide() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let service_target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "typed".to_string(),
        name: "shared".to_string(),
    };
    let ai_target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "typed".to_string(),
        name: "shared".to_string(),
    };

    let mut policies = HashMap::new();
    policies.insert(
        "typed/service-policy".to_string(),
        policy(
            "typed",
            "service-policy",
            "shared",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        ),
    );
    policies.insert(
        "typed/ai-policy".to_string(),
        ai_policy("typed", "ai-policy", "shared", 25, "750ms"),
    );
    assert_eq!(store.full_set_policies(policies), 2);

    assert_eq!(store.get_lb_policy("typed/shared"), Some(ParsedLBPolicy::LeastConn));
    let service = store.get_effective(&service_target).expect("Service snapshot");
    assert!(service.retry_budget.is_none());
    assert!(service.ai_credential_outlier.is_none());
    assert_eq!(service.winning_policy_key, "typed/service-policy");

    let ai = store.get_effective(&ai_target).expect("AI snapshot");
    assert!(ai.ai_credential_outlier.is_some());
    assert!(ai.retry_budget.is_some());
    assert_eq!(ai.circuit_breaker_limit, Some(7));
    assert_eq!(ai.connect_timeout, Some(std::time::Duration::from_millis(750)));
    let outlier = ai.outlier_detection.as_ref().expect("AI outlier config");
    assert_eq!(outlier.consecutive_errors, 4);
    assert_eq!(outlier.consecutive_gateway_errors, 5);
    assert_eq!(outlier.consecutive_local_origin_failures, 6);
    assert_eq!(ai.winning_policy_key, "typed/ai-policy");
}

#[test]
fn equivalent_ai_update_keeps_retry_handle_and_field_update_is_atomic() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "typed-update".to_string(),
        name: "models".to_string(),
    };
    let policy_key = "typed-update/policy".to_string();

    let mut initial = HashMap::new();
    initial.insert(
        policy_key.clone(),
        ai_policy("typed-update", "policy", "models", 20, "250ms"),
    );
    store.full_set_policies(initial);
    let first_snapshot = store.snapshot();
    let first = first_snapshot.get_effective(&target).expect("initial snapshot");
    let first_budget = first.retry_budget.as_ref().expect("retry budget");
    let first_outlier = first.ai_credential_outlier.as_ref().expect("AI outlier generation");

    let mut equivalent = HashMap::new();
    equivalent.insert(
        policy_key.clone(),
        ai_policy("typed-update", "policy", "models", 20, "250ms"),
    );
    store.full_set_policies(equivalent);
    let equivalent_snapshot = store.snapshot();
    let second = equivalent_snapshot.get_effective(&target).expect("equivalent snapshot");
    assert_eq!(first_snapshot.generation(), equivalent_snapshot.generation());
    assert!(Arc::ptr_eq(&first, &second), "equivalent outer Arc retained");
    assert!(Arc::ptr_eq(
        first_budget,
        second.retry_budget.as_ref().expect("retry budget retained")
    ));
    assert!(Arc::ptr_eq(
        first_outlier,
        second
            .ai_credential_outlier
            .as_ref()
            .expect("outlier generation retained")
    ));

    let mut changed = HashMap::new();
    changed.insert(policy_key, ai_policy("typed-update", "policy", "models", 20, "2s"));
    store.full_set_policies(changed);
    let third = store.get_effective(&target).expect("updated snapshot");
    assert_eq!(third.connect_timeout, Some(std::time::Duration::from_secs(2)));
    assert_eq!(third.circuit_breaker_limit, Some(7));
    assert!(third.outlier_detection.is_some());
    assert!(Arc::ptr_eq(
        second.retry_budget.as_ref().unwrap(),
        third.retry_budget.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        second.ai_credential_outlier.as_ref().unwrap(),
        third.ai_credential_outlier.as_ref().unwrap()
    ));

    let mut outlier_changed = ai_policy("typed-update", "policy", "models", 20, "2s");
    Arc::get_mut(&mut outlier_changed)
        .unwrap()
        .spec
        .outlier_detection
        .as_mut()
        .unwrap()
        .consecutive_errors = 8;
    store.full_set_policies(HashMap::from([("typed-update/policy".to_string(), outlier_changed)]));
    let fourth = store.get_effective(&target).expect("outlier-updated snapshot");
    assert!(!Arc::ptr_eq(
        third.ai_credential_outlier.as_ref().unwrap(),
        fourth.ai_credential_outlier.as_ref().unwrap()
    ));

    store.full_set_policies(HashMap::new());
    assert!(store.get_effective(&target).is_none());
    assert!(get_ai_credential_outlier_store()
        .effective_handle_for_test(&target)
        .is_none());
    assert_eq!(first.connect_timeout, Some(std::time::Duration::from_millis(250)));
}

#[test]
fn multi_target_reconcile_publishes_one_coherent_generation() {
    let store = BackendTrafficPolicyStore::new();
    let target_a = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "batch".to_string(),
        name: "a".to_string(),
    };
    let target_b = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "batch".to_string(),
        name: "b".to_string(),
    };
    let before = store.snapshot();
    assert_eq!(before.generation(), 0);

    store.full_set_policies(HashMap::from([
        (
            "batch/policy-a".to_string(),
            ai_policy("batch", "policy-a", "a", 20, "100ms"),
        ),
        (
            "batch/policy-b".to_string(),
            ai_policy("batch", "policy-b", "b", 20, "200ms"),
        ),
    ]));
    let after = store.snapshot();
    assert_eq!(after.generation(), 1, "one batch yields one publication");
    assert_eq!(
        after.get_effective(&target_a).unwrap().connect_timeout,
        Some(std::time::Duration::from_millis(100))
    );
    assert_eq!(
        after.get_effective(&target_b).unwrap().connect_timeout,
        Some(std::time::Duration::from_millis(200))
    );
    assert!(before.get_effective(&target_a).is_none(), "old handle is immutable");
}

#[test]
fn multi_target_partial_update_publishes_both_ai_targets_atomically() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let target_a = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "partial-batch".to_string(),
        name: "a".to_string(),
    };
    let target_b = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "partial-batch".to_string(),
        name: "b".to_string(),
    };
    let policy_key = "partial-batch/policy".to_string();

    let mut initial = ai_policy("partial-batch", "policy", "a", 20, "100ms");
    Arc::get_mut(&mut initial)
        .unwrap()
        .spec
        .target_refs
        .push(PolicyTargetRef {
            group: Some("edgion.io".to_string()),
            kind: "EdgionBackend".to_string(),
            name: "b".to_string(),
        });
    assert_eq!(
        store.partial_update_policies(
            HashMap::from([(policy_key.clone(), initial)]),
            HashMap::new(),
            HashSet::new(),
        ),
        2
    );
    let first = store.snapshot();
    assert_eq!(first.generation(), 1, "one multi-target add yields one publication");
    assert_eq!(
        first.get_effective(&target_a).unwrap().connect_timeout,
        Some(std::time::Duration::from_millis(100))
    );
    assert_eq!(
        first.get_effective(&target_b).unwrap().connect_timeout,
        Some(std::time::Duration::from_millis(100))
    );

    let mut updated = ai_policy("partial-batch", "policy", "a", 20, "2s");
    Arc::get_mut(&mut updated)
        .unwrap()
        .spec
        .target_refs
        .push(PolicyTargetRef {
            group: Some("edgion.io".to_string()),
            kind: "EdgionBackend".to_string(),
            name: "b".to_string(),
        });
    assert_eq!(
        store.partial_update_policies(HashMap::new(), HashMap::from([(policy_key, updated)]), HashSet::new(),),
        2
    );
    let second = store.snapshot();
    assert_eq!(second.generation(), 2, "one multi-target update yields one publication");
    for target in [&target_a, &target_b] {
        assert_eq!(
            second.get_effective(target).unwrap().connect_timeout,
            Some(std::time::Duration::from_secs(2))
        );
        assert_eq!(
            first.get_effective(target).unwrap().connect_timeout,
            Some(std::time::Duration::from_millis(100)),
            "the captured pre-update generation stays immutable"
        );
    }
}

#[test]
fn lb_only_update_bumps_generation_exactly_once() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    store.full_set_policies(HashMap::from([(
        "gen/p".to_string(),
        policy(
            "gen",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        ),
    )]));
    let first = store.snapshot();
    store.full_set_policies(HashMap::from([(
        "gen/p".to_string(),
        policy(
            "gen",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        ),
    )]));
    let second = store.snapshot();
    assert_eq!(second.generation(), first.generation() + 1);
    store.full_set_policies(HashMap::from([(
        "gen/p".to_string(),
        policy(
            "gen",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        ),
    )]));
    assert_eq!(store.snapshot().generation(), second.generation());
    store.full_set_policies(HashMap::new());
}

#[test]
fn authority_only_update_bumps_generation_exactly_once() {
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let mut first = policy(
        "auth",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut first).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
        pattern: "*.a.internal".to_string(),
        template: "${ctx:tenant}.a.internal".to_string(),
        health_check_host: None,
    });
    store.full_set_policies(HashMap::from([("auth/p".to_string(), first)]));
    let g1 = store.snapshot().generation();

    let mut second = policy(
        "auth",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        false,
        false,
    );
    Arc::get_mut(&mut second).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
        pattern: "*.b.internal".to_string(),
        template: "${ctx:tenant}.b.internal".to_string(),
        health_check_host: None,
    });
    store.full_set_policies(HashMap::from([("auth/p".to_string(), second.clone())]));
    assert_eq!(store.snapshot().generation(), g1 + 1);
    store.full_set_policies(HashMap::from([("auth/p".to_string(), second)]));
    assert_eq!(store.snapshot().generation(), g1 + 1);
    store.full_set_policies(HashMap::new());
}

#[test]
fn degrade_override_only_update_bumps_generation() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    store.full_set_policies(HashMap::from([(
        "degrade/p".to_string(),
        policy(
            "degrade",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(20),
            false,
            false,
        ),
    )]));
    let g1 = store.snapshot().generation();
    store.full_set_policies(HashMap::from([(
        "degrade/p".to_string(),
        policy(
            "degrade",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(80),
            false,
            false,
        ),
    )]));
    assert_eq!(store.snapshot().generation(), g1 + 1);
    store.full_set_policies(HashMap::new());
}

#[test]
fn equivalent_full_and_partial_do_not_bump() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let p = policy(
        "eq",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    store.full_set_policies(HashMap::from([("eq/p".to_string(), p.clone())]));
    let g1 = store.snapshot().generation();
    publish_counters::reset();
    store.full_set_policies(HashMap::from([("eq/p".to_string(), p.clone())]));
    store.partial_update_policies(HashMap::new(), HashMap::from([("eq/p".to_string(), p)]), HashSet::new());
    assert_eq!(store.snapshot().generation(), g1);
    assert_eq!(
        publish_counters::SIDE_EFFECT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    store.full_set_policies(HashMap::new());
}

#[test]
fn real_change_full_and_partial_match() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let initial = policy(
        "match",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    let updated = policy(
        "match",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::Ewma),
        None,
        false,
        false,
    );
    let full = BackendTrafficPolicyStore::new();
    full.full_set_policies(HashMap::from([("match/p".to_string(), updated.clone())]));
    let partial = BackendTrafficPolicyStore::new();
    partial.full_set_policies(HashMap::from([("match/p".to_string(), initial)]));
    partial.partial_update_policies(
        HashMap::new(),
        HashMap::from([("match/p".to_string(), updated)]),
        HashSet::new(),
    );
    let target = service_target("match", "svc");
    let left = full.get_effective(&target).expect("full");
    let right = partial.get_effective(&target).expect("partial");
    assert_eq!(left.winning_policy_key, right.winning_policy_key);
    assert_eq!(left.load_balancer, right.load_balancer);
    assert_eq!(
        full.snapshot().candidates(&target).len(),
        partial.snapshot().candidates(&target).len()
    );
    full.full_set_policies(HashMap::new());
    partial.full_set_policies(HashMap::new());
}

#[test]
fn retarget_clears_reverse_index_and_side_effects() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let store = BackendTrafficPolicyStore::new();
    store.full_set_policies(HashMap::from([(
        "rt/p".to_string(),
        policy(
            "rt",
            "p",
            "a",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            Some(40),
            true,
            true,
        ),
    )]));
    assert!(store.get_lb_policy("rt/a").is_some());
    store.partial_update_policies(
        HashMap::new(),
        HashMap::from([(
            "rt/p".to_string(),
            policy(
                "rt",
                "p",
                "b",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                Some(60),
                true,
                true,
            ),
        )]),
        HashSet::new(),
    );
    assert!(store.snapshot().candidates(&service_target("rt", "a")).is_empty());
    assert_eq!(store.get_lb_policy("rt/a"), None);
    assert!(get_hc_config_store().resolve_policy("rt/a").is_none());
    assert_eq!(store.get_lb_policy("rt/b"), Some(ParsedLBPolicy::Ewma));
    assert!(!store.snapshot().candidates(&service_target("rt", "b")).is_empty());
    store.full_set_policies(HashMap::new());
}

#[test]
fn prepare_policy_dedups_repeated_service_target_refs() {
    let mut p = policy(
        "dup",
        "p",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    Arc::get_mut(&mut p).unwrap().spec.target_refs.push(PolicyTargetRef {
        group: Some(String::new()),
        kind: "Service".to_string(),
        name: "svc".to_string(),
    });
    let prepared = BackendTrafficPolicyStore::prepare_policy("dup/p".to_string(), p);
    assert_eq!(prepared.target_keys.len(), 1);
    assert!(prepared.eligible);
}

#[test]
fn overlapping_deltas_use_only_previous_and_final_policy_ownership() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    for removed in [false, true] {
        let store = BackendTrafficPolicyStore::new();
        let make = |target| {
            policy(
                "overlap-ebtp",
                "p",
                target,
                None,
                Some(LoadBalancerType::LeastConn),
                Some(40),
                true,
                true,
            )
        };
        store.full_set_policies(HashMap::from([("overlap-ebtp/p".into(), make("a"))]));
        let pinned = store.snapshot();
        publish_counters::reset();
        let affected = store.partial_update_policies(
            HashMap::from([("overlap-ebtp/p".into(), make("b"))]),
            HashMap::from([("overlap-ebtp/p".into(), make("c"))]),
            if removed {
                HashSet::from(["overlap-ebtp/p".into()])
            } else {
                HashSet::new()
            },
        );
        assert_eq!(affected, if removed { 1 } else { 2 });
        assert_eq!(
            publish_counters::PREPARE_POLICY_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!removed)
        );
        assert_eq!(
            publish_counters::FULL_INDEX_BUILDS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            publish_counters::SIDE_EFFECT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(store.snapshot().generation(), pinned.generation() + 1);
        for name in ["a", "b"] {
            assert!(store
                .snapshot()
                .candidates(&service_target("overlap-ebtp", name))
                .is_empty());
            assert!(store.get_effective(&service_target("overlap-ebtp", name)).is_none());
            let key = format!("overlap-ebtp/{name}");
            assert!(get_hc_config_store().resolve_policy(&key).is_none());
            assert!(!get_outlier_detection_store().keys().contains(&key));
            assert_eq!(resolve_degrade_threshold(&key), 0);
        }
        let final_target = service_target("overlap-ebtp", "c");
        assert_eq!(store.snapshot().candidates(&final_target).len(), usize::from(!removed));
        assert_eq!(store.get_effective(&final_target).is_some(), !removed);
        assert_eq!(
            get_hc_config_store().resolve_policy("overlap-ebtp/c").is_some(),
            !removed
        );
        assert_eq!(
            get_outlier_detection_store().keys().contains(&"overlap-ebtp/c".into()),
            !removed
        );
        assert_eq!(
            resolve_degrade_threshold("overlap-ebtp/c"),
            if removed { 0 } else { 40 }
        );
        assert!(pinned.get_effective(&service_target("overlap-ebtp", "a")).is_some());
        store.full_set_policies(HashMap::new());
        let generation = store.snapshot().generation();
        publish_counters::reset();
        assert_eq!(
            store.partial_update_policies(
                HashMap::new(),
                HashMap::new(),
                HashSet::from(["overlap-ebtp/missing".into()])
            ),
            0
        );
        assert_eq!(store.snapshot().generation(), generation);
        assert_eq!(
            publish_counters::SIDE_EFFECT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}

#[test]
fn invalid_replacement_add_withdraws_previous_winner_and_promotes_sibling() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let store = BackendTrafficPolicyStore::new();
    let first = policy(
        "invalid-add",
        "first",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        Some(LoadBalancerType::LeastConn),
        Some(40),
        true,
        true,
    );
    let second = policy(
        "invalid-add",
        "second",
        "svc",
        Some(ts("2026-02-01T00:00:00Z")),
        Some(LoadBalancerType::Ewma),
        Some(60),
        true,
        true,
    );
    store.full_set_policies(HashMap::from([
        ("invalid-add/first".into(), first.clone()),
        ("invalid-add/second".into(), second.clone()),
    ]));
    let old = store.snapshot();
    for (key, policy, remaining) in [("invalid-add/first", first, 1), ("invalid-add/second", second, 0)] {
        let mut invalid = (*policy).clone();
        invalid.spec.resolved_target_class = None;
        store.partial_update_policies(
            HashMap::from([(key.into(), Arc::new(invalid))]),
            HashMap::new(),
            HashSet::new(),
        );
        let target = service_target("invalid-add", "svc");
        assert_eq!(store.snapshot().candidates(&target).len(), remaining);
        assert_eq!(
            get_hc_config_store().resolve_policy("invalid-add/svc").is_some(),
            remaining > 0
        );
        assert_eq!(
            get_outlier_detection_store().keys().contains(&"invalid-add/svc".into()),
            remaining > 0
        );
        assert_eq!(
            resolve_degrade_threshold("invalid-add/svc"),
            if remaining > 0 { 60 } else { 0 }
        );
        if remaining > 0 {
            assert_eq!(
                store.get_effective(&target).unwrap().winning_policy_key,
                "invalid-add/second"
            );
            assert_eq!(store.get_lb_policy("invalid-add/svc"), Some(ParsedLBPolicy::Ewma));
        } else {
            assert!(store.get_effective(&target).is_none());
        }
    }
    assert_eq!(
        old.get_effective(&service_target("invalid-add", "svc"))
            .unwrap()
            .winning_policy_key,
        "invalid-add/first"
    );
    store.full_set_policies(HashMap::new());
}

#[test]
fn replacement_ai_add_retains_equivalent_handles_and_retires_old_typed_target() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let target = |name: &str| PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "ai-add".into(),
        name: name.into(),
    };
    let ai = ai_policy("ai-add", "ai", "a", 20, "250ms");
    let service = policy(
        "ai-add",
        "service",
        "a",
        None,
        Some(LoadBalancerType::LeastConn),
        None,
        false,
        false,
    );
    store.full_set_policies(HashMap::from([
        ("ai-add/ai".into(), ai.clone()),
        ("ai-add/service".into(), service),
    ]));
    let pinned = store.snapshot();
    let first = pinned.get_effective(&target("a")).unwrap();
    let service_effective = pinned.get_effective(&service_target("ai-add", "a")).unwrap();
    publish_counters::reset();
    assert_eq!(
        store.partial_update_policies(
            HashMap::from([("ai-add/ai".into(), ai_policy("ai-add", "ai", "a", 20, "250ms"))]),
            HashMap::new(),
            HashSet::new()
        ),
        1
    );
    let repeated = store.snapshot();
    assert_eq!(repeated.generation(), pinned.generation());
    assert!(Arc::ptr_eq(&ai, &repeated.candidates(&target("a"))[0]));
    let kept = repeated.get_effective(&target("a")).unwrap();
    assert!(Arc::ptr_eq(&first, &kept));
    assert!(Arc::ptr_eq(
        first.retry_budget.as_ref().unwrap(),
        kept.retry_budget.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        first.logical_circuit_breaker.as_ref().unwrap(),
        kept.logical_circuit_breaker.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        first.ai_credential_outlier.as_ref().unwrap(),
        kept.ai_credential_outlier.as_ref().unwrap()
    ));
    assert_eq!(
        publish_counters::SIDE_EFFECT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    store.partial_update_policies(
        HashMap::from([("ai-add/ai".into(), ai_policy("ai-add", "ai", "a", 20, "2s"))]),
        HashMap::new(),
        HashSet::new(),
    );
    let changed = store.get_effective(&target("a")).unwrap();
    assert_eq!(store.snapshot().candidates(&target("a")).len(), 1);
    assert_eq!(changed.connect_timeout, Some(std::time::Duration::from_secs(2)));
    assert!(Arc::ptr_eq(
        first.retry_budget.as_ref().unwrap(),
        changed.retry_budget.as_ref().unwrap()
    ));

    store.partial_update_policies(
        HashMap::from([("ai-add/ai".into(), ai_policy("ai-add", "ai", "b", 20, "2s"))]),
        HashMap::new(),
        HashSet::new(),
    );
    assert_eq!(store.snapshot().generation(), pinned.generation() + 2);
    assert!(store.snapshot().candidates(&target("a")).is_empty());
    assert!(store.get_effective(&target("a")).is_none());
    assert!(get_ai_credential_outlier_store()
        .effective_handle_for_test(&target("a"))
        .is_none());
    let new = store.get_effective(&target("b")).unwrap();
    assert!(Arc::ptr_eq(
        new.ai_credential_outlier.as_ref().unwrap(),
        &get_ai_credential_outlier_store()
            .effective_handle_for_test(&target("b"))
            .unwrap()
    ));
    assert!(Arc::ptr_eq(
        &service_effective,
        &store.get_effective(&service_target("ai-add", "a")).unwrap()
    ));
    assert_eq!(
        pinned.get_effective(&target("a")).unwrap().connect_timeout,
        Some(std::time::Duration::from_millis(250))
    );
    store.full_set_policies(HashMap::new());
    assert!(get_ai_credential_outlier_store()
        .effective_handle_for_test(&target("b"))
        .is_none());
}

#[test]
fn partial_update_does_not_scan_all_policies() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let mut initial = HashMap::new();
    for i in 0..8 {
        let name = format!("p{i}");
        initial.insert(
            format!("scan/{name}"),
            policy(
                "scan",
                &name,
                &format!("svc{i}"),
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                None,
                false,
                false,
            ),
        );
    }
    store.full_set_policies(initial);
    publish_counters::reset();
    store.partial_update_policies(
        HashMap::new(),
        HashMap::from([(
            "scan/p0".to_string(),
            policy(
                "scan",
                "p0",
                "svc0",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                None,
                false,
                false,
            ),
        )]),
        HashSet::new(),
    );
    assert_eq!(
        publish_counters::PREPARE_POLICY_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        publish_counters::FULL_INDEX_BUILDS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    store.full_set_policies(HashMap::new());
}

#[test]
fn ai_outlier_handle_materialized_before_store() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "mat".to_string(),
        name: "models".to_string(),
    };
    store.full_set_policies(HashMap::from([(
        "mat/policy".to_string(),
        ai_policy("mat", "policy", "models", 20, "250ms"),
    )]));
    let snap = store.get_effective(&target).expect("effective");
    let handle = snap.ai_credential_outlier.as_ref().expect("handle on snapshot");
    let published = get_ai_credential_outlier_store()
        .effective_handle_for_test(&target)
        .expect("handle in outlier store");
    assert!(Arc::ptr_eq(handle, &published));
    store.full_set_policies(HashMap::new());
}

#[test]
fn ai_policy_publication_gaps_never_alias_the_backend_fallback() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "after".to_string(),
        name: "models".to_string(),
    };
    let outliers = get_ai_credential_outlier_store();
    outliers.clear_effective_policy(&target);
    outliers.remove_credentials(&target);
    let fallback = outliers
        .set_credentials(target.clone(), ["primary".to_string()])
        .expect("config-free fallback");

    // `build_effective` deliberately materializes the effective role before
    // its containing ArcSwap generation is stored. The live backend
    // fallback must remain a different generation during that add gap.
    let policy = ai_policy("after", "policy", "models", 20, "250ms");
    let candidate = BackendTrafficPolicyStore::build_effective(&target, "after/policy", policy.as_ref(), None);
    let effective = candidate.ai_credential_outlier.as_ref().expect("effective generation");
    assert!(!Arc::ptr_eq(&fallback, effective));
    assert!(outliers
        .fallback_handle_for_test(&target)
        .is_some_and(|published| Arc::ptr_eq(&fallback, &published)));

    // During removal, the ArcSwap without an effective policy is visible
    // before cold cleanup clears this role. Fallback selection is still
    // stable both before and after that cleanup.
    assert!(outliers.effective_handle_for_test(&target).is_some());
    outliers.clear_effective_policy(&target);
    assert!(outliers.effective_handle_for_test(&target).is_none());
    assert!(outliers
        .fallback_handle_for_test(&target)
        .is_some_and(|published| Arc::ptr_eq(&fallback, &published)));
    assert!(effective.capture("primary").is_some());

    outliers.remove_credentials(&target);
    assert!(!outliers.keys().contains(&target.to_string()));
}

#[test]
fn bare_policy_target_repair_preserves_raw_owner_and_atomic_eligibility() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    for full in [false, true] {
        let store = BackendTrafficPolicyStore::new();
        let mut source = (*policy(
            "default",
            "bare",
            "backend",
            None,
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        ))
        .clone();
        source.metadata.namespace = None;
        store.full_set_policies(HashMap::from([("bare".into(), Arc::new(source.clone()))]));
        let target = service_target("default", "backend");
        let pinned = store.snapshot();
        assert!(pinned.get_effective(&target).is_some());
        let mut bad = source.spec.target_refs[0].clone();
        bad.name = "bad/name".into();
        source.spec.target_refs.push(bad);
        let invalid = HashMap::from([("bare".into(), Arc::new(source.clone()))]);
        if full {
            store.full_set_policies(invalid);
        } else {
            store.partial_update_policies(HashMap::new(), invalid, HashSet::new());
        }
        assert!(store.get_effective(&target).is_none());
        assert!(pinned.get_effective(&target).is_some());
        source.spec.target_refs.pop();
        source.spec.target_refs = vec![source.spec.target_refs[0].clone(); 20];
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([("bare".into(), Arc::new(source.clone()))]),
            HashSet::new(),
        );
        assert!(
            store.get_effective(&target).is_some(),
            "Service duplicates and fanout remain unbounded"
        );
        source.spec.target_refs = vec![source.spec.target_refs[0].clone()];
        source.spec.target_refs[0].name = "next".into();
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([("bare".into(), Arc::new(source.clone()))]),
            HashSet::new(),
        );
        assert!(store.get_effective(&target).is_none());
        assert!(store.get_effective(&service_target("default", "next")).is_some());
        source.metadata.namespace = Some("".into());
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([("bare".into(), Arc::new(source))]),
            HashSet::new(),
        );
        assert!(store.get_effective(&service_target("default", "next")).is_none());
        store.partial_update_policies(HashMap::new(), HashMap::new(), HashSet::from(["bare".into()]));
        assert!(store.snapshot().get_effective(&target).is_none());
    }
}

#[test]
fn health_snapshot_keeps_actual_invalid_winner_and_repairs_without_policy_fallback() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let mut winner = (*policy(
        "health-owned",
        "first",
        "svc",
        Some(ts("2026-01-01T00:00:00Z")),
        None,
        None,
        true,
        false,
    ))
    .clone();
    winner.spec.health_check.as_mut().unwrap().active = Some(
        serde_json::from_value(serde_json::json!({
            "type": "https", "tls": {"validation": {"hostname": "probe.example"}}
        }))
        .unwrap(),
    );
    winner
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .resolved_tls_error = Some("missing material".into());
    let sibling = policy(
        "health-owned",
        "second",
        "svc",
        Some(ts("2026-02-01T00:00:00Z")),
        None,
        None,
        true,
        false,
    );
    let first = Arc::new(winner.clone());
    store.full_set_policies(HashMap::from([
        ("health-owned/first".into(), first.clone()),
        ("health-owned/second".into(), sibling.clone()),
    ]));
    let snapshot = get_hc_config_store().resolve_policy("health-owned/svc").unwrap();
    assert!(std::ptr::eq(snapshot.owner(), first.as_ref()));
    assert!(snapshot.config().resolved_tls_error.is_some());
    winner
        .spec
        .health_check
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .resolved_tls_error = None;
    store.full_set_policies(HashMap::from([
        ("health-owned/first".into(), Arc::new(winner)),
        ("health-owned/second".into(), sibling),
    ]));
    let repaired = get_hc_config_store().resolve_policy("health-owned/svc").unwrap();
    assert_eq!(repaired.owner().metadata.name.as_deref(), Some("first"));
    assert!(repaired.config().resolved_tls_error.is_none());
    assert!(!Arc::ptr_eq(&snapshot.identity, &repaired.identity));
    assert!(snapshot.config().resolved_tls_error.is_some());
    store.full_set_policies(HashMap::new());
}
