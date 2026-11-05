use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::HttpRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::route_delegation::{DelegationTargetName, RouteKindName};
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

fn route(ns: &str, name: &str, spec: serde_json::Value) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"namespace": ns, "name": name},
        "spec": spec
    }))
    .expect("HTTPRoute fixture must deserialize")
}

fn delegating_rule(prefix: &str, target_ns: &str, target_name: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"path": {"type": "PathPrefix", "value": prefix}}],
        "backendRefs": [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
            "namespace": target_ns, "name": target_name
        }]
    })
}

fn root_parent(ns: &str, name: &str, rules: serde_json::Value) -> HTTPRoute {
    route(
        ns,
        name,
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": rules}),
    )
}

fn route_parent_ref(ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": ns, "name": name})
}

fn child(ns: &str, name: &str, parents: &[(&str, &str)]) -> HTTPRoute {
    let refs: Vec<_> = parents.iter().map(|(ns, name)| route_parent_ref(ns, name)).collect();
    route(
        ns,
        name,
        serde_json::json!({
            "parentRefs": refs,
            "rules": [{
                "matches": [{"path": {"type": "PathPrefix", "value": "/api/v1"}}],
                "backendRefs": [{"name": format!("{name}-svc"), "port": 80}]
            }]
        }),
    )
}

async fn parse(route: HTTPRoute, ctx: &HandlerContext) -> HTTPRoute {
    match HttpRouteHandler::default().parse(route, ctx).await {
        ProcessResult::Continue(route) => route,
        other => panic!("unexpected parse result: {other:?}"),
    }
}

fn key(parent: (&str, &str), target_ns: &str, target_name: &str) -> DelegationKey {
    DelegationKey {
        kind: RouteKindName::HTTPRoute,
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
async fn parse_publishes_authored_rules_without_copies() {
    let ctx = make_ctx();
    let authored = root_parent(
        "infra",
        "parent",
        serde_json::json!([
            delegating_rule("/api", "t", "*"),
            {"matches": [{"path": {"type": "PathPrefix", "value": "/plain"}}],
             "backendRefs": [{"name": "plain", "port": 80}]}
        ]),
    );
    let parsed = parse(authored.clone(), &ctx).await;

    assert!(
        parsed.spec.resolved_delegation_keys.is_empty(),
        "a parent carries no keys"
    );
    let published = parsed.spec.rules.as_ref().unwrap();
    let authored_rules = authored.spec.rules.as_ref().unwrap();
    assert_eq!(published.len(), authored_rules.len());
    for (published, authored) in published.iter().zip(authored_rules) {
        assert_eq!(
            serde_json::to_value(&published.matches).unwrap(),
            serde_json::to_value(&authored.matches).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&published.backend_refs).unwrap(),
            serde_json::to_value(&authored.backend_refs).unwrap()
        );
        assert_eq!(published.filters.is_some(), authored.filters.is_some());
    }

    for i in 0..50 {
        let parsed_child = parse(child("t", &format!("c{i}"), &[("infra", "parent")]), &ctx).await;
        assert_eq!(
            parsed_child.spec.resolved_delegation_keys,
            vec![key(("infra", "parent"), "t", "*")],
            "child c{i} carries exactly its wildcard key"
        );
        assert_eq!(parsed_child.spec.rules.as_ref().unwrap().len(), 1);
    }
    // The parent republished after its children still carries only its rules.
    let reparsed = parse(authored.clone(), &ctx).await;
    assert_eq!(reparsed.spec.rules.as_ref().unwrap().len(), 2);
}

#[tokio::test]
async fn parse_clears_authored_resolved_delegation_keys() {
    let ctx = make_ctx();
    let mut forged = child("t", "c", &[("infra", "parent")]);
    forged.spec.resolved_delegation_keys = vec![key(("infra", "parent"), "t", "c")];
    let parsed = parse(forged, &ctx).await;
    assert!(
        parsed.spec.resolved_delegation_keys.is_empty(),
        "an unconfirmed edge must not survive from authored input"
    );
}

#[tokio::test]
async fn child_keys_require_bidirectional_confirmation() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("/api", "t", "other")]),
        ),
        &ctx,
    )
    .await;
    let parsed = parse(child("t", "c", &[("infra", "parent")]), &ctx).await;
    assert!(parsed.spec.resolved_delegation_keys.is_empty());

    // The parent delegates, but the child does not declare it.
    parse(
        root_parent(
            "infra",
            "p2",
            serde_json::json!([delegating_rule("/api", "t", "lonely")]),
        ),
        &ctx,
    )
    .await;
    let lonely = route(
        "t",
        "lonely",
        serde_json::json!({"rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    assert!(parse(lonely, &ctx).await.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn child_with_three_parents_gets_keys_for_each() {
    let ctx = make_ctx();
    for name in ["p1", "p2", "p3"] {
        parse(
            root_parent("infra", name, serde_json::json!([delegating_rule("/api", "t", "c")])),
            &ctx,
        )
        .await;
    }
    let parsed = parse(
        child("t", "c", &[("infra", "p3"), ("infra", "p1"), ("infra", "p2")]),
        &ctx,
    )
    .await;
    assert_eq!(
        parsed.spec.resolved_delegation_keys,
        vec![
            key(("infra", "p1"), "t", "c"),
            key(("infra", "p2"), "t", "c"),
            key(("infra", "p3"), "t", "c"),
        ],
        "keys are sorted for a deterministic payload"
    );
}

#[tokio::test]
async fn child_with_named_and_wildcard_from_same_parent_gets_two_keys() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([
                delegating_rule("/a", "t", "c"),
                delegating_rule("/b", "t", "*"),
                delegating_rule("/c", "t", "c")
            ]),
        ),
        &ctx,
    )
    .await;
    let parsed = parse(child("t", "c", &[("infra", "parent")]), &ctx).await;
    let keys = &parsed.spec.resolved_delegation_keys;
    assert_eq!(keys.len(), 2, "rules sharing a target share one key: {keys:?}");
    assert!(keys.contains(&key(("infra", "parent"), "t", "c")));
    assert!(keys.contains(&key(("infra", "parent"), "t", "*")));
}

