use super::*;

#[test]
fn status_serializes_gep2648_ancestor_shape() {
    use crate::resources::common::{ParentReference, PolicyAncestorStatus};
    let status = EdgionBackendTrafficPolicyStatus {
        ancestors: vec![PolicyAncestorStatus {
            ancestor_ref: ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("ns".to_string()),
                name: "gw".to_string(),
                section_name: None,
                port: None,
            },
            controller_name: "example.io/controller".to_string(),
            conditions: vec![],
        }],
    };
    let v = serde_json::to_value(&status).unwrap();
    assert_eq!(v["ancestors"][0]["ancestorRef"]["name"], "gw");
    assert_eq!(v["ancestors"][0]["controllerName"], "example.io/controller");
    // Empty ancestors still serialize an explicit `"ancestors": []`, never
    // an empty object: K8s status persistence uses JSON Merge Patch, and
    // merge-patching `{"status": {}}` cannot clear a previously-persisted
    // non-empty ancestors list — only an explicit empty array replaces it.
    let empty = serde_json::to_value(EdgionBackendTrafficPolicyStatus::default()).unwrap();
    assert_eq!(empty, serde_json::json!({"ancestors": []}));
}

#[test]
fn backend_tls_policy_status_empty_ancestors_serializes_explicit_array() {
    use crate::resources::backend_tls_policy::BackendTLSPolicyStatus;
    // Same merge-patch tombstone concern as EdgionBackendTrafficPolicyStatus
    // above: an empty ancestors list must serialize as `"ancestors": []`,
    // not be omitted, or a JSON Merge Patch can never clear a
    // previously-persisted non-empty list.
    let empty = serde_json::to_value(BackendTLSPolicyStatus::default()).unwrap();
    assert_eq!(empty, serde_json::json!({"ancestors": []}));
}

fn minimal_spec_yaml() -> &'static str {
    r#"
targetRefs:
  - kind: Service
    name: payments
loadBalancer:
  type: ConsistentHash
  consistentHash:
    hashOn: header
    key: x-user-id
  degradeThreshold: 30
outlierDetection:
  consecutiveErrors: 3
"#
}

#[test]
fn deserialize_full_spec() {
    let spec: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(minimal_spec_yaml()).expect("deserialize");
    assert_eq!(spec.target_refs[0].name, "payments");
    let lb = spec.load_balancer.as_ref().expect("lb");
    assert_eq!(lb.r#type, LoadBalancerType::ConsistentHash);
    let ch = lb.consistent_hash.as_ref().expect("ch");
    assert_eq!(ch.hash_on, HashOn::Header);
    assert_eq!(ch.key.as_deref(), Some("x-user-id"));
    assert_eq!(lb.degrade_threshold, Some(30));
    assert!(spec.get_validation_errors().is_empty());
}

#[test]
fn resolved_target_class_is_transport_only_and_not_schema_visible() {
    let forged: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: payments\nresolvedTargetClass: ai\n")
            .expect("transport-shaped input can carry the marker for controller clearing tests");
    assert_eq!(forged.resolved_target_class, Some(ResolvedPolicyTargetClass::Ai));
    assert_eq!(serde_json::to_value(&forged).unwrap()["resolvedTargetClass"], "ai");

    let schema = serde_json::to_value(schemars::schema_for!(EdgionBackendTrafficPolicySpec)).unwrap();
    assert!(schema["properties"].get("resolvedTargetClass").is_none());
}

#[test]
fn canonical_target_keys_preserve_target_type() {
    let service = PolicyTargetRef {
        group: Some("core".to_string()),
        kind: "Service".to_string(),
        name: "shared".to_string(),
    }
    .canonical_key("app")
    .expect("core Service target");
    let ai_backend = PolicyTargetRef {
        group: Some("edgion.io".to_string()),
        kind: "EdgionBackend".to_string(),
        name: "shared".to_string(),
    }
    .canonical_key("app")
    .expect("EdgionBackend target");

    assert_eq!(service.target_kind, PolicyTargetKind::Service);
    assert_eq!(service.to_string(), "/Service/app/shared");
    assert_eq!(ai_backend.target_kind, PolicyTargetKind::EdgionBackend);
    assert_eq!(ai_backend.to_string(), "edgion.io/EdgionBackend/app/shared");
    assert_ne!(service, ai_backend);
}

#[test]
fn target_refs_preserve_service_fanout_and_bound_ai_targets() {
    let empty: EdgionBackendTrafficPolicySpec = yaml_serde::from_str("targetRefs: []").unwrap();
    assert!(empty
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("must not be empty")));

    let service_refs = (0..17).map(|index| format!("  - kind: Service\n    name: svc-{index}"));
    let many_services: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str(&format!("targetRefs:\n{}", service_refs.collect::<Vec<_>>().join("\n"))).unwrap();
    assert!(many_services.get_validation_errors().is_empty());

    let duplicate_service: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - group: core\n    kind: Service\n    name: shared\n  - group: \"\"\n    kind: Service\n    name: shared",
        )
        .unwrap();
    assert!(duplicate_service.get_validation_errors().is_empty());

    let ai_refs = (0..17).map(|index| format!("  - group: edgion.io\n    kind: EdgionBackend\n    name: ai-{index}"));
    let too_many_ai: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str(&format!("targetRefs:\n{}", ai_refs.collect::<Vec<_>>().join("\n"))).unwrap();
    assert!(too_many_ai
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("at most 16")));

    let duplicate_ai: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - group: edgion.io\n    kind: EdgionBackend\n    name: shared\n  - group: edgion.io\n    kind: EdgionBackend\n    name: shared",
        )
        .unwrap();
    assert!(duplicate_ai
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("duplicate target")));

    let mixed: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - kind: Service\n    name: backend\n  - group: edgion.io\n    kind: EdgionBackend\n    name: provider",
        )
        .unwrap();
    assert!(mixed
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("same target kind")));
}

