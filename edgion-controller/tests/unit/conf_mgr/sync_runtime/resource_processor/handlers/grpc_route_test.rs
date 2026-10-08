use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn hostname_validation_clears_stale_attachments_and_recovers() {
    use super::super::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::Gateway;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    raw.upsert(
        "apps/gateway".into(),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "metadata": {"namespace": "apps", "name": "gateway"},
                "spec": {"gatewayClassName": "test", "listeners": [
                    {"name": "http", "port": 8080, "protocol": "HTTP"}
                ]}
            }))
            .unwrap(),
        ),
    );
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let handler = GrpcRouteHandler::default();
    let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "route"},
        "spec": {"parentRefs": [{"name": "gateway", "sectionName": "http"}], "rules": [{}]}
    }))
    .unwrap();
    for (hostnames, accepted) in [
        (None, true),
        (Some(Vec::new()), true),
        (Some(vec!["a.example.com".to_string(); 16]), true),
        (Some(vec!["a.example.com".to_string(); 17]), false),
        (Some(vec!["a.example.com".to_string()]), true),
        (Some(vec!["a.example.com".to_string(), "*".to_string()]), false),
        (Some(vec!["127.0.0.1".to_string()]), false),
        (Some(vec!["*.127.0.0.1".to_string()]), true),
        (Some(vec!["A.example.com".to_string()]), false),
        (Some(vec!["a.example.com".to_string()]), true),
    ] {
        route.spec.hostnames = hostnames;
        let errors = handler.validate(&route, &ctx);
        let ProcessResult::Continue(mut parsed) = handler.parse(route, &ctx).await else {
            panic!("invalid hostnames must retain the object for status reporting");
        };
        if !accepted {
            // A prior rule-level warning must not survive whole-route rejection.
            parsed.status.as_mut().unwrap().parents[0]
                .conditions
                .push(condition_true(
                    condition_types::PARTIALLY_INVALID,
                    condition_reasons::RULE_INVALID,
                    "Previous generation contained an invalid rule",
                    None,
                ));
        }
        handler.update_status(&mut parsed, &ctx, &errors);
        let conditions = &parsed.status.as_ref().unwrap().parents[0].conditions;
        let condition = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(condition.status, if accepted { "True" } else { "False" });
        assert_eq!(condition.reason, if accepted { "Accepted" } else { "UnsupportedValue" });
        assert_eq!(parsed.spec.resolved_listeners.is_some(), accepted);
        assert_eq!(parsed.spec.resolved_hostnames.is_some(), accepted);
        if !accepted {
            assert!(parsed
                .metadata
                .annotations
                .as_ref()
                .is_none_or(|a| !a.contains_key("edgion.io/hostname-resolution")));
            assert!(!conditions
                .iter()
                .any(|c| c.type_ == "PartiallyInvalid" && c.status == "True"));
        }
        assert_eq!(
            runtime
                .attached_route_tracker
                .count_for_listener("apps/gateway", "http"),
            i32::from(accepted)
        );
        route = parsed;
    }
    route.spec.parent_refs = None;
    let ProcessResult::Continue(mut parsed) = handler.parse(route, &ctx).await else {
        panic!("parentless route must remain available for status reporting");
    };
    handler.update_status(&mut parsed, &ctx, &[]);
    assert!(parsed.spec.resolved_listeners.is_none());
    assert!(parsed.spec.resolved_hostnames.is_none());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        0
    );
}

fn make_ctx() -> HandlerContext {
    HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        3,
        20,
        Arc::new(ControllerRuntime::default()),
    )
}

fn make_ctx_with_raw_waf(namespace: &str, name: &str) -> HandlerContext {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::prelude_resources::EdgionPlugins;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let plugins: EdgionPlugins = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionPlugins",
        "metadata": {"namespace": namespace, "name": name},
        "spec": {"waf": {"policyRef": {"name": "waf-policy"}}}
    }))
    .expect("WAF fixture must deserialize");
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(format!("{namespace}/{name}"), Arc::new(plugins));
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);
    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