#[tokio::test]
async fn mixed_parent_refs_route_gets_no_keys() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    let mixed = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [{"name": "gw", "namespace": "infra"}, route_parent_ref("infra", "parent")],
            "rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]
        }),
    );
    let parsed = parse(mixed, &ctx).await;
    assert!(parsed.spec.resolved_delegation_keys.is_empty());
    assert!(matches!(
        http_delegation_facts(&parsed, ctx.runtime()).shape,
        ParentRefShape::Invalid(_)
    ));
}

#[tokio::test]
async fn child_with_hostnames_gets_no_keys() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    let mut with_hostnames = child("t", "c", &[("infra", "parent")]);
    with_hostnames.spec.hostnames = Some(vec!["child.example.com".to_string()]);
    let parsed = parse(with_hostnames, &ctx).await;
    assert!(parsed.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn delegating_rule_with_canary_plugin_is_invalid_rule() {
    assert_backend_plugin_on_delegating_rule_is_invalid(
        "Canary",
        serde_json::json!({"enable": true, "activeProfile": "base", "profiles": {"base": {"name": "backend"}}}),
    )
    .await;
}

#[tokio::test]
async fn delegating_rule_with_ai_proxy_plugin_is_invalid_rule() {
    assert_backend_plugin_on_delegating_rule_is_invalid("AiProxy", serde_json::json!({"api": "openaiChat"})).await;
}

/// A delegating rule whose ExtensionRef carries `plugin_type` (a
/// backend-selecting plugin) is an invalid rule and publishes no edge.
async fn assert_backend_plugin_on_delegating_rule_is_invalid(plugin_type: &str, config: serde_json::Value) {
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

    let mut rule = delegating_rule("/api", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "ExtensionRef", "extensionRef": {
        "group": "edgion.io", "kind": "EdgionPlugins", "name": "canary"
    }}]);
    let parent = root_parent(
        "infra",
        "parent",
        serde_json::json!([rule, delegating_rule("/ok", "t", "c2")]),
    );
    let mut parsed = parse(parent, &ctx).await;

    let facts = http_delegation_facts(&parsed, ctx.runtime());
    assert_eq!(facts.backend_plugin_on_delegating_rule.len(), 1);
    assert_eq!(facts.backend_plugin_on_delegating_rule[0].0, 0);
    assert!(facts.backend_plugin_on_delegating_rule[0].1.contains(plugin_type));
    assert_eq!(
        facts.targets,
        vec![DelegationTarget {
            ns: "t".into(),
            name: DelegationTargetName::Named("c2".into())
        }],
        "an invalid delegating rule publishes no edge"
    );

    HttpRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
    assert_eq!(parsed.spec.invalid_rule_indices, Some(vec![0]));

    // The edge of the invalid rule is not confirmed for its child.
    let parsed_child = parse(child("t", "c", &[("infra", "parent")]), &ctx).await;
    assert!(parsed_child.spec.resolved_delegation_keys.is_empty());
}

