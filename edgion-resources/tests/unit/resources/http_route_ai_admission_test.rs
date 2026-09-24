use super::*;
use serde_json::json;

fn rule(backends: serde_json::Value) -> HTTPRouteRule {
    serde_json::from_value(json!({"backendRefs": backends})).unwrap()
}

fn ai_fact(valid: bool) -> ResolvedForwardingTarget {
    ResolvedForwardingTarget::EdgionBackend {
        spec_valid: valid,
        canonical_models: vec!["canonical".to_string()],
    }
}

fn check(
    rule: &HTTPRouteRule,
    facts: Option<&ResolvedAiAdmission>,
    count: usize,
    config: Option<&AiProxyConfig>,
) -> Option<String> {
    let configs: Vec<_> = config.into_iter().collect();
    ai_rule_invalid_reason(
        rule,
        "apps",
        facts,
        AiProxyAdmission {
            active_count: count,
            configs: &configs,
            missing_reference: None,
        },
    )
}

#[test]
fn required_metadata_never_becomes_ordinary_forwarding() {
    let ai = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
    for facts in [
        None,
        Some(ResolvedAiAdmission { backends: vec![] }),
        Some(ResolvedAiAdmission {
            backends: vec![ResolvedForwardingTarget::Service],
        }),
    ] {
        assert_eq!(
            check(&ai, facts.as_ref(), 0, None).as_deref(),
            Some("AI routing admission metadata is unavailable")
        );
    }
    let service = rule(json!([{"name": "web", "port": 80}]));
    assert_eq!(check(&service, None, 0, None), None);
    assert!(check(&service, None, 1, None).is_some());
    let unsupported = rule(json!([{"group": "edgion.io", "kind": "Unsupported", "name": "target"}]));
    assert!(check(&unsupported, None, 0, None).is_some());
}

#[test]
fn bypassed_ai_proxy_rejects_the_matched_ai_rule() {
    let rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
    let facts = ResolvedAiAdmission {
        backends: vec![ai_fact(true)],
    };
    assert_eq!(check(&rule, Some(&facts), 1, None), None);
    assert_eq!(
        check(&rule, Some(&facts), 0, None).as_deref(),
        Some("AI routing requires exactly one enabled AiProxy (found 0)")
    );
    assert_eq!(
        check(&rule, Some(&facts), 2, None).as_deref(),
        Some("AI routing requires exactly one enabled AiProxy (found 2)")
    );
}

#[test]
fn promoted_zero_weight_slots_use_current_effective_config() {
    let rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0}]));
    let config: AiProxyConfig = serde_json::from_value(json!({
            "modelRoutes": [{"match": {"model": "virtual"}, "action": {
                "candidates": [{"backend": {"group": "edgion.io", "kind": "EdgionBackend", "namespace": "apps", "name": "models", "occurrence": 0}, "weight": 1}]
            }}]
        })).unwrap();
    let facts = ResolvedAiAdmission {
        backends: vec![ai_fact(true)],
    };
    assert!(check(&rule, Some(&facts), 1, None).is_some());
    assert_eq!(check(&rule, Some(&facts), 1, Some(&config)), None);
    let invalid = ResolvedAiAdmission {
        backends: vec![ai_fact(false)],
    };
    assert_eq!(
        check(&rule, Some(&invalid), 1, Some(&config)).as_deref(),
        Some("Forwarding backend has an invalid kind or variant")
    );
}

#[test]
fn ordered_shape_variant_and_model_failures_preserve_diagnostics() {
    let mut rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
    let facts = ResolvedAiAdmission {
        backends: vec![ai_fact(true)],
    };
    let config: AiProxyConfig = serde_json::from_value(json!({
        "modelRoutes": [{"match": {"model": "virtual"}, "action": {"rewrite": "missing"}}]
    }))
    .unwrap();
    assert_eq!(
        check(&rule, Some(&facts), 1, Some(&config)).as_deref(),
        Some("modelRoutes[0].action.rewrite must name a canonical model on a selected backend")
    );
    rule.retry = serde_json::from_value(json!({"attempts": 8})).ok();
    assert_eq!(
        check(&rule, Some(&facts), 1, Some(&config)).as_deref(),
        Some("AI routing retry.attempts must not exceed 7")
    );
    let invalid = ResolvedAiAdmission {
        backends: vec![ai_fact(false)],
    };
    assert_eq!(
        backend_variant_invalid_reason(&rule, &invalid).as_deref(),
        Some("EdgionBackend target does not configure a supported variant")
    );
    let incompatible = ResolvedAiAdmission {
        backends: vec![ResolvedForwardingTarget::Incompatible],
    };
    assert_eq!(
        backend_variant_invalid_reason(&rule, &incompatible).as_deref(),
        Some("EdgionBackend target has an incompatible cached representation")
    );
}

#[test]
fn missing_and_denied_slots_do_not_become_service_backends() {
    let mut rule = rule(json!([
        {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
        {"name": "web", "port": 80}
    ]));
    let mut facts = ResolvedAiAdmission {
        backends: vec![ai_fact(true), ResolvedForwardingTarget::Missing],
    };
    assert_eq!(check(&rule, Some(&facts), 1, None), None);
    facts.backends[1] = ResolvedForwardingTarget::Service;
    assert!(check(&rule, Some(&facts), 1, None)
        .unwrap()
        .contains("must not be mixed"));
    rule.backend_refs.as_mut().unwrap()[1].ref_denied = Some(super::super::common::RefDenied {
        target_namespace: "other".to_string(),
        target_name: "web".to_string(),
        reason: None,
    });
    assert_eq!(check(&rule, Some(&facts), 1, None), None);
}

#[test]
fn facts_survive_clone_and_transport_without_entering_operator_schema() {
    let mut rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
    rule.resolved_ai_admission = Some(ResolvedAiAdmission {
        backends: vec![ai_fact(true)],
    });
    let clone = rule.clone();
    assert_eq!(clone.resolved_ai_admission, rule.resolved_ai_admission);
    let wire = serde_json::to_value(&clone).unwrap();
    assert!(wire.get("resolvedAiAdmission").is_some());
    let restored: HTTPRouteRule = serde_json::from_value(wire).unwrap();
    assert_eq!(restored.resolved_ai_admission, rule.resolved_ai_admission);
    let schema = serde_json::to_value(schemars::schema_for!(HTTPRouteRule)).unwrap();
    assert!(!schema["properties"]
        .as_object()
        .unwrap()
        .contains_key("resolvedAiAdmission"));
}