#[tokio::test]
async fn backend_waf_is_controller_invalid_and_registered_for_requeue() {
    let ctx = make_ctx_with_raw_waf("apps", "waf");
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"namespace": "apps", "name": "route"},
        "spec": {"rules": [{
            "backendRefs": [{
                "name": "service", "port": 80,
                "filters": [{
                    "type": "ExtensionRef",
                    "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "waf"
                    }
                }]
            }]
        }]}
    }))
    .expect("GRPCRoute fixture must deserialize");

    assert_eq!(
        find_controller_invalid_rule_indices(&route, "apps", ctx.runtime()),
        vec![0]
    );
    assert!(
        backend_plugin_scope_invalid_reason(&route, 0, false, ctx.runtime(), false)
            .unwrap()
            .contains("cannot contain WAF")
    );
    let accepted = backend_route_wide_scope_errors(&route, ctx.runtime());
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].reason(), condition_reasons::UNSUPPORTED_VALUE);
    assert!(accepted[0].message().contains("rules[0]"));

    GrpcRouteHandler::register_plugin_refs(&route, &ctx);
    let route_ref = GrpcRouteHandler::create_resource_ref(&route);
    assert_eq!(
        ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
        vec!["apps/waf".to_string()]
    );
}

#[tokio::test]
async fn backend_jump_invalidates_only_its_rule() {
    use super::super::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::prelude_resources::EdgionPlugins;
    use edgion_resources::resources::Gateway;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    let plugins: EdgionPlugins = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "default", "name": "jump"},
        "spec": {"requestPlugins": [{"type": "RegionRoute", "config": {}}]}
    }))
    .unwrap();
    raw.upsert("default/jump".to_string(), Arc::new(plugins));
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);
    let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let gateway_raw = Arc::new(LocalRawStore::new());
    let gateway: Gateway = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"namespace": "default", "name": "gw"},
        "spec": {"gatewayClassName": "edgion", "listeners": [{"name": "grpc", "protocol": "HTTP", "port": 80}]}
    }))
    .unwrap();
    gateway_raw.upsert("default/gw".to_string(), Arc::new(gateway));
    gateway_processor.set_raw_store(gateway_raw);
    runtime.processor_registry.register(gateway_processor);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
    let mut route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
        "metadata": {"namespace": "default", "name": "jump-scope"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
            {"backendRefs": [{"name": "bad", "port": 80, "filters": [{"type": "ExtensionRef",
                "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "jump"}}]}]},
            {"backendRefs": [{"name": "good", "port": 80}]}
        ]}
    }))
    .unwrap();
    GrpcRouteHandler::default().update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"
        && condition.status == "True"
        && condition.message.contains("RegionRoute")));
    assert!(conditions
        .iter()
        .any(|condition| condition.type_ == "Accepted" && condition.status == "True"));
}

#[tokio::test]
async fn parse_clears_authored_extension_ref_namespace_metadata() {
    let ctx = make_ctx();
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"namespace": "apps", "name": "route"},
        "spec": {"rules": [{
            "filters": [{"type": "ExtensionRef", "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "rule", "resolvedNamespace": "injected"
            }}],
            "backendRefs": [{"name": "service", "port": 80, "filters": [{
                "type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "backend", "resolvedNamespace": "injected"
                }
            }]}]
        }]}
    }))
    .expect("GRPCRoute fixture must deserialize");

    let parsed = match GrpcRouteHandler::default().parse(route, &ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    };
    let rule = &parsed.spec.rules.as_ref().unwrap()[0];
    assert!(rule.filters.as_ref().unwrap()[0]
        .extension_ref
        .as_ref()
        .unwrap()
        .resolved_namespace
        .is_none());
    assert!(rule.backend_refs.as_ref().unwrap()[0].filters.as_ref().unwrap()[0]
        .extension_ref
        .as_ref()
        .unwrap()
        .resolved_namespace
        .is_none());
}

