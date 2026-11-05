use super::super::route_delegation::sync_delegation_graph;
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::GrpcRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::route_delegation::{
    DelegationKey, DelegationTarget, DelegationTargetName, ParentRefShape, RouteKindName,
};
use std::sync::Arc;

// =====================================================================
// Fixtures
// =====================================================================

fn ctx_with(runtime: Arc<ControllerRuntime>) -> HandlerContext {
    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

fn make_ctx() -> HandlerContext {
    ctx_with(Arc::new(ControllerRuntime::default()))
}

fn route(ns: &str, name: &str, spec: serde_json::Value) -> GRPCRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"namespace": ns, "name": name},
        "spec": spec
    }))
    .expect("GRPCRoute fixture must deserialize")
}

fn delegating_rule(service: &str, target_ns: &str, target_name: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"method": {"service": service}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
            "namespace": target_ns, "name": target_name
        }]
    })
}

fn root_parent(ns: &str, name: &str, rules: serde_json::Value) -> GRPCRoute {
    route(
        ns,
        name,
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": rules}),
    )
}

fn route_parent_ref(ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "namespace": ns, "name": name})
}

fn child(ns: &str, name: &str, parents: &[(&str, &str)]) -> GRPCRoute {
    let refs: Vec<_> = parents.iter().map(|(ns, name)| route_parent_ref(ns, name)).collect();
    route(
        ns,
        name,
        serde_json::json!({
            "parentRefs": refs,
            "rules": [{
                "matches": [{"method": {"method": "Get"}}],
                "backendRefs": [{"name": format!("{name}-svc"), "port": 50051}]
            }]
        }),
    )
}

async fn parse(route: GRPCRoute, ctx: &HandlerContext) -> GRPCRoute {
    match GrpcRouteHandler::default().parse(route, ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    }
}

fn key(parent: (&str, &str), target_ns: &str, target_name: &str) -> DelegationKey {
    DelegationKey {
        kind: RouteKindName::GRPCRoute,
        parent_ns: parent.0.to_string(),
        parent_name: parent.1.to_string(),
        target_ns: target_ns.to_string(),
        target_name: if target_name == "*" {
            DelegationTargetName::Wildcard
        } else {
            DelegationTargetName::Named(target_name.to_string())
        },
    }
}

// =====================================================================
// Publication without flattening
// =====================================================================

#[tokio::test]
async fn parse_publishes_authored_rules_without_copies_grpc() {
    let ctx = make_ctx();
    let authored = root_parent(
        "infra",
        "parent",
        serde_json::json!([
            delegating_rule("pkg.Billing", "t", "*"),
            {"matches": [{"method": {"service": "pkg.Plain"}}],
             "backendRefs": [{"name": "plain", "port": 50051}]}
        ]),
    );
    let parsed = parse(authored.clone(), &ctx).await;
    assert!(
        parsed.spec.resolved_delegation_keys.is_empty(),
        "a parent carries no keys"
    );
    assert_eq!(
        serde_json::to_value(&parsed.spec.rules).unwrap(),
        serde_json::to_value(&authored.spec.rules).unwrap(),
        "the parent publishes its authored rules unchanged"
    );

    for i in 0..20 {
        let parsed_child = parse(child("t", &format!("c{i}"), &[("infra", "parent")]), &ctx).await;
        assert_eq!(
            parsed_child.spec.resolved_delegation_keys,
            vec![key(("infra", "parent"), "t", "*")]
        );
    }
}

#[tokio::test]
async fn matchless_child_rule_is_published_unchanged_grpc() {
    // §9: the old security-critical rewrite of matchless children is gone; a
    // matchless child rule is a catch-all inside the delegated subtree.
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("pkg.Billing", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    let authored = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [route_parent_ref("infra", "parent")],
            "rules": [{"backendRefs": [{"name": "svc", "port": 50051}]}]
        }),
    );
    let parsed = parse(authored.clone(), &ctx).await;
    assert!(parsed.spec.rules.as_ref().unwrap()[0].matches.is_none());
    assert_eq!(
        parsed.spec.resolved_delegation_keys,
        vec![key(("infra", "parent"), "t", "c")]
    );
}