#[tokio::test]
async fn malformed_delegating_rule_is_invalid_rule() {
    let ctx = make_ctx();
    let mut exact = delegating_rule("/api", "t", "c");
    exact["matches"][0]["path"]["type"] = serde_json::json!("Exact");
    let mut parsed = parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([exact, {"backendRefs": [{"name": "svc", "port": 80}]}]),
        ),
        &ctx,
    )
    .await;
    let facts = http_delegation_facts(&parsed, ctx.runtime());
    assert_eq!(facts.malformed_rules.len(), 1);
    assert!(facts.targets.is_empty());
    HttpRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
    assert_eq!(parsed.spec.invalid_rule_indices, Some(vec![0]));
}

#[tokio::test]
async fn ordinary_route_payload_unchanged() {
    let ctx = make_ctx();
    // The input fixture carries authored values for fields a route must never
    // set, each a drop-on-parse probe: `resolvedDelegationKeys` (cleared by
    // parse), forged resolved ExternalAuth/terminal fields, and an
    // ExtensionRef `resolvedNamespace` (no longer a field at all: serde
    // ignores it as an unknown key). The byte-identical parsed fixture proves
    // none of them reaches the conf_sync payload.
    let input: HTTPRoute =
        serde_json::from_str(include_str!("../../../../../fixtures/http-route-ordinary-input.json")).unwrap();
    let handler = HttpRouteHandler::default();
    let mut parsed = parse(input, &ctx).await;
    handler.finalize_transport(&mut parsed, &ctx);
    let payload = serde_json::to_string_pretty(&parsed).unwrap() + "\n";
    assert!(
        !payload.contains("resolvedNamespace") && !payload.contains("injected"),
        "the unknown ExtensionRef resolvedNamespace probe must be dropped on parse"
    );
    assert_eq!(
        payload,
        include_str!("../../../../../fixtures/http-route-ordinary-parsed.json"),
        "the ordinary-route conf_sync payload must stay byte-identical"
    );
    assert!(
        !ctx.runtime()
            .http_route_delegation_graph
            .is_participant("apps/ordinary"),
        "an ordinary route never enters the delegation graph"
    );
}

#[tokio::test]
async fn diamond_and_fanout_work_is_linear() {
    const PARENTS: usize = 3;
    const CHILDREN: usize = 200;
    let ctx = make_ctx();
    let parents: Vec<String> = (0..PARENTS).map(|i| format!("p{i}")).collect();
    for name in &parents {
        parse(
            root_parent("infra", name, serde_json::json!([delegating_rule("/api", "t", "*")])),
            &ctx,
        )
        .await;
    }
    let declared: Vec<(&str, &str)> = parents.iter().map(|p| ("infra", p.as_str())).collect();
    let mut total_keys = 0;
    for i in 0..CHILDREN {
        let parsed = parse(child("t", &format!("c{i}"), &declared), &ctx).await;
        total_keys += parsed.spec.resolved_delegation_keys.len();
    }
    // Every child confirms every parent: Σ confirmed edges = PARENTS × CHILDREN.
    assert_eq!(total_keys, PARENTS * CHILDREN);
    // A child-only event (unchanged reprocess) issues no parent requeues.
    let mut requeued = 0;
    for i in 0..CHILDREN {
        let c = child("t", &format!("c{i}"), &declared);
        let facts = http_delegation_facts(&c, ctx.runtime());
        requeued += sync_http_delegation_graph(&ctx.runtime().http_route_delegation_graph, &c, &facts, false, || false)
            .expect("a child is a participant")
            .len();
    }
    assert_eq!(requeued, 0, "unchanged children must not requeue their parents");
    // Every parent and child is a participant; the per-participant (not
    // per-pair) storage bound is pinned in `delegation_graph_test`.
    let graph = &ctx.runtime().http_route_delegation_graph;
    assert!(parents.iter().all(|p| graph.is_participant(&format!("infra/{p}"))));
    assert!((0..CHILDREN).all(|i| graph.is_participant(&format!("t/c{i}"))));
    for name in &parents {
        assert_eq!(graph.children_of(&format!("infra/{name}")).len(), CHILDREN);
    }
}