#[tokio::test]
async fn parse_clears_stale_resolved_listeners_when_current_parent_refs_cannot_resolve() {
    let handler = GrpcRouteHandler::default();
    let ctx = make_ctx();
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {
            "name": "test-route",
            "namespace": "default"
        },
        "spec": {
            "parentRefs": [
                {
                    "name": "missing-gateway"
                }
            ]
        }
    }))
    .expect("route should deserialize");

    let parsed = match handler.parse(route, &ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    };

    assert_eq!(
        parsed.spec.resolved_listeners, None,
        "stale or user-provided resolvedListeners must be cleared when current parentRefs cannot be resolved"
    );
}

fn route_with_rules(rules: serde_json::Value) -> GRPCRoute {
    serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "t", "namespace": "default" },
        "spec": { "rules": rules }
    }))
    .expect("route should deserialize")
}

#[tokio::test]
async fn method_validation_preserves_siblings_publishes_indices_and_recovers() {
    use super::super::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::resources::Gateway;

    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime,
    ));
    let raw = Arc::new(LocalRawStore::new());
    let gateway: Gateway = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "gw", "namespace": "default"},
        "spec": {"gatewayClassName": "edgion", "listeners": [
            {"name": "grpc", "protocol": "HTTP", "port": 80}
        ]}
    }))
    .unwrap();
    raw.upsert("default/gw".to_string(), Arc::new(gateway));
    processor.set_raw_store(raw);
    ctx.runtime().processor_registry.register(processor);

    let mut route = route_with_rules(json!([
        {"matches": [{"method": {"method": "Get"}},
            {"method": {"type": "RegularExpression", "service": "private-marker[", "method": "Get"}}]},
        {"matches": [{"method": {}}]},
        {"matches": [{"method": {"method": "Get"}}]},
        {"matches": [{"method": {"type": "RegularExpression", "method": "^List.*$"}}]}
    ]));
    route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());
    let handler = GrpcRouteHandler::default();
    let errors = validate_grpc_route_if_enabled(&route);
    assert!(errors.is_empty(), "per-rule errors must not reject valid siblings");
    route = match handler.parse(route, &ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    };
    handler.update_status(&mut route, &ctx, &errors);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True"));
    assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
        && c.status == "True"
        && c.message.contains("invalid regular expression")));
    assert!(!conditions.iter().any(|c| c.message.contains("private-marker")));
    let published: GRPCRoute = serde_json::from_slice(&serde_json::to_vec(&route).unwrap()).unwrap();
    assert_eq!(published.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
    assert_eq!(published.effective_rules().unwrap().len(), 4);
    assert!(published.effective_rules().unwrap()[2].method_match_error().is_none());

    route.spec.rules.as_mut().unwrap().truncate(2);
    handler.update_status(&mut route, &ctx, &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "Accepted"
        && c.status == "False"
        && c.reason == "UnsupportedValue"
        && c.message.contains("rules[0]")));
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

    for rule in route.spec.rules.iter_mut().flatten() {
        rule.matches = None;
    }
    handler.update_status(&mut route, &ctx, &[]);
    assert!(route.spec.invalid_rule_indices.is_none());
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True"));
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

    // Delegation can leave no effective rules even when its authored
    // parent service scope is valid. That attachment must also reject.
    route.spec.resolved_rules = Some(vec![]);
    route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
        message: "child rule dropped: invalid method matcher".to_string(),
    }];
    handler.update_status(&mut route, &ctx, &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions
        .iter()
        .any(|c| c.type_ == "Accepted" && c.status == "False" && c.reason == "UnsupportedValue"));
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));
}

#[test]
fn duplicate_request_header_modifier_marks_rule_invalid() {
    let route = route_with_rules(json!([
        {
            "filters": [
                { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } },
                { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
            ]
        }
    ]));
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn response_header_policy_rejects_backend_mutation_and_preserves_request_direction() {
    let mut route = route_with_rules(json!([
        {"backendRefs": [{"name": "svc", "port": 80, "filters": [
            {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"remove": ["Content-Length"]}}
        ]}]},
        {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "Content-Length", "value": "123"}]
        }}]}
    ]));
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].filters = None;
    assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
}