#[test]
fn edgion_backend_targets_enforce_static_policy_fields() {
    let allowed: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - group: edgion.io\n    kind: EdgionBackend\n    name: provider\nconnection:\n  connectTimeout: 2s\nretryConstraint: {}\ncircuitBreaker:\n  maxParallelRequests: 20\noutlierDetection:\n  consecutiveErrors: 5",
        )
        .unwrap();
    assert!(allowed.get_validation_errors().is_empty());

    let forbidden = "healthCheck:\n  active: {}";
    let yaml = format!("targetRefs:\n  - group: edgion.io\n    kind: EdgionBackend\n    name: provider\n{forbidden}");
    let spec: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(&yaml).unwrap();
    assert!(
        spec.get_validation_errors()
            .iter()
            .any(|error| error.contains("not supported for EdgionBackend targets")),
        "field must be rejected: {forbidden}"
    );

    // Variant-dependent fields are admitted structurally and resolved by
    // the Controller after it proves the target capability.
    for deferred in [
        "loadBalancer:\n  type: RoundRobin",
        "upstreamAuthority:\n  pattern: '*.example.com'\n  template: '${ctx:tenant}.example.com'",
    ] {
        let yaml =
            format!("targetRefs:\n  - group: edgion.io\n    kind: EdgionBackend\n    name: provider\n{deferred}");
        let spec: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(&yaml).unwrap();
        assert!(
            spec.get_validation_errors().is_empty(),
            "field is capability-resolved: {deferred}"
        );
    }
}

#[test]
fn to_parsed_mapping() {
    use crate::resources::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
    let mk = |t, ch| LoadBalancerConfig {
        r#type: t,
        consistent_hash: ch,
        degrade_threshold: None,
    };
    assert_eq!(mk(LoadBalancerType::RoundRobin, None).to_parsed(), None);
    assert_eq!(
        mk(LoadBalancerType::LeastConn, None).to_parsed(),
        Some(ParsedLBPolicy::LeastConn)
    );
    assert_eq!(mk(LoadBalancerType::Ewma, None).to_parsed(), Some(ParsedLBPolicy::Ewma));
    let ch = ConsistentHashConfig {
        hash_on: HashOn::QueryParam,
        key: Some("uid".into()),
    };
    assert_eq!(
        mk(LoadBalancerType::ConsistentHash, Some(ch)).to_parsed(),
        Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg("uid".into())))
    );
    // Header hashOn maps to ConsistentHashOn::Header.
    let ch_header = ConsistentHashConfig {
        hash_on: HashOn::Header,
        key: Some("x-user-id".into()),
    };
    assert_eq!(
        mk(LoadBalancerType::ConsistentHash, Some(ch_header)).to_parsed(),
        Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
            "x-user-id".into()
        )))
    );
    // Cookie hashOn maps to ConsistentHashOn::Cookie.
    let ch_cookie = ConsistentHashConfig {
        hash_on: HashOn::Cookie,
        key: Some("session-id".into()),
    };
    assert_eq!(
        mk(LoadBalancerType::ConsistentHash, Some(ch_cookie)).to_parsed(),
        Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie(
            "session-id".into()
        )))
    );
    // SourceIp hashOn maps to the no-argument ConsistentHashOn::SourceIp.
    let ch_source_ip = ConsistentHashConfig {
        hash_on: HashOn::SourceIp,
        key: None,
    };
    assert_eq!(
        mk(LoadBalancerType::ConsistentHash, Some(ch_source_ip)).to_parsed(),
        Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp))
    );
}