#[tokio::test]
async fn deleting_a_parent_clears_graph_participation() {
    let ctx = make_ctx();
    let parent = parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    parse(child("t", "c", &[("infra", "parent")]), &ctx).await;
    let graph = &ctx.runtime().http_route_delegation_graph;
    assert_eq!(graph.level("t/c"), Some(2));

    HttpRouteHandler::default().on_delete(&parent, &ctx).await;
    assert!(!graph.is_participant("infra/parent"));
    assert!(graph.confirmed_keys("t/c").is_empty());

    // A route that stops delegating leaves the graph on its next parse.
    let ordinary = parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([{"backendRefs": [{"name": "svc", "port": 80}]}]),
        ),
        &ctx,
    )
    .await;
    assert!(ordinary.spec.resolved_delegation_keys.is_empty());
    assert!(!graph.is_participant("infra/parent"));
}

#[test]
fn sync_requeues_target_children_parents_and_level_dependents() {
    let graph = DelegationGraph::new(RouteKindName::HTTPRoute);
    let runtime = ControllerRuntime::default();
    let parent = root_parent(
        "infra",
        "parent",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    );
    let c = child("t", "c", &[("infra", "parent")]);

    // Child first: the parent is not in the graph yet (it names nobody), so
    // nothing is requeued; it reads the declaration when it is processed.
    let child_facts = http_delegation_facts(&c, &runtime);
    let requeue = sync_http_delegation_graph(&graph, &c, &child_facts, false, || false).unwrap();
    assert!(requeue.is_empty());
    assert!(graph.confirmed_keys("t/c").is_empty());

    // Parent second: its added target requeues the declaring child.
    let parent_facts = http_delegation_facts(&parent, &runtime);
    let requeue = sync_http_delegation_graph(&graph, &parent, &parent_facts, true, || false).unwrap();
    assert_eq!(requeue, vec!["t/c".to_string()]);
    assert_eq!(
        resolve_child_keys(&graph, "t/c"),
        vec![key(("infra", "parent"), "t", "c")]
    );

    // Unchanged parent reprocess: no target delta and no level change.
    let requeue = sync_http_delegation_graph(&graph, &parent, &parent_facts, true, || false).unwrap();
    assert!(requeue.is_empty());

    // Child reprocess picks up its level and has no children to requeue.
    sync_http_delegation_graph(&graph, &c, &child_facts, false, || false).unwrap();
    assert_eq!(graph.level("t/c"), Some(2));

    // Root detach changes the parent's level: its confirmed children requeue.
    let requeue = sync_http_delegation_graph(&graph, &parent, &parent_facts, false, || false).unwrap();
    assert_eq!(requeue, vec!["t/c".to_string()]);

    // An ordinary route is a single probe and requeues nothing.
    let ordinary = route(
        "apps",
        "plain",
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}),
    );
    let facts = http_delegation_facts(&ordinary, &runtime);
    assert!(sync_http_delegation_graph(&graph, &ordinary, &facts, true, || false).is_none());
}

#[test]
fn two_cycle_settles_without_requeue_ping_pong() {
    let graph = DelegationGraph::new(RouteKindName::HTTPRoute);
    let runtime = ControllerRuntime::default();
    let cyclic = |name: &str, other: &str| {
        route(
            "t",
            name,
            serde_json::json!({
                "parentRefs": [route_parent_ref("t", other)],
                "rules": [delegating_rule("/api", "t", other)]
            }),
        )
    };
    let a = cyclic("a", "b");
    let b = cyclic("b", "a");
    let facts_a = http_delegation_facts(&a, &runtime);
    let facts_b = http_delegation_facts(&b, &runtime);
    // First round registers both sides.
    sync_http_delegation_graph(&graph, &a, &facts_a, false, || false).unwrap();
    sync_http_delegation_graph(&graph, &b, &facts_b, false, || false).unwrap();
    // After one processing round, reprocessing either side requeues nothing.
    assert!(sync_http_delegation_graph(&graph, &a, &facts_a, false, || false)
        .unwrap()
        .is_empty());
    assert!(sync_http_delegation_graph(&graph, &b, &facts_b, false, || false)
        .unwrap()
        .is_empty());
    assert_eq!(graph.level("t/a"), None);
    assert_eq!(graph.level("t/b"), None);
}