#[test]
fn header_modifier_status_preserves_siblings_rejects_empty_route_and_recovers() {
    let mut route = route_with_rules(json!([
        {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "x-test", "value": "secret-marker\r\ninvalid"}]
        }}]},
        {"backendRefs": [{"name": "svc", "port": 80, "filters": [
            {"type": "ResponseHeaderModifier"}
        ]}]},
        {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "x-valid", "value": "yes"}]
        }}]}
    ]));
    route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());
    let handler = GrpcRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
        && c.status == "True"
        && c.message.starts_with("Dropped Rule")
        && c.message.contains("1 of 3 rules are active")));
    assert!(!conditions.iter().any(|c| c.message.contains("secret-marker")));

    route.spec.rules.as_mut().unwrap().truncate(2);
    handler.update_status(&mut route, &ctx, &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "Accepted"
        && c.status == "False"
        && c.message.contains("rules[0]")
        && c.message.contains("rules[1]")));
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

    route.spec.rules.as_mut().unwrap()[0].filters = None;
    route.spec.rules.as_mut().unwrap()[1].backend_refs.as_mut().unwrap()[0].filters = None;
    handler.update_status(&mut route, &ctx, &[]);
    assert!(route.spec.invalid_rule_indices.is_none());
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));
    assert!(!conditions.iter().any(|c| c.message.contains("rules[0]")));
}

#[test]
fn duplicate_response_header_modifier_marks_rule_invalid() {
    let route = route_with_rules(json!([
        {
            "filters": [
                { "type": "ResponseHeaderModifier", "responseHeaderModifier": { "set": [] } },
                { "type": "ResponseHeaderModifier", "responseHeaderModifier": { "remove": ["x"] } }
            ]
        }
    ]));
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn duplicate_filter_in_backend_ref_marks_rule_invalid() {
    let route = route_with_rules(json!([
        {
            "backendRefs": [
                {
                    "name": "svc-a",
                    "filters": [
                        { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } },
                        { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
                    ]
                }
            ]
        }
    ]));
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn repeated_extension_ref_is_allowed() {
    // ExtensionRef is explicitly allowed to appear multiple times.
    // ExtensionRef filters must point at a supported kind (EdgionPlugins) to be valid.
    let route = route_with_rules(json!([
        {
            "filters": [
                { "type": "ExtensionRef", "extensionRef": { "group": "edgion.io", "kind": "EdgionPlugins", "name": "a" } },
                { "type": "ExtensionRef", "extensionRef": { "group": "edgion.io", "kind": "EdgionPlugins", "name": "b" } }
            ]
        }
    ]));
    assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
}

#[test]
fn extension_ref_pairing_errors_drop_only_the_affected_grpc_rules() {
    let mut route = route_with_rules(json!([
        {"filters": [{"type": "ExtensionRef"}]},
        {"filters": [{"type": "RequestHeaderModifier", "extensionRef": {
            "group": "edgion.io", "kind": "EdgionPlugins", "name": "unexpected"
        }}]},
        {"backendRefs": [{"name": "svc", "port": 80, "filters": [
            {"type": "ExtensionRef"}
        ]}]},
        {"filters": [{"type": "ExtensionRef", "extensionRef": {
            "group": "edgion.io", "kind": "EdgionPlugins", "name": "valid"
        }}]}
    ]));
    route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());

    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0, 1, 2]);
    GrpcRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1, 2][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.starts_with("Dropped Rule")
    }));

    route.spec.rules.as_mut().unwrap().truncate(3);
    GrpcRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "Accepted" && condition.status == "False" && condition.message.contains("rules[0]")
    }));
    assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
}