#[tokio::test]
async fn parse_clears_authored_resolved_delegation_keys_grpc() {
    let ctx = make_ctx();
    let mut forged = child("t", "c", &[("infra", "parent")]);
    forged.spec.resolved_delegation_keys = vec![key(("infra", "parent"), "t", "c")];
    assert!(parse(forged, &ctx).await.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn child_keys_require_bidirectional_confirmation_grpc() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("pkg.A", "t", "other")]),
        ),
        &ctx,
    )
    .await;
    assert!(parse(child("t", "c", &[("infra", "parent")]), &ctx)
        .await
        .spec
        .resolved_delegation_keys
        .is_empty());

    parse(
        root_parent(
            "infra",
            "p2",
            serde_json::json!([delegating_rule("pkg.A", "t", "lonely")]),
        ),
        &ctx,
    )
    .await;
    let lonely = route(
        "t",
        "lonely",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 50051}]}]}),
    );
    assert!(parse(lonely, &ctx).await.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn mixed_parent_refs_and_hostnames_get_no_keys_grpc() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("pkg.A", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    let mixed = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [{"name": "gw", "namespace": "infra"}, route_parent_ref("infra", "parent")],
            "rules": [{"backendRefs": [{"name": "svc", "port": 50051}]}]
        }),
    );
    let parsed = parse(mixed, &ctx).await;
    assert!(parsed.spec.resolved_delegation_keys.is_empty());
    assert!(matches!(
        grpc_delegation_facts(&parsed, ctx.runtime()).shape,
        ParentRefShape::Invalid(_)
    ));

    let mut with_hostnames = child("t", "c", &[("infra", "parent")]);
    with_hostnames.spec.hostnames = Some(vec!["child.example.com".to_string()]);
    assert!(parse(with_hostnames, &ctx)
        .await
        .spec
        .resolved_delegation_keys
        .is_empty());
}

#[tokio::test]
async fn malformed_delegating_rules_are_invalid_rules_grpc() {
    let ctx = make_ctx();
    let mut matchless = delegating_rule("pkg.A", "t", "c");
    matchless.as_object_mut().unwrap().remove("matches");
    let mut method_level = delegating_rule("pkg.A", "t", "c");
    method_level["matches"][0]["method"]["method"] = "Get".into();
    let mut parsed = parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([matchless, method_level, delegating_rule("pkg.B", "t", "c2"),
                               {"backendRefs": [{"name": "svc", "port": 50051}]}]),
        ),
        &ctx,
    )
    .await;
    let facts = grpc_delegation_facts(&parsed, ctx.runtime());
    let malformed: Vec<usize> = facts.malformed_rules.iter().map(|(i, _)| *i).collect();
    assert_eq!(malformed, vec![0, 1]);
    assert_eq!(
        facts.targets,
        vec![DelegationTarget {
            ns: "t".into(),
            name: DelegationTargetName::Named("c2".into())
        }]
    );
    GrpcRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
    assert_eq!(parsed.spec.invalid_rule_indices, Some(vec![0, 1]));
    let partially = parsed.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .find(|c| c.type_ == "PartiallyInvalid")
        .expect("PartiallyInvalid is set");
    assert!(partially.message.contains("rules[0]") && partially.message.contains("rules[1]"));
    assert!(partially.message.contains("delegate-all"), "{}", partially.message);
}

#[tokio::test]
async fn delegating_rule_with_canary_plugin_is_invalid_rule_grpc() {
    assert_backend_plugin_on_delegating_rule_is_invalid_grpc(
        "Canary",
        serde_json::json!({"enable": true, "activeProfile": "base", "profiles": {"base": {"name": "backend"}}}),
    )
    .await;
}

#[tokio::test]
async fn delegating_rule_with_ai_proxy_plugin_is_invalid_rule_grpc() {
    assert_backend_plugin_on_delegating_rule_is_invalid_grpc("AiProxy", serde_json::json!({"api": "openaiChat"})).await;
}

