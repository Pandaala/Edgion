use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use std::sync::Arc;

#[test]
fn v16_cors_and_retry_drift_invalidates_only_the_affected_rules() {
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "v16", "namespace": "default"},
        "spec": {"rules": [
            {"filters": [
                {"type": "CORS", "cors": {"allowOrigins": ["https://a.example"]}},
                {"type": "CORS", "cors": {"allowOrigins": ["https://b.example"]}}
            ]},
            {"retry": {"attempts": 0}},
            {"retry": {"codes": [503, 503]}},
            {"backendRefs": [{"name": "service", "port": 8080}]}
        ]}
    }))
    .unwrap();
    let invalid = find_invalid_rule_details(&route, "default");
    assert_eq!(invalid.len(), 3);
    assert!(invalid
        .iter()
        .any(|(index, reason)| *index == 0 && reason.contains("CORS filter cannot be repeated")));
    assert!(invalid
        .iter()
        .any(|(index, reason)| *index == 1 && reason.contains("at least 1")));
    assert!(invalid
        .iter()
        .any(|(index, reason)| *index == 2 && reason.contains("unique")));
}

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
    let handler = HttpRouteHandler::default();
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
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

#[tokio::test]
async fn default_gateway_rejection_keeps_owned_gateway_tracker_empty_until_corrected() {
    use super::super::{GatewayClassHandler, GatewayClassOwnership, GatewayHandler};
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::{
        gateway_is_owned, KubernetesStatusIdentity,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::{Gateway, GatewayClass};

    let runtime = Arc::new(ControllerRuntime::default());
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "edgion.io/gateway-controller".into(),
    });
    let classes = Arc::new(ResourceProcessor::new(
        "GatewayClass",
        16,
        Arc::new(GatewayClassHandler::new(GatewayClassOwnership::new(
            "edgion.io/gateway-controller".into(),
        ))),
        runtime.clone(),
    ));
    let gateways = Arc::new(ResourceProcessor::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(
            Some(GatewayClassOwnership::new("edgion.io/gateway-controller".into())),
            None,
        )),
        runtime.clone(),
    ));
    let routes = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        16,
        Arc::new(HttpRouteHandler::default()),
        runtime.clone(),
    ));
    let class_raw = Arc::new(LocalRawStore::new());
    let gateway_raw = Arc::new(LocalRawStore::new());
    let route_raw = Arc::new(LocalRawStore::new());
    classes.set_raw_store(class_raw.clone());
    gateways.set_raw_store(gateway_raw.clone());
    routes.set_raw_store(route_raw.clone());
    runtime.processor_registry.register(classes.clone());
    runtime.processor_registry.register(gateways.clone());
    runtime.processor_registry.register(routes.clone());

    let class: GatewayClass = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "uid": "class-uid"},
        "spec": {"controllerName": "edgion.io/gateway-controller"}
    }))
    .unwrap();
    class_raw.upsert("edge".into(), Arc::new(class.clone()));
    classes.on_init_apply(class, None).await;
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "gateway", "uid": "gateway-uid"},
        "spec": {"gatewayClassName": "edge", "listeners": [
            {"name": "http", "port": 8080, "protocol": "HTTP"}
        ]}
    }))
    .unwrap();
    gateway_raw.upsert("apps/gateway".into(), Arc::new(gateway.clone()));
    gateways.on_init_apply(gateway.clone(), None).await;
    assert!(gateway_is_owned(&gateway, &runtime));

    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "route", "uid": "route-uid"},
        "spec": {"parentRefs": [{"name": "gateway"}], "useDefaultGateways": "All",
            "rules": [{"backendRefs": [{"name": "service", "port": 80}]}]}
    }))
    .unwrap();
    route_raw.upsert("apps/route".into(), Arc::new(route.clone()));
    routes.on_init_apply(route.clone(), None).await;
    assert!(routes.get("apps/route").is_none());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        0
    );

    routes.on_init_done();
    async fn reconcile(routes: &Arc<ResourceProcessor<HTTPRoute>>, source: Option<HTTPRoute>) {
        routes.on_event_key("apps/route".into()).await;
        let queue = routes.workqueue();
        let item = tokio::time::timeout(std::time::Duration::from_secs(1), queue.dequeue())
            .await
            .expect("route event was not queued")
            .expect("route queue closed");
        assert_eq!(item.key, "apps/route");
        routes
            .process_work_item(&item.key, source, None, item.trigger_chain)
            .await;
        queue.done(&item.key);
    }

    async fn reconcile_gateway(gateways: &Arc<ResourceProcessor<Gateway>>, source: &Gateway, expected: i32) {
        let queue = gateways.workqueue();
        let item = tokio::time::timeout(std::time::Duration::from_secs(2), queue.dequeue())
            .await
            .expect("parent Gateway was not requeued")
            .expect("Gateway queue closed");
        assert_eq!(item.key, "apps/gateway");
        gateways
            .process_work_item(&item.key, Some(source.clone()), None, item.trigger_chain)
            .await;
        queue.done(&item.key);
        let reconciled = gateways.get("apps/gateway").expect("Gateway remains published");
        let listeners = reconciled.status.unwrap().listeners.unwrap();
        assert_eq!(listeners[0].attached_routes, expected);
    }

    gateways.on_event_key("apps/gateway".into()).await;
    reconcile_gateway(&gateways, &gateway, 0).await;

    route_raw.remove("apps/route");
    reconcile(&routes, None).await;
    assert!(routes.get("apps/route").is_none());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        0
    );
    assert_eq!(
        gateways.get("apps/gateway").unwrap().status.unwrap().listeners.unwrap()[0].attached_routes,
        0
    );

    route.spec.use_default_gateways = None;
    route_raw.upsert("apps/route".into(), Arc::new(route.clone()));
    reconcile(&routes, Some(route.clone())).await;
    assert!(routes.get("apps/route").is_some());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        1
    );
    reconcile_gateway(&gateways, &gateway, 1).await;

    route.spec.use_default_gateways = Some("All".into());
    route_raw.upsert("apps/route".into(), Arc::new(route.clone()));
    reconcile(&routes, Some(route.clone())).await;
    assert!(routes.get("apps/route").is_none());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        0
    );
    reconcile_gateway(&gateways, &gateway, 0).await;

    route.spec.use_default_gateways = None;
    route_raw.upsert("apps/route".into(), Arc::new(route.clone()));
    reconcile(&routes, Some(route)).await;
    assert!(routes.get("apps/route").is_some());
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "http"),
        1
    );
    reconcile_gateway(&gateways, &gateway, 1).await;
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

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
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

fn make_ctx_with_raw_ai_backend(namespace: &str, name: &str) -> HandlerContext {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackend",
        "metadata": {"namespace": namespace, "name": name},
        "spec": {"ai": {
            "provider": "OpenAI",
            "credentialPool": {"credentials": [{
                "name": "primary", "secretRef": {"name": "provider-key"}
            }]},
            "models": [{"name": "model-a"}]
        }}
    }))
    .unwrap();
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(format!("{namespace}/{name}"), Arc::new(backend));
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);
    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

fn runtime_with_ai_backends(raw_backends: &[(&str, serde_json::Value)]) -> Arc<ControllerRuntime> {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let runtime = Arc::new(ControllerRuntime::default());
    let backend_processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let raw_store = Arc::new(LocalRawStore::new());
    for (name, spec) in raw_backends {
        let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": "apps", "name": name},
            "spec": spec
        }))
        .expect("backend fixture must deserialize");
        raw_store.upsert(format!("apps/{name}"), Arc::new(backend));
    }
    backend_processor.set_raw_store(raw_store);
    runtime.processor_registry.register(backend_processor);
    runtime
}

fn invalid_ai_backend_spec() -> edgion_resources::resources::edgion_backend::EdgionBackendSpec {
    serde_json::from_value(serde_json::json!({
        "ai": {
            "provider": "OpenAI",
            "credentialPool": {"credentials": [{
                "name": "primary", "secretRef": {"name": "provider-key"}
            }]},
            "models": []
        }
    }))
    .expect("structurally deserializable invalid AI backend")
}

#[tokio::test]
async fn ai_rule_preserves_a_missing_service_as_an_invalid_share() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionPluginsHandler, ServiceHandler};
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPluginsSpec, RequestFilterEntry};
    use k8s_openapi::api::core::v1::Service;

    let runtime = runtime_with_ai_backends(&[(
        "models",
        serde_json::json!({
            "ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }
        }),
    )]);
    let service_processor = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        16,
        Arc::new(ServiceHandler::new()),
        runtime.clone(),
    ));
    service_processor.set_raw_store(Arc::new(LocalRawStore::new()));
    runtime.processor_registry.register(service_processor);
    let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler),
        runtime.clone(),
    ));
    let mut plugins = EdgionPlugins::new(
        "ai",
        EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::AiProxy(
                serde_json::from_value::<AiProxyConfig>(serde_json::json!({"api":"openaiChat"}))
                    .expect("explicit API config must use stable policy defaults"),
            ))]),
            ..Default::default()
        },
    );
    plugins.metadata.namespace = Some("apps".to_string());
    let plugin_raw_store = Arc::new(LocalRawStore::new());
    plugin_raw_store.upsert("apps/ai".to_string(), Arc::new(plugins.clone()));
    plugin_processor.set_raw_store(plugin_raw_store);
    plugin_processor.save(plugins, None);
    runtime.processor_registry.register(plugin_processor);

    let rule = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
            {"name": "missing-service", "port": 80}
        ]),
        serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
            }
        }]),
    );

    assert_eq!(runtime_ai_rule_invalid_reason(&rule, "apps", &runtime), None);
}