#[test]
fn extension_ref_empty_group_is_not_edgion_io() {
    let route = route_with_rules(json!([{
        "filters": [{"type": "ExtensionRef", "extensionRef": {
            "group": "", "kind": "EdgionPlugins", "name": "plugins"
        }}]
    }]));
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn invalid_session_persistence_marks_grpc_rule_invalid() {
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "r", "namespace": "default" },
        "spec": {
            "rules": [{
                "sessionPersistence": { "absoluteTimeout": "1d" }
            }]
        }
    }))
    .expect("route should deserialize");
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn extension_ref_unsupported_kind_marks_rule_invalid() {
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "r", "namespace": "default" },
        "spec": {
            "rules": [{
                "filters": [{
                    "type": "ExtensionRef",
                    "extensionRef": { "group": "edgion.io", "kind": "NotAPlugin", "name": "x" }
                }]
            }]
        }
    }))
    .expect("route should deserialize");
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn extension_ref_unsupported_kind_on_backend_ref_marks_rule_invalid() {
    // The backendRef-level ExtensionRef check must also flag the rule invalid.
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "r", "namespace": "default" },
        "spec": {
            "rules": [{
                "backendRefs": [{
                    "name": "svc-a",
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": { "group": "edgion.io", "kind": "NotAPlugin", "name": "x" }
                    }]
                }]
            }]
        }
    }))
    .expect("route should deserialize");
    assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
}

#[test]
fn extension_ref_lb_policy_on_backend_ref_now_flagged_as_unsupported() {
    // Regression: the route-level extensionRef-based load-balancing-policy
    // mechanism was removed, so a formerly-exempt kind now fails the same
    // "only EdgionPlugins is supported" gate as any other unknown kind.
    let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "backendRefs": [{
                        "name": "svc-a",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": { "group": "edgion.io", "kind": "LBPolicyConsistentHash", "name": "header.x-user-id" }
                        }]
                    }]
                }]
            }
        }))
        .expect("route should deserialize");
    assert_eq!(
        super::find_invalid_rule_indices(&route, "default"),
        vec![0],
        "LBPolicy backendRef ExtensionRef must now be flagged invalid"
    );
}

#[test]
fn cross_level_dup_documents_intentional_conservative_scope() {
    // GRPC spec does not define cross-level (rule + backend) duplicate as conflict.
    // Same single-instance filter type appearing once in rule and once in backend is allowed.
    // This test pins the intentional scope choice — do not relax it without revisiting spec.
    let route = route_with_rules(json!([
        {
            "filters": [
                { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } }
            ],
            "backendRefs": [
                {
                    "name": "svc-a",
                    "filters": [
                        { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
                    ]
                }
            ]
        }
    ]));
    assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
}

// =====================================================================
// Delegation wiring tests (mirrors http_route.rs's delegation coverage)
// =====================================================================

#[test]
fn collect_backend_ref_tuples_excludes_delegation_refs() {
    // Regression: a delegating backendRef (kind: GRPCRoute) must never
    // reach validate_backend_refs — it is not a Service backend and would
    // otherwise be flagged ResolvedRefs=False/InvalidKind on a valid
    // delegating parent.
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "r", "namespace": "default" },
        "spec": {
            "rules": [{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "backendRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                    "name": "child"
                }]
            }]
        }
    }))
    .expect("route should deserialize");
    assert!(
        route_backend_refs::collect_dependencies(GrpcRouteBackendRefs::from_route(&route).unwrap().authored())
            .is_empty()
    );
}

#[test]
fn collect_ref_denied_list_uses_effective_rules() {
    // Regression: once resolved_rules is present, the parent's status
    // must scan the flattened copies (which carry the real ref_denied
    // state the gateway compiles), not the pre-flatten spec.rules.
    let mut route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "name": "r", "namespace": "default" },
        "spec": { "rules": [] }
    }))
    .expect("route should deserialize");
    route.spec.resolved_rules = Some(
        serde_json::from_value(json!([{
            "matches": [{"method": {"service": "svc.Billing"}}],
            "backendRefs": [{
                "name": "svc",
                "namespace": "other-ns",
                "port": 80,
                "refDenied": {
                    "targetNamespace": "other-ns",
                    "targetName": "svc",
                    "reason": "NoMatchingReferenceGrant"
                }
            }]
        }]))
        .expect("resolved_rules fixture should deserialize"),
    );
    let list = super::collect_ref_denied_list(&route);
    assert_eq!(list.len(), 1);
    assert!(
        list[0].is_some(),
        "ref_denied on resolved_rules must be picked up via effective_rules()"
    );
}