#[test]
fn child_declared_parent_change_requeues_only_naming_parents() {
    let graph = DelegationGraph::new(RouteKindName::HTTPRoute);
    let runtime = ControllerRuntime::default();
    for (name, target) in [("p1", "c"), ("p2", "*"), ("p3", "other")] {
        let parent = root_parent("infra", name, serde_json::json!([delegating_rule("/api", "t", target)]));
        let facts = http_delegation_facts(&parent, &runtime);
        sync_http_delegation_graph(&graph, &parent, &facts, true, || false).unwrap();
    }
    let first = child("t", "c", &[("infra", "p1")]);
    let facts = http_delegation_facts(&first, &runtime);
    assert_eq!(
        sync_http_delegation_graph(&graph, &first, &facts, false, || false).unwrap(),
        vec!["infra/p1".to_string()],
        "a parent naming the child refreshes its RefNotPermitted"
    );
    // Moving to a wildcard parent and a parent that targets another name:
    // only the old naming parent depends on the change.
    let moved = child("t", "c", &[("infra", "p2"), ("infra", "p3")]);
    let facts = http_delegation_facts(&moved, &runtime);
    assert_eq!(
        sync_http_delegation_graph(&graph, &moved, &facts, false, || false).unwrap(),
        vec!["infra/p1".to_string()]
    );
}

/// Round 3 M-1: a parent with no level (not attached) that becomes blocked
/// or unblocked requeues its children, whose message differs ("not
/// accepted" vs "not reachable") although the level stays `None`.
#[test]
fn blocked_flip_on_unlevelled_parent_requeues_children() {
    let graph = DelegationGraph::new(RouteKindName::HTTPRoute);
    let runtime = ControllerRuntime::default();
    let parent = route(
        "infra",
        "parent",
        serde_json::json!({"rules": [delegating_rule("/api", "t", "c")]}),
    );
    let c = child("t", "c", &[("infra", "parent")]);
    let child_facts = http_delegation_facts(&c, &runtime);
    sync_http_delegation_graph(&graph, &c, &child_facts, false, || false).unwrap();
    let parent_facts = http_delegation_facts(&parent, &runtime);
    sync_http_delegation_graph(&graph, &parent, &parent_facts, false, || false).unwrap();
    assert_eq!(graph.level("infra/parent"), None);

    let sync = |blocked: bool| sync_http_delegation_graph(&graph, &parent, &parent_facts, false, || blocked).unwrap();
    assert_eq!(sync(true), vec!["t/c".to_string()], "blocking requeues the child");
    assert_eq!(graph.level("infra/parent"), None);
    assert!(sync(true).is_empty(), "an unchanged blocked flag requeues nothing");
    assert_eq!(sync(false), vec!["t/c".to_string()], "unblocking requeues the child");
    assert!(sync(false).is_empty());
}

/// Task 1 ledger: a route-kind parentRef outside the Gateway API group is
/// not a delegation parentRef. The route is classified as a root (its
/// parentRefs are all non-route refs), declares no parent, takes no part
/// in delegation and gets no keys, even under a parent naming it.
#[tokio::test]
async fn route_kind_parent_ref_with_foreign_group_is_not_a_delegation_parent() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "infra",
            "parent",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
        ),
        &ctx,
    )
    .await;
    let foreign = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [{"group": "example.com", "kind": "HTTPRoute", "namespace": "infra", "name": "parent"}],
            "rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]
        }),
    );
    let facts = http_delegation_facts(&foreign, ctx.runtime());
    assert_eq!(facts.shape, ParentRefShape::Root);
    assert!(facts.declared_parents.is_empty());
    assert!(!facts.inert);
    let parsed = parse(foreign, &ctx).await;
    assert!(parsed.spec.resolved_delegation_keys.is_empty());
    let graph = &ctx.runtime().http_route_delegation_graph;
    assert!(!graph.is_participant("t/c"));
    assert!(graph.declared_parents("t/c").is_empty());
}

/// Security I4: a route in a foreign namespace declaring the platform root
/// must not requeue it on create, update or delete.
#[tokio::test]
async fn foreign_child_churn_never_requeues_platform_root() {
    let ctx = make_ctx();
    parse(
        root_parent(
            "platform",
            "root",
            serde_json::json!([
                delegating_rule("/api", "team", "*"),
                delegating_rule("/b", "team", "billing")
            ]),
        ),
        &ctx,
    )
    .await;
    let graph = &ctx.runtime().http_route_delegation_graph;
    for round in 0..3 {
        let evil = child("evil", &format!("c{round}"), &[("platform", "root")]);
        let facts = http_delegation_facts(&evil, ctx.runtime());
        assert!(sync_http_delegation_graph(graph, &evil, &facts, false, || false)
            .unwrap()
            .is_empty());
        assert!(graph.on_route_deleted(&format!("evil/c{round}")).is_empty());
    }
}