#[test]
fn source_ip_hash_accepts_absent_key() {
    let spec: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: sourceIp",
        )
        .unwrap();
    assert!(
        spec.get_validation_errors().is_empty(),
        "sourceIp with no key must be accepted: {:?}",
        spec.get_validation_errors()
    );
    // And it deserializes to the keyless config.
    let ch = spec.load_balancer.unwrap().consistent_hash.unwrap();
    assert_eq!(ch.hash_on, HashOn::SourceIp);
    assert_eq!(ch.key, None);
}

#[test]
fn source_ip_hash_rejects_present_key() {
    let spec: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: sourceIp\n    key: x-user-id",
        )
        .unwrap();
    assert!(
        spec.get_validation_errors()
            .iter()
            .any(|e| e.contains("key must be absent when hashOn is sourceIp")),
        "sourceIp with a key must be rejected"
    );
}

#[test]
fn validation_rejects_bad_specs() {
    // empty targetRefs
    let empty: EdgionBackendTrafficPolicySpec = yaml_serde::from_str("targetRefs: []").unwrap();
    assert!(!empty.get_validation_errors().is_empty());
    // non-Service kind
    let bad_kind: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Deployment\n    name: x").unwrap();
    assert!(!bad_kind.get_validation_errors().is_empty());
    // ConsistentHash without consistentHash section
    let no_ch: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash")
            .unwrap();
    assert!(!no_ch.get_validation_errors().is_empty());
    // degradeThreshold > 100
    let big_pt: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
        "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: RoundRobin\n  degradeThreshold: 101",
    )
    .unwrap();
    assert!(!big_pt.get_validation_errors().is_empty());
    // empty targetRef name
    let empty_name: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: \"\"").unwrap();
    assert!(
        empty_name
            .get_validation_errors()
            .iter()
            .any(|e| e.contains("targetRef name must not be empty")),
        "empty targetRef name must be rejected"
    );
    // consistentHash present with a non-ConsistentHash type
    let ch_wrong_type: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: RoundRobin\n  consistentHash:\n    hashOn: header\n    key: x-user-id",
        )
        .unwrap();
    assert!(
        ch_wrong_type
            .get_validation_errors()
            .iter()
            .any(|e| e.contains("consistentHash is only valid when type is ConsistentHash")),
        "consistentHash with a non-ConsistentHash type must be rejected"
    );
    // consistentHash present but key is empty
    let ch_empty_key: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: header\n    key: \"\"",
        )
        .unwrap();
    assert!(
        ch_empty_key
            .get_validation_errors()
            .iter()
            .any(|e| e.contains("consistentHash.key must not be empty")),
        "empty consistentHash.key must be rejected"
    );
    // invalid nested healthCheck — the `healthCheck: {err}` prefix is exercised
    let bad_hc: EdgionBackendTrafficPolicySpec = yaml_serde::from_str(
        "targetRefs:\n  - kind: Service\n    name: x\nhealthCheck:\n  active:\n    healthyThreshold: 0",
    )
    .unwrap();
    assert!(
        bad_hc
            .get_validation_errors()
            .iter()
            .any(|e| e.starts_with("healthCheck:")),
        "invalid nested healthCheck must surface with a healthCheck: prefix"
    );
    // invalid nested outlierDetection — the `outlierDetection: {err}` prefix is exercised
    let bad_od: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\noutlierDetection:\n  consecutiveErrors: 0")
            .unwrap();
    assert!(
        bad_od
            .get_validation_errors()
            .iter()
            .any(|e| e.starts_with("outlierDetection:")),
        "invalid nested outlierDetection must surface with an outlierDetection: prefix"
    );
    // invalid nested circuitBreaker — the `circuitBreaker: {err}` prefix is exercised
    let bad_cb: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\ncircuitBreaker:\n  maxParallelRequests: 0")
            .unwrap();
    assert!(
        bad_cb
            .get_validation_errors()
            .iter()
            .any(|e| e.starts_with("circuitBreaker:")),
        "invalid nested circuitBreaker must surface with a circuitBreaker: prefix"
    );
}