#[test]
fn delegation_accepted_condition_true_when_parent_delegates() {
    let delegating = vec!["infra/parent".to_string()];
    let cond = super::delegation_accepted_condition("infra/parent", &delegating, Some(3));
    assert_eq!(cond.status, "True");
    assert_eq!(cond.reason, "DelegationAccepted");
    assert!(cond.message.contains("infra/parent"));
}

#[test]
fn delegation_accepted_condition_false_when_parent_does_not_delegate() {
    let delegating: Vec<String> = vec![];
    let cond = super::delegation_accepted_condition("infra/parent", &delegating, Some(3));
    assert_eq!(cond.status, "False");
    assert_eq!(cond.reason, "NoMatchingParent");
}

#[tokio::test]
async fn parse_marks_ref_denied_before_flatten_copies_siblings() {
    // Regression (P0): flatten copies non-delegating sibling rules into
    // resolved_rules, which the gateway compiles INSTEAD of spec.rules.
    // The ReferenceGrant marking must therefore run BEFORE flatten so the
    // copies carry ref_denied — otherwise an ungranted cross-ns Service
    // ref on a delegating parent would be silently routable.
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [
                {
                    "matches": [{"method": {"service": "svc.Plain"}}],
                    "backendRefs": [{"name": "secret-svc", "namespace": "secret-ns", "port": 80}]
                },
                {
                    "matches": [{"method": {"service": "svc.Billing"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                        "name": "*", "namespace": "billing"
                    }]
                }
            ]
        }
    }))
    .unwrap();

    let handler = GrpcRouteHandler::default();
    let ctx = make_ctx();
    let ProcessResult::Continue(parsed) = handler.parse(route, &ctx).await else {
        panic!("parse must continue");
    };

    // No ReferenceGrant exists in the default runtime → the sibling's
    // cross-ns Service ref is denied on spec.rules...
    let spec_br = &parsed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0];
    assert!(spec_br.ref_denied.is_some(), "spec.rules sibling must be ref_denied");

    // ...AND on the resolved_rules copy the gateway actually compiles.
    let resolved = parsed
        .spec
        .resolved_rules
        .as_ref()
        .expect("delegating parent has resolved_rules");
    let resolved_sibling = resolved
        .iter()
        .find(|r| {
            r.matches
                .iter()
                .flatten()
                .any(|m| m.method.as_ref().and_then(|mm| mm.service.as_deref()) == Some("svc.Plain"))
        })
        .expect("sibling rule copied into resolved_rules");
    let resolved_br = &resolved_sibling.backend_refs.as_ref().unwrap()[0];
    assert!(
        resolved_br.ref_denied.is_some(),
        "resolved_rules sibling copy must carry ref_denied (gateway compiles this copy)"
    );
}

#[tokio::test]
async fn parse_never_marks_delegation_refs_ref_denied() {
    // Regression: a cross-namespace delegation backendRef (kind: GRPCRoute)
    // must not go through the ReferenceGrant check — delegation is
    // authorized by the parent opt-in, and a spurious ref_denied surfaced
    // as ResolvedRefs=False/RefNotPermitted on the parent status.
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "backendRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                    "name": "*", "namespace": "billing"
                }]
            }]
        }
    }))
    .unwrap();

    let handler = GrpcRouteHandler::default();
    let ctx = make_ctx();
    let result = handler.parse(route, &ctx).await;
    let ProcessResult::Continue(parsed) = result else {
        panic!("parse must continue");
    };
    let br = &parsed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0];
    assert!(
        br.ref_denied.is_none(),
        "delegation ref must not be ref_denied: {:?}",
        br.ref_denied
    );
    // And the tuple collection excludes it (no InvalidKind on status path).
    assert!(
        route_backend_refs::collect_dependencies(GrpcRouteBackendRefs::from_route(&parsed).unwrap().authored())
            .is_empty()
    );
}