#[test]
fn backend_selecting_plugins_use_the_shared_upstream_jump_classifier() {
    for name in [
        "Canary",
        "DynamicInternalUpstream",
        "DynamicExternalUpstream",
        "DirectEndpoint",
        "RegionRoute",
        "GraphqlOperationRouter",
        "AllEndpointStatus",
        "AiProxy",
    ] {
        assert!(is_backend_selecting_plugin(name), "{name}");
    }
    for name in ["RequestId", "BasicAuth", "RateLimitLocal", "AiGuard", "Guardrail"] {
        assert!(!is_backend_selecting_plugin(name), "{name}");
    }
}

// =====================================================================
// Static scope check helpers
// =====================================================================

#[test]
fn prefix_covers_uses_segment_boundaries() {
    assert!(prefix_covers("/api/v1/billing", "/api/v1/billing"));
    assert!(prefix_covers("/api/v1/billing", "/api/v1/billing/x"));
    assert!(!prefix_covers("/api/v1/billing", "/api/v1/billingx"));
    assert!(!prefix_covers("/api/v1/billing", "/api/v1"));
    assert!(prefix_covers("/", "/anything"));
}

#[test]
fn canonical_http_path_matches_gateway_canonicalization() {
    assert_eq!(canonical_http_path("/api//v1/", true).as_deref(), Some("/api/v1"));
    assert_eq!(canonical_http_path("/api/v1/", false).as_deref(), Some("/api/v1/"));
    assert_eq!(canonical_http_path("/", true).as_deref(), Some("/"));
    for unchecked in ["/users/:id", "/a%2Fb", "/a;x", "relative"] {
        assert_eq!(canonical_http_path(unchecked, true), None, "{unchecked}");
    }
}

fn scoped(mask: u32, scopes: &[&str]) -> DelegatingRule {
    DelegatingRule {
        mask,
        scopes: Some(scopes.iter().map(|s| s.to_string()).collect()),
    }
}

fn child_rule(mask: u32, matches: Vec<ChildMatch>) -> ChildRuleFacts {
    ChildRuleFacts { mask, matches }
}

#[test]
fn classify_child_rules_reports_scope_and_per_rule_conflicts() {
    let edge = [scoped(0b1, &["/a"]), scoped(0, &["/b"])];
    let rules = [
        // Reachable through /a only, conflicts with it.
        child_rule(0b1, vec![ChildMatch::HttpPrefix("/a/x".into())]),
        // Reachable through /b (mask 0): serves.
        child_rule(0b1, vec![ChildMatch::HttpPrefix("/b/x".into())]),
        // Reachable through both: serves through /b.
        child_rule(0b1, vec![]),
        // Out of scope, exact and prefix.
        child_rule(0, vec![ChildMatch::HttpExact("/c".into())]),
        child_rule(0, vec![ChildMatch::HttpPrefix("/ab".into())]),
        // A broader child prefix intersects the delegated prefix.
        child_rule(0, vec![ChildMatch::HttpPrefix("/".into())]),
        // Uncheckable match: never reported.
        child_rule(0, vec![ChildMatch::HttpExact("/c".into()), ChildMatch::Unchecked]),
    ];
    assert_eq!(classify_child_rules(&edge, &rules), (vec![0], vec![3, 4]));

    // An unscoped delegating rule reaches everything.
    let edge = [DelegatingRule::unscoped(0b1)];
    assert_eq!(classify_child_rules(&edge, &rules), (vec![0, 1, 2], vec![]));

    // gRPC: service equality.
    let edge = [scoped(0, &["pkg.Api"])];
    let rules = [
        child_rule(0, vec![ChildMatch::GrpcService("pkg.Api".into())]),
        child_rule(0, vec![ChildMatch::GrpcService("pkg.Other".into())]),
    ];
    assert_eq!(classify_child_rules(&edge, &rules), (vec![], vec![1]));
}

#[test]
fn edge_message_lists_scope_then_conflict_segments() {
    assert_eq!(edge_message(RouteKindName::HTTPRoute, "a/p", &[], &[]), None);
    assert_eq!(
        edge_message(RouteKindName::HTTPRoute, "a/p", &[2], &[0, 1]).as_deref(),
        Some(
            "Rule(s) outside parent HTTPRoute 'a/p' scope: rules[0], rules[1]; \
             Conflicting Rule(s) with parent HTTPRoute 'a/p': rules[2]: a standard filter conflicts with a mandatory filter of the same type"
        )
    );
}