/// A delegating rule whose ExtensionRef carries `plugin_type` (a
/// backend-selecting plugin) is an invalid rule and publishes no edge.
async fn assert_backend_plugin_on_delegating_rule_is_invalid_grpc(plugin_type: &str, config: serde_json::Value) {
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
    let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionPlugins",
        "metadata": {"namespace": "infra", "name": "canary"},
        "spec": {"requestPlugins": [{"type": plugin_type, "config": config}]}
    }))
    .expect("plugin fixture must deserialize");
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert("infra/canary".to_string(), Arc::new(plugins));
    processor.set_raw_store(raw);
    runtime.processor_registry.register(processor);
    let ctx = ctx_with(runtime);

    let mut rule = delegating_rule("pkg.A", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "ExtensionRef", "extensionRef": {
        "group": "edgion.io", "kind": "EdgionPlugins", "name": "canary"
    }}]);
    let parent = root_parent(
        "infra",
        "parent",
        serde_json::json!([rule, delegating_rule("pkg.B", "t", "c2")]),
    );
    let mut parsed = parse(parent, &ctx).await;
    let facts = grpc_delegation_facts(&parsed, ctx.runtime());
    assert_eq!(facts.backend_plugin_on_delegating_rule.len(), 1);
    assert!(facts.backend_plugin_on_delegating_rule[0].1.contains(plugin_type));
    assert_eq!(facts.targets.len(), 1, "an invalid delegating rule publishes no edge");
    GrpcRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
    assert_eq!(parsed.spec.invalid_rule_indices, Some(vec![0]));
    let parsed_child = parse(child("t", "c", &[("infra", "parent")]), &ctx).await;
    assert!(parsed_child.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn ordinary_grpc_route_payload_unchanged() {
    let ctx = make_ctx();
    // The input fixture carries authored values for fields a route must never
    // set, each a drop-on-parse probe: `resolvedDelegationKeys` (cleared by
    // parse) and an ExtensionRef `resolvedNamespace` (no longer a field at
    // all: serde ignores it as an unknown key). The byte-identical parsed fixture proves
    // none of them reaches the conf_sync payload.
    let input: GRPCRoute =
        serde_json::from_str(include_str!("../../../../../fixtures/grpc-route-ordinary-input.json")).unwrap();
    let handler = GrpcRouteHandler::default();
    let mut parsed = parse(input, &ctx).await;
    handler.finalize_transport(&mut parsed, &ctx);
    let payload = serde_json::to_string_pretty(&parsed).unwrap() + "\n";
    assert!(
        !payload.contains("resolvedNamespace") && !payload.contains("injected"),
        "the unknown ExtensionRef resolvedNamespace probe must be dropped on parse"
    );
    assert_eq!(
        payload,
        include_str!("../../../../../fixtures/grpc-route-ordinary-parsed.json"),
        "the ordinary-route conf_sync payload must stay byte-identical"
    );
    assert!(!ctx
        .runtime()
        .grpc_route_delegation_graph
        .is_participant("apps/ordinary"));
}

#[test]
fn sync_requeues_target_children_parents_and_level_dependents_grpc() {
    let runtime = ControllerRuntime::default();
    let graph = &runtime.grpc_route_delegation_graph;
    let parent = root_parent(
        "infra",
        "parent",
        serde_json::json!([delegating_rule("pkg.A", "t", "c")]),
    );
    let c = child("t", "c", &[("infra", "parent")]);

    // The parent is not in the graph yet: nothing names the child.
    let child_facts = grpc_delegation_facts(&c, &runtime);
    assert!(sync_delegation_graph(graph, "t", "c", &child_facts, false, || false)
        .unwrap()
        .is_empty());
    let parent_facts = grpc_delegation_facts(&parent, &runtime);
    assert_eq!(
        sync_delegation_graph(graph, "infra", "parent", &parent_facts, true, || false).unwrap(),
        vec!["t/c".to_string()]
    );
    assert!(
        sync_delegation_graph(graph, "infra", "parent", &parent_facts, true, || false)
            .unwrap()
            .is_empty()
    );
    sync_delegation_graph(graph, "t", "c", &child_facts, false, || false).unwrap();
    assert_eq!(graph.level("t/c"), Some(2));
    assert_eq!(
        sync_delegation_graph(graph, "infra", "parent", &parent_facts, false, || false).unwrap(),
        vec!["t/c".to_string()],
        "root detach requeues the confirmed child"
    );

    let ordinary = route(
        "apps",
        "plain",
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": [{"backendRefs": [{"name": "svc", "port": 50051}]}]}),
    );
    let facts = grpc_delegation_facts(&ordinary, &runtime);
    assert!(sync_delegation_graph(graph, "apps", "plain", &facts, true, || false).is_none());
}