fn route_with_one_cors_rule(origins: &[&str]) -> HTTPRoute {
    let origins_yaml = origins
        .iter()
        .map(|o| format!("            - \"{o}\""))
        .collect::<Vec<_>>()
        .join("\n");
    let yaml = format!(
        r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins:
{origins_yaml}
"#
    );
    serde_yaml::from_str(&yaml).expect("HTTPRoute yaml must parse")
}

fn indices_only(details: Vec<(usize, String)>) -> Vec<usize> {
    details.into_iter().map(|(i, _)| i).collect()
}

/// Build a route whose first rule is an AI rule violating the retry cap,
/// optionally followed by a plain Service rule that stays valid.
fn route_with_bad_ai_rule(with_valid_second_rule: bool) -> HTTPRoute {
    let second_rule = if with_valid_second_rule {
        r#"
    - backendRefs:
        - name: web
          port: 80
"#
    } else {
        ""
    };
    let yaml = format!(
        r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  parentRefs:
    - name: gw
  rules:
    - backendRefs:
        - group: edgion.io
          kind: EdgionBackend
          name: chat
      retry:
        attempts: 8
{second_rule}"#
    );
    serde_yaml::from_str(&yaml).expect("HTTPRoute yaml must parse")
}

#[tokio::test]
async fn ai_source_errors_do_not_claim_gateway_api_rule_drops() {
    for with_sibling in [false, true] {
        let mut route = route_with_bad_ai_rule(with_sibling);
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");
        HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        let accepted = conditions
            .iter()
            .find(|condition| condition.type_ == "Accepted")
            .unwrap();
        assert!(!accepted.message.contains("retry.attempts"), "{accepted:?}");
        assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
        assert_eq!(route.spec.invalid_rule_indices, None);
    }
}

#[tokio::test]
async fn ai_admission_facts_replace_injected_metadata_after_flattening() {
    let ctx = make_ctx_with_raw_ai_backend("default", "chat");
    let mut route = route_with_bad_ai_rule(false);
    let rule = &mut route.spec.rules.as_mut().unwrap()[0];
    let mut zero = rule.backend_refs.as_ref().unwrap()[0].clone();
    zero.weight = Some(0);
    rule.backend_refs.as_mut().unwrap().push(zero);
    rule.resolved_ai_admission = Some(ResolvedAiAdmission {
        backends: vec![ResolvedForwardingTarget::Incompatible],
    });
    rule.resolved_parent_match =
        Some(edgion_resources::resources::http_route::ResolvedParentMatch::Delegated { matches: vec![] });
    // An empty wildcard delegation makes the original AI rule a flattened
    // sibling. Its intrinsic retry error must not erase the future gate.
    route.spec.rules.as_mut().unwrap().push(ai_rule(
        serde_json::json!([{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "*", "namespace": "children"
        }]),
        serde_json::json!([]),
    ));
    let ProcessResult::Continue(mut parsed) = HttpRouteHandler::default().parse(route, &ctx).await else {
        panic!("parse must continue");
    };
    let source = &parsed.spec.rules.as_ref().unwrap()[0];
    let resolved = parsed.spec.resolved_rules.as_ref().unwrap();
    assert!(
        source.resolved_parent_match.is_none(),
        "authored scope metadata is cleared"
    );
    assert!(
        matches!(
            resolved[0].resolved_parent_match,
            Some(edgion_resources::resources::http_route::ResolvedParentMatch::Direct {})
        ),
        "root-owned sibling receives fresh direct provenance"
    );
    assert_eq!(resolved.len(), 1, "the invalid AI sibling must remain matchable");
    for rule in [source, &resolved[0]] {
        let facts = rule.resolved_ai_admission.as_ref().unwrap();
        assert_eq!(facts.backends.len(), 2, "include the zero-weight slot");
        assert!(facts
            .backends
            .iter()
            .all(|fact| matches!(fact, ResolvedForwardingTarget::EdgionBackend { spec_valid: true, .. })));
    }
    HttpRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
    assert!(!parsed
        .spec
        .invalid_rule_indices
        .as_ref()
        .is_some_and(|indices| indices.contains(&0)));
    let conditions = &parsed.status.as_ref().unwrap().parents[0].conditions;
    assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
    assert!(conditions
        .iter()
        .filter(|condition| condition.type_ == "Accepted")
        .all(|condition| !condition.message.contains("retry.attempts")));
}

#[tokio::test]
async fn ai_source_diagnostic_never_claims_a_rule_drop() {
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx_with_raw_ai_backend("default", "chat");
    let mut route = route_with_bad_ai_rule(true);
    handler.update_status(&mut route, &ctx, &[]);
    route.spec.rules.as_mut().unwrap().truncate(1);
    handler.update_status(&mut route, &ctx, &[]);
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|condition| condition.type_ == "PartiallyInvalid"));
    assert_eq!(route.spec.invalid_rule_indices, None);

    // Replacing the invalid source with an ordinary Service rule removes
    // the intrinsic AI error without an attachment-policy decision.
    route.spec.rules = Some(vec![ai_rule(
        serde_json::json!([{"name": "web", "port": 80}]),
        serde_json::json!([]),
    )]);
    handler.update_status(&mut route, &ctx, &[]);
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|condition| condition.type_ == "PartiallyInvalid"));
}

fn ai_rule(backends: serde_json::Value, filters: serde_json::Value) -> HTTPRouteRule {
    serde_json::from_value(serde_json::json!({
        "backendRefs": backends,
        "filters": filters,
    }))
    .expect("AI rule must parse")
}

fn active_ai_proxies(active_count: usize) -> AiProxyAdmissionState {
    AiProxyAdmissionState {
        active_count,
        config: None,
    }
}

#[test]
fn ai_rule_requires_homogeneous_backends_and_exactly_one_ai_proxy() {
    let extension_ref = serde_json::json!([{
        "type": "ExtensionRef",
        "extensionRef": {
            "group": "edgion.io",
            "kind": "EdgionPlugins",
            "name": "ai",
            "resolvedNamespace": "child"
        }
    }]);
    let pure_ai = ai_rule(
        serde_json::json!([{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
        }]),
        extension_ref.clone(),
    );
    assert_eq!(
        ai_rule_invalid_reason(&pure_ai, "parent", |namespace, name| {
            assert_eq!((namespace, name), ("child", "ai"));
            Some(active_ai_proxies(1))
        }),
        None
    );

    let missing = ai_rule_invalid_reason(&pure_ai, "parent", |_, _| Some(active_ai_proxies(0))).unwrap();
    assert!(missing.contains("exactly one enabled AiProxy"), "{missing}");

    let mixed = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
            {"name": "ordinary", "port": 80}
        ]),
        extension_ref,
    );
    let reason = ai_rule_invalid_reason(&mixed, "parent", |_, _| Some(active_ai_proxies(1))).unwrap();
    assert!(reason.contains("must not be mixed"), "{reason}");
}

#[test]
fn zero_weight_refs_do_not_participate_in_ai_homogeneity() {
    let filters = serde_json::json!([{
        "type": "ExtensionRef",
        "extensionRef": {
            "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
        }
    }]);
    let rule = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1},
            {"name": "ordinary", "port": 80, "weight": 0}
        ]),
        filters,
    );
    assert!(ai_rule_invalid_reason(&rule, "parent", |_, _| Some(active_ai_proxies(1))).is_none());

    let no_positive_backend = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0},
            {"name": "ordinary", "port": 80, "weight": 1}
        ]),
        serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
            }
        }]),
    );
    assert!(ai_rule_invalid_reason(&no_positive_backend, "parent", |_, _| { Some(active_ai_proxies(1)) }).is_some());
}

#[test]
fn model_route_positive_override_promotes_zero_weight_slot_for_admission() {
    let rule = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0},
            {"name": "ordinary", "port": 80, "weight": 0}
        ]),
        serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"}
        }]),
    );
    let config: AiProxyConfig = serde_yaml::from_str(
        r#"
api: openaiChat
modelRoutes:
  - match: {model: virtual}
    action:
      candidates:
        - backend:
            group: edgion.io
            kind: EdgionBackend
            namespace: parent
            name: models
            occurrence: 0
          weight: 5
"#,
    )
    .unwrap();
    assert!(
        ai_rule_invalid_reason(&rule, "parent", |_, _| Some(AiProxyAdmissionState {
            active_count: 1,
            config: Some(config.clone()),
        }))
        .is_none()
    );
}

#[tokio::test]
async fn positive_weight_backend_requires_ai_capability() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use kube::api::ObjectMeta;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(
        "apps/models".to_string(),
        Arc::new(EdgionBackend {
            metadata: ObjectMeta {
                namespace: Some("apps".to_string()),
                name: Some("models".to_string()),
                ..Default::default()
            },
            spec: invalid_ai_backend_spec(),
            status: None,
        }),
    );
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);

    let positive = ai_rule(
        serde_json::json!([{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1
        }]),
        serde_json::json!([]),
    );
    assert!(backend_variant_invalid_reason(&positive, "apps", &runtime).is_some());

    let zero = ai_rule(
        serde_json::json!([{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0
        }]),
        serde_json::json!([]),
    );
    assert!(backend_variant_invalid_reason(&zero, "apps", &runtime).is_none());
}