#[tokio::test]
async fn parse_updates_grpc_route_delegation_index_and_on_delete_removes_it() {
    // Handler-level index bookkeeping: parse() must register the
    // delegation edge in grpc_route_delegation_index (separate instance
    // from route_delegation_index, so it doesn't collide with HTTPRoute
    // keys), and on_delete() must clear it via update_parent(∅,∅).
    let handler = GrpcRouteHandler::default();
    let ctx = make_ctx();
    let route: GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "backendRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                    "name": "child", "namespace": "billing"
                }]
            }]
        }
    }))
    .unwrap();

    let parsed = match handler.parse(route, &ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    };

    assert_eq!(
        ctx.runtime()
            .grpc_route_delegation_index
            .parents_of("billing/child", "billing"),
        vec!["infra/parent".to_string()],
        "parse must register the delegation edge in grpc_route_delegation_index"
    );

    handler.on_delete(&parsed, &ctx).await;

    assert!(
        ctx.runtime()
            .grpc_route_delegation_index
            .parents_of("billing/child", "billing")
            .is_empty(),
        "on_delete must remove the parent's delegation edges"
    );
}

#[tokio::test]
async fn delegated_effective_targets_drive_missing_invalid_and_denied_status() {
    use crate::conf_mgr::sync_runtime::resource_processor::{handlers::ServiceHandler, ResourceProcessor};
    use k8s_openapi::api::core::v1::Service;
    let runtime = Arc::new(ControllerRuntime::default());
    runtime
        .processor_registry
        .register(Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            16,
            Arc::new(ServiceHandler::new()),
            runtime.clone(),
        )));
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime);
    let mut route:GRPCRoute=serde_json::from_value(json!({
        "metadata":{"namespace":"apps","name":"root"},
        "spec":{"parentRefs":[{"name":"gateway"}],
            "rules":[{"backendRefs":[{"group":"gateway.networking.k8s.io","kind":"GRPCRoute","name":"child"}]}],
            "resolvedRules":[{"backendRefs":[
                {"name":"delegated-missing","namespace":"apps","port":80},
                {"name":"delegated-invalid","namespace":"apps","port":0},
                {"name":"delegated-denied","namespace":"other","port":80,"refDenied":{"targetNamespace":"other","targetName":"delegated-denied"}}
            ]}]}
    })).unwrap();
    let errors = route_backend_refs::validate_targets(
        GrpcRouteBackendRefs::from_route(&route).map(|references| references.effective()),
        ctx.runtime(),
    );
    assert_eq!(errors.len(), 2);
    assert!(errors
        .iter()
        .any(|error| matches!(error,ResolvedRefsError::BackendNotFound{name,..} if name=="delegated-missing")));
    assert!(errors
        .iter()
        .any(|error| matches!(error, ResolvedRefsError::InvalidReference { .. })));
    GrpcRouteHandler::default().update_status(&mut route, &ctx, &[]);
    let condition = route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(condition.status, "False");
    assert!(condition.message.contains("delegated-missing"));
    assert!(condition.message.contains("delegated-denied"));
}

#[tokio::test]
async fn extension_owner_default_and_invalid_status_paths_never_create_forged_dependencies() {
    let ctx = make_ctx();
    let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"extension-owner"},"spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"rule","resolvedNamespace":"forged"}}],"backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"backend"}}]}]}]}})).unwrap();
    let owner = GrpcRouteHandler::create_resource_ref(&route);
    GrpcRouteHandler::register_plugin_refs(&route, &ctx);
    let mut keys = ctx.runtime().plugin_ref_mgr.get_dependencies(&owner.key());
    keys.sort();
    assert_eq!(keys, vec!["default/backend", "default/rule"]);
    assert!(owner.namespace.is_none());
    route.metadata.namespace = Some("bad/ns".into());
    let invalid_owner = GrpcRouteHandler::create_resource_ref(&route);
    ctx.runtime()
        .plugin_ref_mgr
        .add_ref("stale/old".into(), invalid_owner.clone());
    GrpcRouteHandler::default().update_status(&mut route, &ctx, &[]);
    assert!(ctx
        .runtime()
        .plugin_ref_mgr
        .get_dependencies(&invalid_owner.key())
        .is_empty());
    assert!(route.spec.resolved_rules.as_ref().unwrap().is_empty());
    GrpcRouteHandler::register_plugin_refs(&route, &ctx);
    assert!(ctx
        .runtime()
        .plugin_ref_mgr
        .get_dependencies(&invalid_owner.key())
        .is_empty());
    for bad in ["", "bad/name"] {
        route.metadata.namespace = None;
        route.metadata.name = Some(bad.into());
        GrpcRouteHandler::register_plugin_refs(&route, &ctx);
        assert!(ctx
            .runtime()
            .plugin_ref_mgr
            .get_dependencies(&GrpcRouteHandler::create_resource_ref(&route).key())
            .is_empty());
    }
}