#[test]
fn precedence_key_orders_oldest_then_name() {
    use crate::resources::common::policy_precedence::policy_precedence_key;
    let older = policy_precedence_key(Some("2026-01-01T00:00:00Z".parse().unwrap()), "b");
    let newer = policy_precedence_key(Some("2026-06-01T00:00:00Z".parse().unwrap()), "a");
    let no_ts = policy_precedence_key(None, "a");
    assert!(older < newer, "older timestamp wins regardless of name");
    assert!(newer < no_ts, "any timestamp beats absent timestamp");
    // Equal timestamps: the remaining tie breaks by name lexicographically.
    let tie_a = policy_precedence_key(Some("2026-01-01T00:00:00Z".parse().unwrap()), "a");
    let tie_b = policy_precedence_key(Some("2026-01-01T00:00:00Z".parse().unwrap()), "b");
    assert!(tie_a < tie_b, "equal timestamps break ties by name (a < b)");
}

#[test]
fn upstream_authority_validation_flows_through_spec() {
    use crate::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
    use crate::resources::upstream_authority::UpstreamAuthorityConfig;

    let base = |ua: Option<UpstreamAuthorityConfig>, hc: bool| EdgionBackendTrafficPolicySpec {
        prepared_ancestor_admission: Some(
            crate::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted,
        ),
        prepared_precedence: None,
        current_status: None,
        target_refs: vec![PolicyTargetRef {
            group: Some(String::new()),
            kind: "Service".to_string(),
            name: "sandbox-gw".to_string(),
        }],
        resolved_target_class: None,
        load_balancer: None,
        health_check: hc.then(|| ServiceHealthCheck {
            active: Some(ActiveHealthCheckConfig::default()),
        }),
        outlier_detection: None,
        upstream_authority: ua,
        retry_constraint: None,
        circuit_breaker: None,
        connection: None,
    };

    // Valid, no health check.
    let ok = UpstreamAuthorityConfig {
        pattern: "*.sandbox.internal".to_string(),
        template: "${ctx:tenant}.sandbox.internal".to_string(),
        health_check_host: None,
    };
    assert!(base(Some(ok.clone()), false).get_validation_errors().is_empty());

    // Same config, but an active health check now demands healthCheckHost.
    let errs = base(Some(ok), true).get_validation_errors();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("healthCheckHost"), "got: {}", errs[0]);

    // A template whose suffix does not match the pattern is rejected.
    let bad = UpstreamAuthorityConfig {
        pattern: "*.sandbox.internal".to_string(),
        template: "${ctx:tenant}.evil.com".to_string(),
        health_check_host: None,
    };
    let errs = base(Some(bad), false).get_validation_errors();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("upstreamAuthority"), "got: {}", errs[0]);

    // Absent section => unchanged behaviour, no errors.
    assert!(base(None, true).get_validation_errors().is_empty());
}

#[test]
fn retry_constraint_empty_object_uses_upstream_defaults() {
    let spec: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint: {}").unwrap();
    let rc = spec.retry_constraint.as_ref().expect("present");
    assert_eq!(rc.budget.percent, 20);
    assert_eq!(rc.budget.interval, "10s");
    let f = rc.min_retry_rate.as_ref().expect("floor default present");
    assert_eq!(f.count, 10);
    assert_eq!(f.interval, "1s");
    assert!(spec.get_validation_errors().is_empty());
}

#[test]
fn retry_constraint_validation_rejects_bad_values() {
    let bad = |y: &str| -> Vec<String> {
        yaml_serde::from_str::<EdgionBackendTrafficPolicySpec>(y)
            .unwrap()
            .get_validation_errors()
    };
    // percent > 100
    assert!(!bad(
        "targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 101\n    interval: 10s"
    )
    .is_empty());
    // budget.interval < 1s
    assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 20\n    interval: 500ms").iter().any(|e| e.contains("budget.interval")));
    // budget.interval > 1h
    assert!(bad(
        "targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 20\n    interval: 2h"
    )
    .iter()
    .any(|e| e.contains("budget.interval")));
    // minRetryRate.count == 0
    assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: 10s\n  minRetryRate:\n    count: 0\n    interval: 1s").iter().any(|e| e.contains("minRetryRate.count")));
    // minRetryRate.count > 1_000_000
    assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: 10s\n  minRetryRate:\n    count: 1000001\n    interval: 1s").iter().any(|e| e.contains("minRetryRate.count")));
    // unparseable interval
    assert!(
        !bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: nope").is_empty()
    );
}