#[tokio::test]
async fn model_route_checks_zero_weight_slot_identity_before_ignoring_target() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use kube::api::ObjectMeta;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
        "EdgionBackend",
        16,
        Arc::new(EdgionBackendHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert(
        "apps/models".to_string(),
        Arc::new(EdgionBackend {
            metadata: ObjectMeta {
                namespace: Some("apps".to_string()),
                name: Some("models".to_string()),
                ..Default::default()
            },
            spec: invalid_ai_backend_spec(),
            status: None,
        }),
    );
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);

    let candidate_config = |group: &str, kind: &str, name: &str, occurrence: u32| {
        serde_json::from_value::<AiProxyConfig>(serde_json::json!({
            "api": "openaiChat",
            "modelRoutes": [{
                "match": {"model": "virtual"},
                "action": {"candidates": [{"backend": {
                    "group": group,
                    "kind": kind,
                    "namespace": "apps",
                    "name": name,
                    "occurrence": occurrence
                }, "weight": 0}]}
            }]
        }))
        .unwrap()
    };

    let missing_rule = ai_rule(
        serde_json::json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]),
        serde_json::json!([]),
    );
    let reason = model_route_context_invalid_reason_for_config(
        &missing_rule,
        "apps",
        &runtime,
        &candidate_config("edgion.io", "EdgionBackend", "missing", 0),
    )
    .unwrap();
    assert!(reason.contains("must resolve to one backendRef slot"), "{reason}");

    for (label, backend, config) in [
        (
            "denied",
            serde_json::json!({
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
                "refDenied": {"targetNamespace": "apps", "targetName": "models"}
            }),
            candidate_config("edgion.io", "EdgionBackend", "models", 0),
        ),
        (
            "port",
            serde_json::json!({
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "port": 443
            }),
            candidate_config("edgion.io", "EdgionBackend", "models", 0),
        ),
        (
            "Service",
            serde_json::json!({"kind": "Service", "name": "models", "port": 80}),
            candidate_config("", "Service", "models", 0),
        ),
        (
            "unsupported",
            serde_json::json!({"group": "edgion.io", "kind": "UnsupportedBackend", "name": "models"}),
            candidate_config("edgion.io", "UnsupportedBackend", "models", 0),
        ),
    ] {
        let rule = ai_rule(serde_json::json!([backend]), serde_json::json!([]));
        let reason = model_route_context_invalid_reason_for_config(&rule, "apps", &runtime, &config).unwrap();
        assert!(
            reason.contains("denied")
                || reason.contains("must select an EdgionBackend")
                || reason.contains("must resolve to one backendRef slot"),
            "{label}: {reason}"
        );
    }

    let rule = ai_rule(
        serde_json::json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1},
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0}
        ]),
        serde_json::json!([]),
    );
    let config = candidate_config("edgion.io", "EdgionBackend", "models", 1);
    assert_eq!(
        model_route_context_invalid_reason_for_config(&rule, "apps", &runtime, &config),
        None,
        "a selected zero-weight source remains inert after slot and kind checks"
    );
}

#[tokio::test]
async fn ai_source_diagnostics_use_enabled_processed_entries_without_policy() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use edgion_resources::resources::edgion_plugins::{EdgionPluginsSpec, RequestFilterEntry};
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler),
        runtime.clone(),
    ));
    let config: AiProxyConfig = serde_json::from_value(serde_json::json!({"api":"openaiChat"})).unwrap();
    let mut plugins = EdgionPlugins::new(
        "ai",
        EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::AiProxy(config.clone()))]),
            ..Default::default()
        },
    );
    plugins.metadata.namespace = Some("apps".to_string());
    processor.save(plugins.clone(), None);
    runtime.processor_registry.register(processor.clone());
    assert_eq!(
        ai_proxy_admission_state(&runtime, "apps", "ai"),
        Some(AiProxyAdmissionState {
            active_count: 1,
            config: Some(config),
        })
    );
    plugins.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    processor.save(plugins, None);
    assert_eq!(
        ai_proxy_admission_state(&runtime, "apps", "ai"),
        Some(AiProxyAdmissionState::default())
    );
}

#[test]
fn ai_backend_filters_allow_only_header_modifiers() {
    let extension_ref = serde_json::json!([{
        "type": "ExtensionRef",
        "extensionRef": {
            "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
        }
    }]);
    let allowed = ai_rule(
        serde_json::json!([{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
            "filters": [
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-tenant", "value": "a"}]}},
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"set": [{"name": "x-ai", "value": "yes"}]}}
            ]
        }]),
        extension_ref.clone(),
    );
    assert!(ai_rule_invalid_reason(&allowed, "default", |_, _| Some(active_ai_proxies(1))).is_none());

    let rejected = ai_rule(
        serde_json::json!([{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
            "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {"type": "ReplaceFullPath", "replaceFullPath": "/v2"}}}]
        }]),
        extension_ref,
    );
    let reason = ai_rule_invalid_reason(&rejected, "default", |_, _| Some(active_ai_proxies(1))).unwrap();
    assert!(reason.contains("URLRewrite"), "{reason}");
}

#[test]
fn ai_rule_rejects_more_than_seven_explicit_retries() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "backendRefs": [{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
        }],
        "filters": [{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
            }
        }],
        "retry": {"attempts": 8}
    }))
    .expect("AI rule must parse");

    let reason = ai_rule_invalid_reason(&rule, "default", |_, _| Some(active_ai_proxies(1))).unwrap();
    assert!(reason.contains("must not exceed 7"), "{reason}");
}

#[tokio::test]
async fn ai_and_plugin_reverse_refs_replace_old_effective_dependencies() {
    let ctx = make_ctx();
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [{
                "filters": [{
                    "type": "ExtensionRef",
                    "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins",
                        "name": "parent-plugins"
                    }
                }]
            }],
            "resolvedRules": [{
                "backendRefs": [{
                    "group": "edgion.io", "kind": "EdgionBackend",
                    "name": "models-a", "namespace": "child",
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "group": "edgion.io", "kind": "EdgionPlugins",
                            "name": "backend-plugins", "resolvedNamespace": "child"
                        }
                    }]
                }],
                "filters": [{
                    "type": "ExtensionRef",
                    "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins",
                        "name": "ai-plugins", "resolvedNamespace": "child"
                    }
                }]
            }]
        }
    }))
    .unwrap();
    let route_ref = HttpRouteHandler::create_resource_ref(&route);

    let child: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "child", "name": "declaring-child"},
        "spec": {"rules": [{"backendRefs": [{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models-a"
        }]}]}
    }))
    .unwrap();
    let first_tuples =
        route_backend_refs::collect_dependencies(HttpRouteBackendRefs::from_route(&child).unwrap().authored());
    // Parse owns generation replacement; this pass tests the existing effective
    // projection's additive registration after that real entry boundary.
    HttpRouteHandler::default().parse(route.clone(), &ctx).await;
    let first = HttpRouteHandler::register_backend_refs(&route, &first_tuples, &ctx);
    assert_eq!(
        first.affected,
        vec!["edgion.io/EdgionBackend/child/models-a".to_string()]
    );
    HttpRouteHandler::register_plugin_refs(&route, &ctx);
    let mut dependencies = ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key());
    dependencies.sort();
    assert_eq!(
        dependencies,
        vec![
            "child/ai-plugins".to_string(),
            "child/backend-plugins".to_string(),
            "infra/parent-plugins".to_string()
        ]
    );

    let resolved = route.spec.resolved_rules.as_mut().unwrap();
    resolved[0].backend_refs.as_mut().unwrap()[0].name = "models-b".to_string();
    resolved[0].backend_refs.as_mut().unwrap()[0].filters = None;
    resolved[0].filters = None;
    let child: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "child", "name": "declaring-child"},
        "spec": {"rules": [{"backendRefs": [{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "models-b"
        }]}]}
    }))
    .unwrap();
    let second_tuples =
        route_backend_refs::collect_dependencies(HttpRouteBackendRefs::from_route(&child).unwrap().authored());
    let second = HttpRouteHandler::register_backend_refs(&route, &second_tuples, &ctx);
    assert_eq!(
        second.dropped,
        vec!["edgion.io/EdgionBackend/child/models-a".to_string()]
    );
    assert_eq!(second.affected.len(), 2);
    // Parse owns generation replacement; this pass tests the existing effective
    // projection's additive registration after that real entry boundary.
    HttpRouteHandler::default().parse(route.clone(), &ctx).await;
    HttpRouteHandler::register_plugin_refs(&route, &ctx);
    assert_eq!(
        ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
        vec!["infra/parent-plugins".to_string()],
        "operator-declared refs must survive removal from effective rules"
    );
}

#[tokio::test]
async fn backend_waf_is_controller_invalid_and_registered_for_requeue() {
    let ctx = make_ctx_with_raw_waf("apps", "waf");
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
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
    .expect("HTTPRoute fixture must deserialize");

    let details = find_controller_invalid_rule_details(&route, "apps", ctx.runtime());
    assert_eq!(details.len(), 1);
    assert!(details[0].1.contains("cannot contain WAF"), "{:?}", details[0]);
    let accepted = backend_route_wide_scope_errors(&route, ctx.runtime());
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].reason(), condition_reasons::UNSUPPORTED_VALUE);
    assert!(accepted[0].message().contains("rules[0]"));

    HttpRouteHandler::register_plugin_refs(&route, &ctx);
    let route_ref = HttpRouteHandler::create_resource_ref(&route);
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
    use edgion_resources::resources::Gateway;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
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
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"namespace": "default", "name": "gw"},
        "spec": {"gatewayClassName": "edgion", "listeners": [{"name": "http", "protocol": "HTTP", "port": 80}]}
    }))
    .unwrap();
    gateway_raw.upsert("default/gw".to_string(), Arc::new(gateway));
    gateway_processor.set_raw_store(gateway_raw);
    runtime.processor_registry.register(gateway_processor);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": "jump-scope"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
            {"backendRefs": [{"name": "bad", "port": 80, "filters": [{"type": "ExtensionRef",
                "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "jump"}}]}]},
            {"backendRefs": [{"name": "good", "port": 80}]}
        ]}
    }))
    .unwrap();
    HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"
        && condition.status == "True"
        && condition.message.contains("RegionRoute")));
    assert_eq!(route.spec.rules.as_ref().unwrap().len(), 2);
}