#[tokio::test]
async fn grpc_rule_transcoder_priority_precedes_backend_waf_and_request_id() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::EdgionPlugins;
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let filter =
        json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bundle"}});
    let route:GRPCRoute=serde_json::from_value(json!({"metadata":{"name":"route","namespace":"apps"},"spec":{"rules":[{"filters":[filter.clone()],"backendRefs":[{"name":"service","port":80,"filters":[filter]}]}]}})).unwrap();
    for enabled in [true, false, true] {
        let plugins:EdgionPlugins=serde_json::from_value(json!({"metadata":{"namespace":"apps","name":"bundle"},"spec":{"waf":{"policyRef":{"name":"policy"}},"requestPlugins":[{"type":"RequestId","config":{}},{"type":"GrpcJsonTranscoder","enable":enabled,"config":{"descriptor":{"type":"configMapRef","name":"descriptor","key":"proto"},"services":["test.TestService"]}}]}})).unwrap();
        raw.upsert("apps/bundle".into(), Arc::new(plugins));
        let error = backend_plugin_scope_invalid_reason(&route, 0, false, &runtime, false).unwrap();
        assert_eq!(
            error,
            if enabled {
                "GrpcJsonTranscoder requires HTTPRoute rule scope"
            } else {
                "backendRefs[0] EdgionPlugins ExtensionRef cannot contain WAF; attach WAF at Global, Gateway, or Route scope"
            }
        );
    }
}

#[tokio::test]
async fn plugin_collision_status_preserves_effective_rule_and_backend_positions() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::EdgionPlugins;
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let mut source = EdgionPlugins::new("plugins", Default::default());
    raw.upsert("plugins".into(), Arc::new(source.clone()));
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
    let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"route","namespace":"default"},
        "spec":{"parentRefs":[{"name":"gateway"}],"rules":[
            {"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}],
             "backendRefs":[{"name":"service","port":80,"weight":7,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}]}]},
            {"backendRefs":[{"name":"fallback","port":80}]}
        ]}
    })).unwrap();
    let handler = GrpcRouteHandler::default();
    handler.update_status(&mut route, &ctx, &[]);
    let before_rules = serde_json::to_value(route.effective_rules()).unwrap();
    let before_indices = route.spec.invalid_rule_indices.clone();
    let admission = |route: &GRPCRoute| {
        route
            .status
            .as_ref()
            .unwrap()
            .parents
            .iter()
            .flat_map(|p| &p.conditions)
            .filter(|c| matches!(c.type_.as_str(), "Accepted" | "PartiallyInvalid"))
            .map(|c| (c.type_.clone(), c.status.clone(), c.reason.clone(), c.message.clone()))
            .collect::<Vec<_>>()
    };
    let before_admission = admission(&route);
    source.metadata.namespace = Some("default".into());
    raw.upsert("default/plugins".into(), Arc::new(source));
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(serde_json::to_value(route.effective_rules()).unwrap(), before_rules);
    assert_eq!(route.spec.invalid_rule_indices, before_indices);
    assert_eq!(admission(&route), before_admission);
    let parents = &route.status.as_ref().unwrap().parents;
    assert!(parents
        .iter()
        .flat_map(|parent| parent.conditions.iter())
        .any(|condition| {
            condition.type_ == "ResolvedRefs" && condition.status == "False" && condition.message.contains("ambiguous")
        }));
    raw.remove("plugins");
    handler.update_status(&mut route, &ctx, &[]);
    assert!(!serde_json::to_string(&route.status).unwrap().contains("ambiguous"));
}