#[test]
fn connection_connect_timeout_deserializes_and_validates() {
    let spec: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 250ms")
            .unwrap();
    assert_eq!(
        spec.connection.as_ref().unwrap().connect_timeout.as_deref(),
        Some("250ms")
    );
    assert!(spec.get_validation_errors().is_empty());

    // Empty connection object is valid (no override).
    let empty: EdgionBackendTrafficPolicySpec =
        yaml_serde::from_str("targetRefs:\n  - kind: Service\n    name: x\nconnection: {}").unwrap();
    assert!(empty.connection.as_ref().unwrap().connect_timeout.is_none());
    assert!(empty.get_validation_errors().is_empty());
}

#[test]
fn connection_connect_timeout_validation_rejects_bad_values() {
    let bad = |y: &str| -> Vec<String> {
        yaml_serde::from_str::<EdgionBackendTrafficPolicySpec>(y)
            .unwrap()
            .get_validation_errors()
    };
    // 0s rejected
    assert!(
        bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 0s")
            .iter()
            .any(|e| e.contains("connection.connectTimeout"))
    );
    // > 1h rejected
    assert!(
        bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 2h")
            .iter()
            .any(|e| e.contains("connection.connectTimeout"))
    );
    // unparseable rejected
    assert!(!bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: nope").is_empty());
}

#[test]
fn malformed_target_name_is_invalid_without_changing_service_and_ai_bounds() {
    use serde_json::json;
    let mut spec: EdgionBackendTrafficPolicySpec = serde_json::from_value(
        json!({"targetRefs":[{"kind":"Service","name":"valid"},{"kind":"Service","name":"bad/name"}]}),
    )
    .unwrap();
    assert!(spec
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("valid resource name")));
    spec.target_refs[1].name = "valid".into();
    spec.target_refs = vec![spec.target_refs[0].clone(); 20];
    assert!(spec.get_validation_errors().is_empty());
    for count in [16, 17] {
        spec.target_refs = (0..count)
            .map(|index| {
                serde_json::from_value(
                    json!({"group":"edgion.io","kind":"EdgionBackend","name":format!("backend-{index}")}),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(spec.get_validation_errors().is_empty(), count == 16);
    }
    spec.target_refs = vec![spec.target_refs[0].clone(); 2];
    assert!(spec
        .get_validation_errors()
        .iter()
        .any(|error| error.contains("duplicate")));
}

#[test]
fn named_hash_policy_clones_share_keys_and_preserve_serialization() {
    use std::sync::Arc;
    for (hash_on, variant) in [
        (HashOn::Header, "Header"),
        (HashOn::Cookie, "Cookie"),
        (HashOn::QueryParam, "Arg"),
    ] {
        for key in [None, Some(""), Some("X-Routing-Key"), Some("café")] {
            let config = LoadBalancerConfig {
                degrade_threshold: None,
                r#type: LoadBalancerType::ConsistentHash,
                consistent_hash: Some(ConsistentHashConfig {
                    hash_on: hash_on.clone(),
                    key: key.map(str::to_owned),
                }),
            };
            let original = config.to_parsed().unwrap();
            let cloned = original.clone();
            let key_of = |policy: &ParsedLBPolicy| match policy {
                ParsedLBPolicy::ConsistentHash(
                    ConsistentHashOn::Header(key) | ConsistentHashOn::Cookie(key) | ConsistentHashOn::Arg(key),
                ) => Arc::clone(key),
                _ => panic!("expected a named hash policy"),
            };
            assert!(Arc::ptr_eq(&key_of(&original), &key_of(&cloned)));
            drop(config);
            drop(original);
            assert_eq!(key_of(&cloned).as_ref(), key.unwrap_or_default());
            assert_eq!(
                serde_json::to_value(&cloned).unwrap(),
                serde_json::json!({"ConsistentHash": { (variant): key.unwrap_or_default() }})
            );
        }
    }
}