#[tokio::test]
async fn parse_clears_authored_extension_ref_namespace_metadata() {
    let ctx = make_ctx();
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
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
    .expect("HTTPRoute fixture must deserialize");

    let parsed = match HttpRouteHandler::default().parse(route, &ctx).await {
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

#[test]
fn flattened_backend_refs_remain_registered_after_rule_withdrawal() {
    let ctx = make_ctx();
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {"resolvedRules": []}
    }))
    .unwrap();
    let child: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "child", "name": "declaring-child"},
        "spec": {"rules": [{"backendRefs": [{
            "group": "edgion.io", "kind": "EdgionBackend", "name": "missing-or-denied"
        }]}]}
    }))
    .unwrap();
    let flattened_tuples =
        route_backend_refs::collect_dependencies(HttpRouteBackendRefs::from_route(&child).unwrap().authored());

    let delta = HttpRouteHandler::register_backend_refs(&route, &flattened_tuples, &ctx);

    assert_eq!(
        delta.affected,
        vec!["edgion.io/EdgionBackend/child/missing-or-denied".to_string()]
    );
    assert_eq!(
        ctx.runtime()
            .backend_ref_mgr
            .get_dependencies(&HttpRouteHandler::create_resource_ref(&route).key()),
        delta.affected
    );
}

#[tokio::test]
async fn parse_marks_ref_denied_before_flatten_copies_siblings() {
    // Regression (P0): flatten copies non-delegating sibling rules into
    // resolved_rules, which the gateway compiles INSTEAD of spec.rules.
    // The ReferenceGrant marking must therefore run BEFORE flatten so the
    // copies carry ref_denied — otherwise an ungranted cross-ns Service
    // ref on a delegating parent would be silently routable.
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [
                {
                    "matches": [{"path": {"type": "PathPrefix", "value": "/plain"}}],
                    "backendRefs": [{"name": "secret-svc", "namespace": "secret-ns", "port": 80}]
                },
                {
                    "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                        "name": "*", "namespace": "billing"
                    }]
                }
            ]
        }
    }))
    .unwrap();

    let handler = HttpRouteHandler::default();
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
                .any(|m| m.path.as_ref().and_then(|p| p.value.as_deref()) == Some("/plain"))
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
    // Regression: a cross-namespace delegation backendRef (kind: HTTPRoute)
    // must not go through the ReferenceGrant check — delegation is
    // authorized by the parent opt-in, and a spurious ref_denied surfaced
    // as ResolvedRefs=False/RefNotPermitted on the parent status.
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "parent", "namespace": "infra"},
        "spec": {
            "rules": [{
                "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                "backendRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                    "name": "*", "namespace": "billing"
                }]
            }]
        }
    }))
    .unwrap();

    let handler = HttpRouteHandler::default();
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
    // And HTTP-specific validation excludes it (no InvalidKind on status path).
    assert!(validate_http_backend_refs(&parsed, ctx.runtime()).is_empty());
}

#[test]
fn invalid_cors_origin_flags_rule_with_reason() {
    let route = route_with_one_cors_rule(&["evil.com"]);
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    let (_, reason) = &details[0];
    assert!(
        reason.contains("CORS") && reason.contains("evil.com"),
        "reason: {reason}"
    );
}

#[test]
fn cors_origin_with_path_flags_rule_with_reason() {
    let route = route_with_one_cors_rule(&["https://example.com/path"]);
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    let (_, reason) = &details[0];
    assert!(
        reason.contains("CORS") && reason.contains("https://example.com/path"),
        "reason: {reason}"
    );
}

#[test]
fn cors_filter_without_cors_block_flags_rule_with_reason() {
    // `type: CORS` with no `cors:` config — gateway-side silently drops the
    // filter and CORS protection is off. Must be flagged.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    assert!(
        details[0].1.contains("missing 'cors' configuration block"),
        "reason: {}",
        details[0].1
    );
}

#[test]
fn valid_cors_does_not_flag_rule() {
    let route = route_with_one_cors_rule(&["https://good.example.com", "https://*.foo.com:8443"]);
    assert!(find_invalid_rule_details(&route, "default").is_empty());
}

#[test]
fn native_cors_v15_validation_reasons_reach_controller_rule_status() {
    for (cors, expected_field) in [
        (
            serde_json::json!({"allowOrigins": ["ftp://example.com"]}),
            "allowOrigins",
        ),
        (serde_json::json!({"allowMethods": ["get"]}), "allowMethods"),
        (serde_json::json!({"allowHeaders": ["bad header"]}), "allowHeaders"),
        (serde_json::json!({"exposeHeaders": [":method"]}), "exposeHeaders"),
        (serde_json::json!({"maxAge": 0}), "maxAge"),
    ] {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "r1", "namespace": "default"},
            "spec": {"rules": [{"filters": [{"type": "CORS", "cors": cors}]}]}
        }))
        .expect("HTTPRoute CORS fixture must deserialize");
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(
            details[0].1.contains(expected_field),
            "expected {expected_field} in controller diagnostic: {}",
            details[0].1
        );
    }
}

#[test]
fn no_cors_filter_does_not_flag_rule() {
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-test
                value: "1"
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    assert!(find_invalid_rule_details(&route, "default").is_empty());
}

#[test]
fn invalid_cors_on_backend_ref_flags_rule_with_backendref_prefix() {
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: CORS
              cors:
                allowOrigins:
                  - "no-scheme.example.com"
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    assert!(
        details[0].1.starts_with("backendRefs[0] CORS"),
        "expected backendRef-prefixed reason; got: {}",
        details[0].1
    );
}

#[test]
fn invalid_cors_only_flags_the_offending_rule() {
    // Two rules: rule[0] has bad CORS, rule[1] has valid CORS. Only rule[0]
    // should be flagged so the route stays partially active.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["bad.example.com"]
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["https://good.example.com"]
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(indices_only(find_invalid_rule_details(&route, "default")), vec![0]);
}

#[test]
fn credentialed_expose_headers_status_rejection_and_recovery() {
    let mut route = route_with_one_cors_rule(&["https://app.example"]);
    route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
    let valid = route.spec.rules.as_ref().unwrap()[0].clone();
    let config = route.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
        .cors
        .as_mut()
        .unwrap();
    config.allow_credentials = Some(true);
    config.expose_headers = Some(vec!["*".into()]);
    route.spec.rules.as_mut().unwrap().push(valid);
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
        && c.status == "True"
        && c.message.contains("exposeHeaders")
        && c.message.contains("1 of 2 rules active")));
    route.spec.rules.as_mut().unwrap().truncate(1);
    handler.update_status(&mut route, &ctx, &[]);
    assert!(route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|c| c.type_ == "Accepted"
                && c.status == "False"
                // This unit fixture has no Gateway; its missing-parent reason takes precedence.
                && c.reason == "NoMatchingParent"
                && c.message.contains("exposeHeaders")));
    route.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
        .cors
        .as_mut()
        .unwrap()
        .expose_headers = Some(vec!["X-Request-Id".into()]);
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices, None);
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|c| c.message.contains("exposeHeaders")));
}

#[test]
fn backend_cors_scope_status_clears_after_policy_moves_to_rule() {
    let mut route = route_with_one_cors_rule(&["https://app.example"]);
    route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
    let rules = route.spec.rules.as_mut().unwrap();
    let valid = rules[0].clone();
    rules[0].backend_refs = Some(
        serde_json::from_value(serde_json::json!([
            {"name": "svc", "port": 80, "filters": [
                {"type": "CORS", "cors": {"allowOrigins": ["https://app.example"]}}
            ]}
        ]))
        .unwrap(),
    );
    rules.push(valid);
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
    let partial = route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .find(|condition| condition.type_ == "PartiallyInvalid")
        .unwrap();
    assert_eq!(partial.status, "True");
    assert_eq!(partial.reason, "RuleInvalid");
    assert!(partial.message.contains("backendRefs[0] CORS is not supported"));
    assert!(partial.message.contains("1 of 2 rules active"));

    // Removing the valid sibling must not clear the diagnostic as AllRulesValid.
    route.spec.rules.as_mut().unwrap().truncate(1);
    handler.update_status(&mut route, &ctx, &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert!(accepted.message.contains("backendRefs[0] CORS is not supported"));
    assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

    route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].filters = None;
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices, None);
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|condition| condition.message.contains("CORS is not supported")));
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|condition| condition.type_ == "PartiallyInvalid" && condition.status == "True"));
}

#[test]
fn backend_cors_scope_delegation_diagnostic_survives_attachment_rejection() {
    let mut route = route_with_one_cors_rule(&["https://app.example"]);
    let valid = route.spec.rules.as_ref().unwrap()[0].clone();
    route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
    route.spec.rules = Some(
        serde_json::from_value(serde_json::json!([{
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "child"}]
        }]))
        .unwrap(),
    );
    route.spec.resolved_rules = Some(vec![]);
    let diagnostic = "child 'default/child' rules[0] dropped: backendRefs[0] CORS is not supported; configure CORS in rules[].filters instead";
    route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
        message: diagnostic.to_string(),
    }];
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert!(accepted.message.contains(diagnostic));
    assert!(!conditions.iter().any(|c| c.reason == "AllRulesValid"));

    // With a surviving effective rule, diagnostics belong to PartiallyInvalid.
    route.spec.resolved_rules = Some(vec![valid]);
    handler.update_status(&mut route, &ctx, &[]);
    assert!(route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|c| c.type_ == "PartiallyInvalid" && c.status == "True" && c.message.contains(diagnostic)));
    route.spec.delegation_issues.clear();
    handler.update_status(&mut route, &ctx, &[]);
    assert!(!route.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|c| c.message.contains("CORS is not supported")));
}

