use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::HttpRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessResult, ProcessorHandler};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use std::sync::Arc;

fn route(ns: &str, name: &str, spec: serde_json::Value) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"namespace": ns, "name": name},
        "spec": spec
    }))
    .expect("HTTPRoute fixture must deserialize")
}

#[test]
fn ordinary_rules_get_uid_or_typed_key_and_deterministic_rule_identity() {
    let mut uid_route = route(
        "apps",
        "ordinary",
        serde_json::json!({"rules": [
            {"name": "primary", "backendRefs": [{"name": "svc", "port": 80}]},
            {"backendRefs": [{"name": "fallback", "port": 80}]}
        ]}),
    );
    uid_route.metadata.uid = Some("ordinary-uid".to_string());
    populate_terminal_rule_identities(&mut uid_route);
    let rules = uid_route.spec.rules.as_ref().expect("ordinary route should keep rules");
    assert_eq!(rules[0].resolved_terminal_route_uid.as_deref(), Some("ordinary-uid"));
    assert_eq!(
        rules[0].resolved_terminal_rule_identity.as_deref(),
        Some("name:primary")
    );
    assert_eq!(rules[1].resolved_terminal_rule_identity.as_deref(), Some("index:1"));

    let mut uidless = route(
        "apps",
        "file-route",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    populate_terminal_rule_identities(&mut uidless);
    let rule = &uidless.spec.rules.as_ref().expect("UID-less route should keep rules")[0];
    assert_eq!(
        rule.resolved_terminal_route_uid.as_deref(),
        Some("HTTPRoute/apps/file-route")
    );
    assert_eq!(rule.resolved_terminal_rule_identity.as_deref(), Some("index:0"));
}

/// Terminal identities and root auth errors are compared with the values the
/// flattening controller produced for the same fixture before this module
/// was extracted.
#[tokio::test]
async fn ordinary_route_terminal_identity_and_auth_unchanged() {
    let ctx = HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        3,
        20,
        Arc::new(ControllerRuntime::default()),
    );
    let input: HTTPRoute =
        serde_json::from_str(include_str!("../../../../../fixtures/http-route-ordinary-input.json")).unwrap();
    let ProcessResult::Continue(parsed) = HttpRouteHandler::default().parse(input, &ctx).await else {
        panic!("parse must continue");
    };
    let rules: Vec<serde_json::Value> = parsed
        .spec
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .map(|rule| {
            serde_json::json!({
                "resolvedTerminalRouteUid": rule.resolved_terminal_route_uid,
                "resolvedTerminalRuleIdentity": rule.resolved_terminal_rule_identity,
            })
        })
        .collect();
    let auth_errors: Vec<String> = root_auth(&parsed, &ctx, false)
        .errors
        .iter()
        .map(|error| format!("{error:?}"))
        .collect();
    let actual = serde_json::json!({"rules": rules, "authErrors": auth_errors});
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/http-route-ordinary-terminal-auth.json"
    ))
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn missing_rules_or_invalid_source_yields_no_root_auth_decisions() {
    let ctx = HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        3,
        20,
        Arc::new(ControllerRuntime::default()),
    );
    let no_rules = route("apps", "empty", serde_json::json!({}));
    let auth = root_auth(&no_rules, &ctx, false);
    assert!(auth.decisions.is_empty());
    assert!(auth.errors.is_empty());

    // An invalid reference source (an owner namespace that is not a valid
    // object namespace) yields no decisions even though the rule carries
    // ExternalAuth.
    let invalid_source = route(
        "Bad",
        "invalid",
        serde_json::json!({"rules": [{
            "filters": [{"type": "ExternalAuth", "externalAuth": {
                "target": {"name": "auth", "port": 8080}, "decision": {}
            }}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }]}),
    );
    assert!(HttpRouteBackendRefs::from_route(&invalid_source).is_err());
    let auth = root_auth(&invalid_source, &ctx, false);
    assert!(auth.decisions.is_empty());
    assert!(auth.errors.is_empty());
}