#[test]
fn invalid_session_persistence_flags_only_offending_rule() {
    // rule[0]: Permanent lifetime without absoluteTimeout -> invalid.
    // rule[1]: valid session persistence. Only rule[0] is flagged.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - sessionPersistence:
        cookieConfig:
          lifetimeType: Permanent
    - sessionPersistence:
        absoluteTimeout: 1h
        cookieConfig:
          lifetimeType: Permanent
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    assert!(
        details[0].1.contains("absoluteTimeout"),
        "expected absoluteTimeout reason; got: {}",
        details[0].1
    );
}

#[test]
fn invalid_session_persistence_bad_duration_flags_rule() {
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - sessionPersistence:
        absoluteTimeout: 1d
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    assert!(details[0].1.contains("sessionPersistence"), "got: {}", details[0].1);
}

#[test]
fn invalid_cors_results_in_partially_invalid_not_accepted_false() {
    // End-to-end: handler.update_status on a route with bad CORS must produce
    // PartiallyInvalid=True, Accepted=True. This is the contract that protects
    // against the upgrade-time outage risk: bad CORS must not nuke the whole
    // route from the gateway route table.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
  generation: 7
spec:
  parentRefs:
    - name: gw
      namespace: default
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["bad.example.com"]
    - filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-test
                value: "1"
"#;
    let handler = HttpRouteHandler::default();
    let mut route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    handler.update_status(&mut route, &make_ctx(), &[]);

    let parents = &route.status.as_ref().expect("status").parents;
    assert_eq!(parents.len(), 1);
    let conds = &parents[0].conditions;
    let accepted = conds.iter().find(|c| c.type_ == "Accepted").expect("Accepted set");
    let partial = conds
        .iter()
        .find(|c| c.type_ == "PartiallyInvalid")
        .expect("PartiallyInvalid set");
    // Note: Accepted may be False here because the parent Gateway 'gw' doesn't
    // exist in the test runtime (NoMatchingParent). What matters is that the
    // bad-CORS rule did NOT push the route into Accepted=False via reason=Invalid —
    // it should be marked as a rule-level problem via PartiallyInvalid.
    assert_ne!(
        accepted.reason, "Invalid",
        "bad CORS must not produce Accepted=False with reason=Invalid (would drop whole route); got: {accepted:?}"
    );
    assert_eq!(
        partial.status, "True",
        "PartiallyInvalid must be True; got: {partial:?}"
    );
    assert_eq!(partial.reason, "RuleInvalid");
    assert_eq!(
        route.spec.invalid_rule_indices.as_deref(),
        Some(&[0usize][..]),
        "invalid_rule_indices must contain the bad-CORS rule index"
    );
    // Operator-debuggability: the PartiallyInvalid message must carry both
    // the rule index and the CORS reason so operators don't have to grep
    // gateway logs to find out which origin was rejected.
    assert!(
        partial.message.contains("rules[0]") && partial.message.contains("CORS"),
        "PartiallyInvalid message must include rule index + CORS reason; got: {}",
        partial.message
    );
    assert!(
        partial.message.contains("1 of 2 rules active"),
        "PartiallyInvalid message must include valid/total rule count; got: {}",
        partial.message
    );
}

#[test]
fn external_auth_response_header_policy_status_and_recovery() {
    use crate::conf_mgr::conf_center::common::parse_resource_content;

    for backend_scope in [false, true] {
        let filter = serde_json::json!({"type": "ExternalAuth", "externalAuth": {
            "target": {"name": "auth", "port": 8080},
            "decision": {"clientHeaders": ["CoNtEnT-LeNgTh"]}
        }});
        let mut invalid = serde_json::json!({});
        if backend_scope {
            invalid["backendRefs"] = serde_json::json!([
                {"name": "svc", "port": 80, "filters": [filter]}
            ]);
        } else {
            invalid["filters"] = serde_json::json!([filter]);
        }
        let input = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"name": "auth", "namespace": "default"},
            "spec": {"parentRefs": [{"name": "gw"}], "rules": [invalid, {}]}
        });
        // Exercise the common FileSystem/etcd decoding boundary.
        let mut route: HTTPRoute = parse_resource_content(&serde_yaml::to_string(&input).unwrap()).unwrap();
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
            && c.status == "True"
            && c.message.contains("ExternalAuth")
            && c.message.contains("1 of 2 rules active")));
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(details[0].1.contains("backendRefs[0]"), backend_scope);

        route.spec.rules.as_mut().unwrap().truncate(1);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions
            .iter()
            .any(|c| c.type_ == "Accepted" && c.status == "False" && c.message.contains("ExternalAuth")));
        assert!(!conditions
            .iter()
            .any(|c| c.type_ == "PartiallyInvalid" && c.status == "True"));

        let rule = &mut route.spec.rules.as_mut().unwrap()[0];
        let filters = if backend_scope {
            rule.backend_refs.as_mut().unwrap()[0].filters.as_mut().unwrap()
        } else {
            rule.filters.as_mut().unwrap()
        };
        let edgion_resources::HTTPRouteExternalAuth::Edgion(config) = filters[0].external_auth.as_mut().unwrap() else {
            panic!("expected Edgion ForwardAuth")
        };
        config.decision.client_headers = vec![
            "WWW-Authenticate".into(),
            "Set-Cookie".into(),
            "Content-Encoding".into(),
        ];
        handler.update_status(&mut route, &ctx, &[]);
        assert!(route.spec.invalid_rule_indices.is_none());
        assert!(find_invalid_rule_details(&route, "default").is_empty());
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|c| c.message.contains("ExternalAuth")));
    }
}

#[test]
fn beta_upstream_external_auth_invalidates_only_its_rule() {
    use crate::conf_mgr::conf_center::common::parse_resource_content;

    let mut source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "auth", "namespace": "default", "resourceVersion": "17"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
            {"filters": [{"type": "ExternalAuth", "externalAuth": {
                "backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP", "http": {"path": "/check"}
            }}], "backendRefs": [{"name": "svc", "port": 80}]},
            {"backendRefs": [{"name": "other", "port": 80}]}
        ]}
    });
    for auth in [
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080},
                "protocol": "HTTP", "http": {"path": "/check"}}),
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080},
                "protocol": "GRPC", "grpc": {"allowedHeaders": ["Authorization"]}}),
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080},
                "protocol": "HTTP", "http": {"path": "/check"},
                "decision": {"clientHeaders": []}}),
    ] {
        source["spec"]["rules"][0]["filters"][0]["externalAuth"] = auth;
        let mut route: HTTPRoute = parse_resource_content(&source.to_string()).unwrap();
        assert_eq!(route.metadata.resource_version.as_deref(), Some("17"));
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.contains("ExternalAuth")));
        assert_eq!(route.spec.rules.as_ref().unwrap().len(), 2);

        route.spec.rules.as_mut().unwrap().remove(1);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| condition.type_ == "Accepted"
            && condition.status == "False"
            && condition.message.contains("ExternalAuth")));
    }
}

#[test]
fn beta_default_gateway_attachment_is_explicitly_rejected() {
    use crate::conf_mgr::conf_center::common::parse_resource_content;
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "default", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gw"}], "useDefaultGateways": "All", "rules": [{}]}
    });
    let route: HTTPRoute = parse_resource_content(&source.to_string()).unwrap();
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    let errors = handler.validate(&route, &ctx);
    assert!(errors.iter().any(|error| error.contains("useDefaultGateways")));
    assert!(!handler.should_publish(&route, &ctx));

    let mut without_parent = route.clone();
    without_parent.spec.parent_refs = None;
    assert!(!handler.should_publish(&without_parent, &ctx));

    let mut none = source;
    none["spec"]["useDefaultGateways"] = serde_json::json!("None");
    let none: HTTPRoute = parse_resource_content(&none.to_string()).unwrap();
    assert!(handler.should_publish(&none, &ctx));
}

#[test]
fn response_header_policy_status_recovers_without_restricting_request_headers() {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "response-policy", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
            {"filters": [{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
                "set": [{"name": "CoNtEnT-LeNgTh", "value": "123"}]
            }}]},
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "Content-Length", "value": "123"}]
            }}]}
        ]}
    }))
    .unwrap();
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
    let filter = &mut route.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0];
    filter.response_header_modifier.as_mut().unwrap().set.as_mut().unwrap()[0].name = "Content-Type".into();
    handler.update_status(&mut route, &ctx, &[]);
    assert!(route.spec.invalid_rule_indices.is_none());
}

#[test]
fn header_modifier_status_preserves_siblings_rejects_empty_route_and_recovers() {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "headers", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "bad name", "value": "secret-marker"}]
            }}]},
            {"backendRefs": [{"name": "svc", "port": 80, "filters": [
                {"type": "ResponseHeaderModifier"}
            ]}]},
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "x-valid", "value": "yes"}]
            }}]}
        ]}
    }))
    .unwrap();
    let handler = HttpRouteHandler::default();
    let ctx = make_ctx();
    handler.update_status(&mut route, &ctx, &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    let partial = conditions.iter().find(|c| c.type_ == "PartiallyInvalid").unwrap();
    assert_eq!(partial.status, "True");
    assert!(partial.message.contains("rules[0]") && partial.message.contains("rules[1]"));
    assert!(partial.message.starts_with("Dropped Rule"));
    assert!(partial.message.contains("1 of 3 rules active"));
    assert!(!partial.message.contains("secret-marker"));

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
fn extension_ref_unsupported_kind_marks_rule_invalid() {
    // A rule with an ExtensionRef filter pointing at a non-EdgionPlugins
    // kind must be flagged invalid.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: NotAPlugin
            name: x
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(
        indices_only(details.clone()),
        vec![0],
        "rule 0 must be flagged: {details:?}"
    );
    assert!(
        details[0].1.contains("ExtensionRef") && details[0].1.contains("NotAPlugin"),
        "reason must name the unsupported kind: {}",
        details[0].1
    );
}

#[test]
fn extension_ref_edgion_plugins_does_not_flag_rule() {
    // A valid EdgionPlugins ExtensionRef must use the explicit edgion.io group.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    assert!(find_invalid_rule_details(&route, "default").is_empty());
}

#[test]
fn extension_ref_pairing_errors_drop_only_the_affected_http_rules() {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "extensions", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gw"}], "rules": [
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
        ]}
    }))
    .expect("HTTPRoute fixture must deserialize");

    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details), vec![0, 1, 2]);

    HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1, 2][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.starts_with("Dropped Rule")
    }));

    route.spec.rules.as_mut().unwrap().truncate(3);
    HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "Accepted" && condition.status == "False" && condition.message.contains("rules[0]")
    }));
    assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
}

#[test]
fn extension_ref_empty_group_is_not_edgion_io() {
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "r", "namespace": "default"},
        "spec": {"rules": [{"filters": [{"type": "ExtensionRef", "extensionRef": {
            "group": "", "kind": "EdgionPlugins", "name": "plugins"
        }}]}]}
    }))
    .expect("HTTPRoute fixture must deserialize");
    assert_eq!(indices_only(find_invalid_rule_details(&route, "default")), vec![0]);
}

#[test]
fn extension_ref_unsupported_kind_on_backend_ref_marks_rule_invalid() {
    // An ExtensionRef filter with an unsupported kind on a backendRef must be
    // flagged, and the reason must carry the per-backendRef prefix.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: NotAPlugin
                name: x
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(
        indices_only(details.clone()),
        vec![0],
        "rule 0 must be flagged: {details:?}"
    );
    assert!(
        details[0].1.contains("backendRefs["),
        "reason must carry the per-backendRef prefix: {}",
        details[0].1
    );
}

#[test]
fn extension_ref_wrong_group_with_edgion_plugins_kind_marks_rule_invalid() {
    // kind is EdgionPlugins but the group is wrong — must still be flagged,
    // proving the group_ok condition is actually ANDed in, not dead code.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: example.com
            kind: EdgionPlugins
            name: my-plugins
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(
        indices_only(details.clone()),
        vec![0],
        "rule 0 must be flagged: {details:?}"
    );
    assert!(
        details[0].1.contains("ExtensionRef") && details[0].1.contains("example.com"),
        "reason must name the wrong group: {}",
        details[0].1
    );
}

#[test]
fn extension_ref_lb_policy_on_backend_ref_now_flagged_as_unsupported() {
    // The route-level extensionRef LB mechanism was removed: `LBPolicy*`
    // kinds are no longer a recognized second use of the ExtensionRef
    // filter type. A `LBPolicyConsistentHash` backendRef ExtensionRef now
    // fails the same "only EdgionPlugins is supported" gate as any other
    // unknown kind. Load-balancing policy is configured via
    // `EdgionBackendTrafficPolicy` instead.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyConsistentHash
                name: header.x-user-id
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(
        indices_only(details.clone()),
        vec![0],
        "LBPolicy backendRef ExtensionRef must now be flagged invalid: {details:?}"
    );
    assert!(
        details[0].1.contains("LBPolicyConsistentHash"),
        "reason must name the unsupported kind: {}",
        details[0].1
    );
}

#[test]
fn extension_ref_lb_policy_at_rule_level_now_flagged_as_unsupported() {
    // Same removal as above, exercised at rule level (filters[]) instead of
    // backendRef level.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: LBPolicyConsistentHash
            name: header.x-user-id
      backendRefs:
        - name: svc
          port: 80
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(
        indices_only(details.clone()),
        vec![0],
        "rule-level LBPolicy ExtensionRef must now be flagged invalid: {details:?}"
    );
    assert!(
        details[0].1.contains("LBPolicyConsistentHash"),
        "reason must name the unsupported kind: {}",
        details[0].1
    );
}

#[test]
fn rewrite_redirect_conflict_still_flags_with_reason() {
    // Regression: existing rewrite/redirect-conflict precedent still flags
    // the rule, now also carrying a reason string in the PartiallyInvalid
    // message rather than just an index list.
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplaceFullPath
              replaceFullPath: /a
        - type: RequestRedirect
          requestRedirect:
            scheme: https
"#;
    let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0]);
    assert!(details[0].1.contains("httpRouteFilterRequestRedirect"));
    assert!(details[0].1.contains("httpRouteFilterRequestRewrite"));
}

#[test]
fn request_transform_validation_preserves_valid_siblings() {
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "transforms", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gateway"}], "rules": [
            {"filters": [{"type": "URLRewrite", "urlRewrite": {
                "path": {"type": "ReplaceFullPath", "replaceFullPath": "/v1"}
            }}]},
            {"matches": [{"path": {"type": "Exact", "value": "/api"}}],
             "filters": [{"type": "URLRewrite", "urlRewrite": {
                "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"}
             }}]},
            {"backendRefs": [{"name": "backend", "filters": [
                {"type": "RequestRedirect", "requestRedirect": {"scheme": "ftp"}}
            ]}]},
            {"filters": [{"type": "RequestRedirect", "requestRedirect": {
                "scheme": "https", "statusCode": 308
            }}]}
        ]}
    }))
    .unwrap();
    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![1, 2]);
    assert!(details[0].1.contains("PathPrefix"));

    let mut route = route;
    HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[1, 2][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.contains("2 of 4 rules active")
    }));
}

#[test]
fn invalid_request_mirror_fields_drop_only_the_owning_rules() {
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "mirrors", "namespace": "default"},
        "spec": {"parentRefs": [{"name": "gateway"}], "rules": [
            {"filters": [{"type": "RequestMirror", "requestMirror": {
                "backendRef": {"name": "shadow", "port": 8080},
                "fraction": {"numerator": 1}, "percent": 50
            }}]},
            {"backendRefs": [{"name": "primary", "port": 8080, "filters": [
                {"type": "RequestMirror", "requestMirror": {
                    "backendRef": {"name": "shadow"}
                }}
            ]}]},
            {"filters": [{"type": "RequestMirror", "requestMirror": {
                "backendRef": {"name": "shadow", "port": 8080}, "percent": 100
            }}]}
        ]}
    }))
    .unwrap();

    let details = find_invalid_rule_details(&route, "default");
    assert_eq!(indices_only(details.clone()), vec![0, 1]);
    assert!(details[0].1.contains("only one of fraction or percent"));
    assert!(details[1].1.contains("backendRef.port is required"));

    let mut route = route;
    HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
    assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
    let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.contains("1 of 3 rules active")
    }));
}

#[test]
fn native_ai_facts_keep_slots_and_skip_invalid_or_denied_target_reads() {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"route","namespace":"apps"},
        "spec":{"rules":[{"backendRefs":[
            {"name":"service","port":80,"weight":3},
            {"name":"service","port":0,"weight":7},
            {"group":"core","name":"service","port":80,"weight":5},
            {"group":"edgion.io","kind":"EdgionBackend","name":"models","namespace":"other","weight":2,
             "refDenied":{"targetNamespace":"other","targetName":"models"}},
            {"group":"edgion.io","kind":"EdgionBackend","name":"models","port":80,"weight":4},
            {"group":"edgion.io","kind":"EdgionBackend","name":"bad/name","weight":6}
        ]}]}
    }))
    .unwrap();
    let refs = HttpRouteBackendRefs::from_route(&route).unwrap();
    let mut reads = 0;
    for reference in refs.effective() {
        let slot = reference.slot();
        let fact = resolve_ai_admission_with(reference, |_| {
            reads += 1;
            ResolvedForwardingTarget::Service
        });
        assert_eq!(matches!(fact, ResolvedForwardingTarget::Service), slot.backend() == 0);
    }
    assert_eq!(reads, 1);
    route.spec.resolved_rules = route.spec.rules.clone();
    populate_ai_admission(&mut route, &ControllerRuntime::default());
    for rule in [
        &route.spec.rules.as_ref().unwrap()[0],
        &route.spec.resolved_rules.as_ref().unwrap()[0],
    ] {
        let facts = &rule.resolved_ai_admission.as_ref().unwrap().backends;
        assert_eq!(facts.len(), 6);
        assert!(matches!(facts[0], ResolvedForwardingTarget::Service));
        assert!(facts[1..]
            .iter()
            .all(|fact| matches!(fact, ResolvedForwardingTarget::Missing)));
        assert_eq!(
            rule.backend_refs
                .as_ref()
                .unwrap()
                .iter()
                .map(|reference| reference.weight)
                .collect::<Vec<_>>(),
            vec![Some(3), Some(7), Some(5), Some(2), Some(4), Some(6)]
        );
    }
    route.metadata.name = Some("bad/name".into());
    populate_ai_admission(&mut route, &ControllerRuntime::default());
    for rule in [
        &route.spec.rules.as_ref().unwrap()[0],
        &route.spec.resolved_rules.as_ref().unwrap()[0],
    ] {
        assert_eq!(rule.resolved_ai_admission.as_ref().unwrap().backends.len(), 6);
        assert!(rule
            .resolved_ai_admission
            .as_ref()
            .unwrap()
            .backends
            .iter()
            .all(|fact| matches!(fact, ResolvedForwardingTarget::Missing)));
    }
}

#[tokio::test]
async fn extension_owner_default_and_invalid_status_paths_never_create_forged_dependencies() {
    let ctx = make_ctx();
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"extension-owner"},"spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"rule","resolvedNamespace":"forged"}}],"backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"backend"}}]}]}]}})).unwrap();
    let owner = HttpRouteHandler::create_resource_ref(&route);
    HttpRouteHandler::register_plugin_refs(&route, &ctx);
    let mut keys = ctx.runtime().plugin_ref_mgr.get_dependencies(&owner.key());
    keys.sort();
    assert_eq!(keys, vec!["default/backend", "default/rule"]);
    assert!(owner.namespace.is_none());
    route.metadata.namespace = Some("bad/ns".into());
    let invalid_owner = HttpRouteHandler::create_resource_ref(&route);
    ctx.runtime()
        .plugin_ref_mgr
        .add_ref("stale/old".into(), invalid_owner.clone());
    HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);
    assert!(ctx
        .runtime()
        .plugin_ref_mgr
        .get_dependencies(&invalid_owner.key())
        .is_empty());
    assert!(route.spec.resolved_rules.as_ref().unwrap().is_empty());
    HttpRouteHandler::register_plugin_refs(&route, &ctx);
    assert!(ctx
        .runtime()
        .plugin_ref_mgr
        .get_dependencies(&invalid_owner.key())
        .is_empty());
    for bad in ["", "bad/name"] {
        route.metadata.namespace = None;
        route.metadata.name = Some(bad.into());
        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        assert!(ctx
            .runtime()
            .plugin_ref_mgr
            .get_dependencies(&HttpRouteHandler::create_resource_ref(&route).key())
            .is_empty());
    }
}

#[tokio::test]
async fn extension_ref_invalid_name_diagnoses_rule_and_backend_status_without_dependencies() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::Gateway;

    let handler = HttpRouteHandler::default();
    for name in ["", "bad/name"] {
        for backend_scope in [false, true] {
            let filter = serde_json::json!({"type":"ExtensionRef","extensionRef":{
                "group":"edgion.io","kind":"EdgionPlugins","name":name
            }});
            let rule = if backend_scope {
                serde_json::json!({"backendRefs":[{"name":"svc","port":80,"filters":[filter]}]})
            } else {
                serde_json::json!({"filters":[filter]})
            };
            let expected = if backend_scope {
                "backendRefs[0] ExtensionRef name must be a valid Kubernetes object name"
            } else {
                "ExtensionRef name must be a valid Kubernetes object name"
            };
            let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
                "metadata":{"name":"name-diagnostic","namespace":"default"},
                "spec":{"parentRefs":[{"name":"gw"}],"rules":[rule,{}]}
            }))
            .unwrap();
            let runtime = Arc::new(ControllerRuntime::default());
            let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
            let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
                "Gateway",
                16,
                Arc::new(GatewayHandler::new(None, None)),
                runtime,
            ));
            let raw = Arc::new(LocalRawStore::new());
            let gateway: Gateway = serde_json::from_value(serde_json::json!({
                "metadata":{"name":"gw","namespace":"default"},
                "spec":{"gatewayClassName":"edgion","listeners":[{"name":"http","protocol":"HTTP","port":80}]}
            }))
            .unwrap();
            raw.upsert("default/gw".into(), Arc::new(gateway));
            gateways.set_raw_store(raw);
            ctx.runtime().processor_registry.register(gateways);
            let owner = HttpRouteHandler::create_resource_ref(&route);
            assert_eq!(find_invalid_rule_details(&route, "default"), vec![(0, expected.into())]);
            handler.update_status(&mut route, &ctx, &[]);
            let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
            let partial = conditions.iter().find(|c| c.type_ == "PartiallyInvalid").unwrap();
            assert_eq!(partial.status, "True");
            assert!(conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True"));
            assert!(partial.message.starts_with("Dropped Rule"));
            assert!(partial.message.contains(expected));
            assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
            assert!(ctx.runtime().plugin_ref_mgr.get_dependencies(&owner.key()).is_empty());

            route.spec.rules.as_mut().unwrap().truncate(1);
            handler.update_status(&mut route, &ctx, &[]);
            let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
            let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
            assert_eq!(accepted.status, "False");
            assert_eq!(accepted.reason, "UnsupportedValue");
            assert!(accepted.message.contains(expected));
            assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));
            assert!(ctx.runtime().plugin_ref_mgr.get_dependencies(&owner.key()).is_empty());
        }
    }
}

#[test]
fn extension_ref_unsupported_group_kind_retains_exact_diagnostic_with_invalid_name() {
    for (group, kind) in [
        ("", "EdgionPlugins"),
        ("example.com", "EdgionPlugins"),
        ("edgion.io", "Secret"),
    ] {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"unsupported-target","namespace":"default"},
            "spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{
                "group":group,"kind":kind,"name":"bad/name"
            }}]}]}
        }))
        .unwrap();
        assert_eq!(
            find_invalid_rule_details(&route, "default"),
            vec![(
                0,
                format!(
                    "ExtensionRef kind '{kind}' (group '{group}') is not supported; only edgion.io/EdgionPlugins is"
                )
            )]
        );
    }
}

#[tokio::test]
async fn malformed_mirror_target_does_not_register_service_or_grant_recovery() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"invalid-mirror","namespace":"app"},"spec":{"rules":[{"filters":[{"type":"RequestMirror","requestMirror":{"backendRef":{"name":"bad/name","namespace":"target","port":80}}}]}]}})).unwrap();
    let owner = HttpRouteHandler::create_resource_ref(&route);
    let ProcessResult::Continue(parsed) = HttpRouteHandler::default().parse(route, &ctx).await else {
        panic!("parse continues for status")
    };
    assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());
    assert!(runtime.cross_ns_ref_mgr.get_dependencies(&owner.key()).is_empty());
    assert!(!find_invalid_rule_details(&parsed, "app").is_empty());
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
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"route","namespace":"default"},
        "spec":{"parentRefs":[{"name":"gateway"}],"rules":[
            {"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}],
             "backendRefs":[{"name":"service","port":80,"weight":7,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}]}]},
            {"backendRefs":[{"name":"fallback","port":80}]}
        ]}
    })).unwrap();
    let handler = HttpRouteHandler::default();
    handler.update_status(&mut route, &ctx, &[]);
    let before_rules = serde_json::to_value(route.effective_rules()).unwrap();
    let before_indices = route.spec.invalid_rule_indices.clone();
    let admission = |route: &HTTPRoute| {
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

#[tokio::test]
async fn graphql_router_uses_raw_desired_membership_and_preserves_sibling_rules() {
    use super::super::EdgionPluginsHandler;
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
    let source = |second: &str| -> Arc<EdgionPlugins> {
        Arc::new(
            serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"namespace":"apps","name":"routing"},
            "spec":{"requestPlugins":[{"type":"GraphqlOperationRouter","config":{"rules":[
                {"operationType":"query","operationName":"GetUser","backendRef":{"name":"users"}},
                {"operationType":"mutation","operationName":"CreateOrder","backendRef":{"name":second}}
            ]}}]}}))
            .unwrap(),
        )
    };
    raw.upsert("apps/routing".into(), source("orders"));
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let route:HTTPRoute=serde_json::from_value(serde_json::json!({"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute",
        "metadata":{"namespace":"apps","name":"graphql"},"spec":{"rules":[
            {"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"routing"}}],
             "backendRefs":[{"name":"users","port":80},{"name":"orders","port":81}]},
            {"backendRefs":[{"name":"healthy","port":80}]}
        ]}})).unwrap();
    assert!(find_controller_invalid_rule_details(&route, "apps", &runtime).is_empty());
    raw.upsert("apps/routing".into(), source("not-declared"));
    let errors = find_controller_invalid_rule_details(&route, "apps", &runtime);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 0);
    assert!(errors[0].1.contains("not declared"));
    raw.upsert("apps/routing".into(), source("orders"));
    assert!(find_controller_invalid_rule_details(&route, "apps", &runtime).is_empty());
    let mut delegated = route.clone();
    delegated.spec.resolved_rules = delegated.spec.rules.clone();
    assert!(find_controller_invalid_rule_details(&delegated, "apps", &runtime)[0]
        .1
        .contains("delegation"));
}

#[tokio::test]
async fn merged_graphql_bare_target_collision_and_survivor_keep_canonical_diagnostics() {
    use super::super::EdgionPluginsHandler;
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
    let source: EdgionPlugins = serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"routing"},"spec":{"requestPlugins":[{"type":"GraphqlOperationRouter","config":{"rules":[{"operationType":"query","operationName":"GetUser","backendRef":{"name":"undeclared"}}]}}]}})).unwrap();
    raw.upsert("routing".into(), Arc::new(source.clone()));
    let route: HTTPRoute = serde_json::from_value(serde_json::json!({"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"namespace":"default","name":"graphql"},"spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"routing"}}],"backendRefs":[{"name":"users","port":80}]}]}})).unwrap();
    assert!(find_controller_invalid_rule_details(&route, "default", &runtime)[0]
        .1
        .contains("not declared"));
    let mut explicit = source;
    explicit.metadata.namespace = Some("default".into());
    raw.upsert("default/routing".into(), Arc::new(explicit));
    assert!(find_controller_invalid_rule_details(&route, "default", &runtime).is_empty());
    let refs = edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs::from_route(&route).unwrap();
    assert!(matches!(
        super::super::route_utils::ambiguous_plugin_ref(refs.authored().next().unwrap(), &runtime),
        Some(crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError::AmbiguousEdgionPlugins { .. })
    ));
    raw.remove("routing");
    assert!(find_controller_invalid_rule_details(&route, "default", &runtime)[0]
        .1
        .contains("not declared"));
}

#[tokio::test]
async fn delegated_internal_auth_recovers_via_secret_and_grant_queues_and_clears_removed_refs() {
    use super::super::{ReferenceGrantHandler, SecretHandler};
    use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::CrossNsRevalidationListener;
    use crate::conf_mgr::sync_runtime::resource_processor::RawStore;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::ReferenceGrant;
    use k8s_openapi::api::core::v1::Secret;
    use serde_json::json;
    let runtime = Arc::new(ControllerRuntime::default());
    runtime
        .ref_grant_dispatcher
        .register_listener(Arc::new(CrossNsRevalidationListener::new(runtime.clone())));
    let routes = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        32,
        Arc::new(HttpRouteHandler::default()),
        runtime.clone(),
    ));
    let route_raw = Arc::new(LocalRawStore::new());
    routes.set_raw_store(route_raw.clone());
    runtime.processor_registry.register(routes.clone());
    let secrets = Arc::new(ResourceProcessor::<Secret>::new(
        "Secret",
        32,
        Arc::new(SecretHandler::new()),
        runtime.clone(),
    ));
    let secret_raw = Arc::new(LocalRawStore::new());
    secrets.set_raw_store(secret_raw.clone());
    runtime.processor_registry.register(secrets.clone());
    let grants = Arc::new(ResourceProcessor::<ReferenceGrant>::new(
        "ReferenceGrant",
        32,
        Arc::new(ReferenceGrantHandler::new()),
        runtime.clone(),
    ));
    let grant_raw = Arc::new(LocalRawStore::new());
    grants.set_raw_store(grant_raw.clone());
    runtime.processor_registry.register(grants.clone());
    macro_rules! step {
        ($processor:expr, $raw:expr, $key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let source = $raw.get(&item.key).map(|value| (*value).clone());
            $processor
                .process_work_item(&item.key, source, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    macro_rules! drain_routes {
        () => {{
            let mut seen = Vec::new();
            let queue = routes.workqueue();
            // Include delayed cross-resource wakeups before the next generation.
            while let Ok(Some(item)) = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue()).await {
                assert!(seen.len() < 32, "route requeue cycle");
                let source = route_raw.get(&item.key).map(|value| (*value).clone());
                routes
                    .process_work_item(&item.key, source, None, item.trigger_chain)
                    .await;
                queue.done(&item.key);
                seen.push(item.key);
            }
            seen
        }};
    }
    let delegation: ReferenceGrant = serde_json::from_value(json!({"metadata":{"name":"delegation","namespace":"child"},"spec":{
        "from":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":"root"}],"to":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute"}]
    }})).unwrap();
    runtime
        .ref_grant_store
        .upsert("child/delegation".into(), Arc::new(delegation));
    let grant: ReferenceGrant = serde_json::from_value(json!({"metadata":{"name":"material","namespace":"material"},"spec":{
        "from":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":"child"}],"to":[{"group":"","kind":"Secret","name":"token"}]
    }})).unwrap();
    grant_raw.upsert("material/material".into(), Arc::new(grant));
    grants.on_event_key("material/material".into()).await;
    step!(grants, grant_raw, "material/material");
    let child: HTTPRoute = serde_json::from_value(json!({"metadata":{"name":"leaf","namespace":"child"},"spec":{"rules":[{
        "matches":[{"path":{"type":"PathPrefix","value":"/"}}],"filters":[{"type":"ExternalAuth","resolvedExternalAuth":{"namespace":"forged"},"externalAuth":{
            "target":{"name":"auth","port":8080},"decision":{},"request":{"path":{"template":"/check/${secretRef:material/token:key}"}},"resolvedSecrets":{"material/token:key":"forged-value"}
        }}],"backendRefs":[{"name":"backend","port":80}]
    }]}})).unwrap();
    let root: HTTPRoute = serde_json::from_value(json!({"metadata":{"name":"root","namespace":"root"},"spec":{"rules":[{
        "matches":[{"path":{"type":"PathPrefix","value":"/"}}],"backendRefs":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":"child","name":"leaf"}]
    }]}})).unwrap();
    route_raw.upsert("child/leaf".into(), Arc::new(child.clone()));
    routes.on_event_key("child/leaf".into()).await;
    drain_routes!();
    route_raw.upsert("root/root".into(), Arc::new(root));
    routes.on_event_key("root/root".into()).await;
    drain_routes!();
    let root_owner = ResourceRef::new(ResourceKind::HTTPRoute, Some("root".into()), "root".into());
    assert!(runtime
        .secret_ref_manager
        .get_refs("material/token")
        .contains(&root_owner));
    let assert_material = |expected: Option<&str>| {
        let current = routes.get("root/root").unwrap();
        let filter = &current.spec.resolved_rules.as_ref().unwrap()[0]
            .filters
            .as_ref()
            .unwrap()[0];
        let config = filter.external_auth.as_ref().unwrap().edgion_config().unwrap();
        assert_eq!(
            config
                .resolved_secrets
                .as_ref()
                .and_then(|values| values.0.get("material/token:key"))
                .map(String::as_str),
            expected
        );
        assert_eq!(
            filter.resolved_external_auth.as_ref().unwrap().error().is_none(),
            expected.is_some()
        );
        let refs =
            edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs::from_route(&current).unwrap();
        assert_eq!(refs.effective_at(0, None, 0).unwrap().namespace(), "child");
    };
    assert_material(None);
    let secret: Secret = serde_json::from_value(
        json!({"metadata":{"name":"token","namespace":"material"},"stringData":{"key":"fresh-value"}}),
    )
    .unwrap();
    secret_raw.upsert("material/token".into(), Arc::new(secret));
    secrets.on_event_key("material/token".into()).await;
    step!(secrets, secret_raw, "material/token");
    assert!(drain_routes!().contains(&"root/root".to_string()));
    assert_material(Some("fresh-value"));
    grant_raw.remove("material/material");
    grants.on_event_key("material/material".into()).await;
    step!(grants, grant_raw, "material/material");
    assert!(drain_routes!().contains(&"root/root".to_string()));
    assert_material(None);
    let mut removed = child;
    removed.spec.rules.as_mut().unwrap()[0].filters = None;
    route_raw.upsert("child/leaf".into(), Arc::new(removed));
    routes.on_event_key("child/leaf".into()).await;
    drain_routes!();
    assert!(runtime
        .secret_ref_manager
        .get_dependencies(&root_owner.key())
        .is_empty());
    assert!(!runtime
        .cross_ns_ref_mgr
        .get_dependencies(&root_owner.key())
        .contains(&"material".to_string()));
    route_raw.remove("root/root");
    routes.on_event_key("root/root".into()).await;
    drain_routes!();
    assert!(runtime
        .secret_ref_manager
        .get_dependencies(&root_owner.key())
        .is_empty());
}

#[tokio::test]
async fn disabling_internal_auth_tls_clears_material_and_tls_edges_but_keeps_template() {
    use edgion_resources::resources::http_route::HTTPRouteExternalAuth;
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use serde_json::json;
    use std::collections::HashMap;
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let route: HTTPRoute = serde_json::from_value(json!({"metadata":{"name":"route","namespace":"apps"},"spec":{"rules":[{"filters":[{"type":"ExternalAuth","externalAuth":{
        "target":{"name":"auth","port":443},"decision":{},"request":{"path":{"template":"/${secretRef:apps/token:key}"}},
        "tls":{"enabled":true,"validation":{"caCertificateRefs":[{"kind":"ConfigMap","name":"ca"}]},"clientCertificateRef":{"name":"client"}}
    }}]}]}})).unwrap();
    let cm: ConfigMap =
        serde_json::from_value(json!({"metadata":{"name":"ca","namespace":"apps"},"data":{"ca.crt":"ca-material"}}))
            .unwrap();
    runtime
        .configmap_store
        .replace_all(HashMap::from([("apps/ca".into(), cm)]));
    let token: Secret =
        serde_json::from_value(json!({"metadata":{"name":"token","namespace":"apps"},"stringData":{"key":"fresh"}}))
            .unwrap();
    let client: Secret =
        serde_json::from_value(json!({"metadata":{"name":"client","namespace":"apps"},"type":"kubernetes.io/tls"}))
            .unwrap();
    runtime.secret_store.replace_all(HashMap::from([
        ("apps/token".into(), token),
        ("apps/client".into(), client),
    ]));
    let handler = HttpRouteHandler::default();
    let ProcessResult::Continue(mut enabled) = handler.parse(route, &ctx).await else {
        panic!("route must parse")
    };
    let filter = &mut enabled.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0];
    let HTTPRouteExternalAuth::Edgion(config) = filter.external_auth.as_mut().unwrap() else {
        panic!()
    };
    let tls = config.conn.tls.as_mut().unwrap();
    assert_eq!(tls.resolved_ca_certificates.as_ref().unwrap().len(), 1);
    assert!(tls.resolved_client_certificate.is_some());
    assert_eq!(runtime.secret_ref_manager.get_refs("ConfigMap/apps/ca").len(), 1);
    assert_eq!(runtime.secret_ref_manager.get_refs("apps/client").len(), 1);
    tls.enabled = false;
    let ProcessResult::Continue(disabled) = handler.parse(enabled, &ctx).await else {
        panic!("route must parse")
    };
    let filter = &disabled.spec.rules.as_ref().unwrap()[0].filters.as_ref().unwrap()[0];
    let config = filter.external_auth.as_ref().unwrap().edgion_config().unwrap();
    let tls = config.conn.tls.as_ref().unwrap();
    assert!(tls.resolved_ca_certificates.is_none());
    assert!(tls.resolved_client_certificate.is_none());
    assert!(runtime.secret_ref_manager.get_refs("ConfigMap/apps/ca").is_empty());
    assert!(runtime.secret_ref_manager.get_refs("apps/client").is_empty());
    assert_eq!(runtime.secret_ref_manager.get_refs("apps/token").len(), 1);
    assert_eq!(config.resolved_secrets.as_ref().unwrap().0["apps/token:key"], "fresh");
    assert!(filter.resolved_external_auth.as_ref().unwrap().error().is_none());
}
