//! GRPCRoute delegation status (design §7): parent-side `PartiallyInvalid` /
//! `ResolvedRefs`, one child entry per route-kind parentRef, and the requeue
//! paths that keep both sides fresh.

use super::GrpcRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler, ResourceProcessor,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::GRPCRoute;
use edgion_resources::resources::common::Condition;
use std::collections::BTreeSet;
use std::sync::Arc;

// =====================================================================
// Harness: a registered GRPCRoute processor provides the cache (route
// existence) and the workqueue that receives the handler's requeues.
// =====================================================================

struct Harness {
    ctx: HandlerContext,
    routes: Arc<ResourceProcessor<GRPCRoute>>,
    handler: GrpcRouteHandler,
}

impl Harness {
    fn new() -> Self {
        let runtime = Arc::new(ControllerRuntime::default());
        let routes = Arc::new(ResourceProcessor::<GRPCRoute>::new(
            "GRPCRoute",
            64,
            Arc::new(GrpcRouteHandler::default()),
            runtime.clone(),
        ));
        runtime.processor_registry.register(routes.clone());
        Self {
            ctx: HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime),
            routes,
            handler: GrpcRouteHandler::default(),
        }
    }

    /// One worker pass: parse, status, on_change, then save to the cache.
    async fn apply(&self, route: GRPCRoute) -> GRPCRoute {
        let mut parsed = match self.handler.parse(route, &self.ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };
        self.handler.update_status(&mut parsed, &self.ctx, &[]);
        self.handler.on_change(&parsed, &self.ctx).await;
        self.routes.save(parsed.clone(), None);
        parsed
    }

    async fn reprocess(&self, key: &str) -> GRPCRoute {
        let cached = self.routes.get(key).unwrap_or_else(|| panic!("{key} is cached"));
        self.apply(cached).await
    }

    async fn delete(&self, key: &str) {
        let cached = self.routes.get(key).unwrap_or_else(|| panic!("{key} is cached"));
        self.handler.on_delete(&cached, &self.ctx).await;
        self.routes.remove(key, None);
    }

    /// Collect every requeued GRPCRoute key until the queue stays quiet.
    async fn drain(&self) -> BTreeSet<String> {
        let queue = self.routes.workqueue();
        let mut keys = BTreeSet::new();
        while let Ok(Some(item)) = tokio::time::timeout(std::time::Duration::from_millis(300), queue.dequeue()).await {
            queue.done(&item.key);
            keys.insert(item.key);
        }
        keys
    }

    /// Reprocess requeued routes until the queue settles.
    async fn settle(&self) {
        for _ in 0..12 {
            let keys = self.drain().await;
            if keys.is_empty() {
                return;
            }
            for key in keys {
                if self.routes.get(&key).is_some() {
                    self.reprocess(&key).await;
                }
            }
        }
        panic!("delegation requeues did not settle");
    }
}

fn route(ns: &str, name: &str, spec: serde_json::Value) -> GRPCRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"namespace": ns, "name": name, "generation": 1},
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

fn plain_rule(grpc_service: &str, service: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"method": {"service": grpc_service}}],
        "backendRefs": [{"name": service, "port": 50051}]
    })
}

fn route_ref(ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "namespace": ns, "name": name})
}

fn root(ns: &str, name: &str, rules: serde_json::Value) -> GRPCRoute {
    route(
        ns,
        name,
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": rules}),
    )
}

fn child_with(ns: &str, name: &str, parents: &[(&str, &str)], rules: serde_json::Value) -> GRPCRoute {
    let refs: Vec<_> = parents.iter().map(|(ns, name)| route_ref(ns, name)).collect();
    route(ns, name, serde_json::json!({"parentRefs": refs, "rules": rules}))
}

fn child(ns: &str, name: &str, parents: &[(&str, &str)]) -> GRPCRoute {
    child_with(ns, name, parents, serde_json::json!([plain_rule("pkg.Api", "svc")]))
}

fn condition<'a>(route: &'a GRPCRoute, kind: Option<&str>, parent: &str, type_: &str) -> Option<&'a Condition> {
    let status = route.status.as_ref().expect("status is set");
    let entry = status
        .parents
        .iter()
        .find(|entry| entry.parent_ref.name == parent && entry.parent_ref.kind.as_deref() == kind)
        .unwrap_or_else(|| panic!("no status entry for {kind:?} {parent}"));
    entry.conditions.iter().find(|c| c.type_ == type_)
}

fn gateway_condition<'a>(route: &'a GRPCRoute, type_: &str) -> Option<&'a Condition> {
    condition(route, None, "gw", type_)
}

fn entry_condition<'a>(route: &'a GRPCRoute, parent: &str, type_: &str) -> &'a Condition {
    condition(route, Some("GRPCRoute"), parent, type_).unwrap_or_else(|| panic!("{type_} on entry {parent}"))
}

fn assert_condition(condition: &Condition, status: &str, reason: &str) {
    assert_eq!(
        (condition.status.as_str(), condition.reason.as_str()),
        (status, reason),
        "condition {}: {}",
        condition.type_,
        condition.message
    );
}

// =====================================================================
// Parent route (§7.1)
// =====================================================================

#[tokio::test]
async fn parent_partially_invalid_lists_only_own_malformed_rules_grpc() {
    let h = Harness::new();
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([{"matches": [{"method": {"type": "Exact"}}], "backendRefs": [{"name": "svc", "port": 50051}]},
                           plain_rule("pkg.V1", "svc")]),
    ))
    .await;
    let mut malformed = delegating_rule("pkg.Bad", "t", "c");
    malformed.as_object_mut().unwrap().remove("matches");
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([
                malformed,
                delegating_rule("pkg.Api", "t", "c"),
                plain_rule("pkg.Plain", "svc")
            ]),
        ))
        .await;
    let partially = gateway_condition(&parent, "PartiallyInvalid").expect("PartiallyInvalid is set");
    assert_condition(partially, "True", "RuleInvalid");
    assert!(partially.message.contains("rules[0]"), "{}", partially.message);
    assert!(partially.message.contains("delegate-all"), "{}", partially.message);
    assert!(!partially.message.contains("rules[1]"), "{}", partially.message);
    assert_eq!(
        partially.message.matches("rules[").count(),
        1,
        "child errors never reach the parent: {}",
        partially.message
    );
    assert_eq!(parent.spec.invalid_rule_indices, Some(vec![0]));
}

#[tokio::test]
async fn parent_missing_named_child_backend_not_found_grpc() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
        ))
        .await;
    let resolved = gateway_condition(&parent, "ResolvedRefs").unwrap();
    assert_condition(resolved, "False", "BackendNotFound");
    assert!(resolved.message.contains("t/c"), "{}", resolved.message);
}

#[tokio::test]
async fn parent_named_child_not_declaring_ref_not_permitted_grpc() {
    let h = Harness::new();
    h.apply(root("t", "c", serde_json::json!([plain_rule("pkg.Api", "svc")])))
        .await;
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
        ))
        .await;
    let resolved = gateway_condition(&parent, "ResolvedRefs").unwrap();
    assert_condition(resolved, "False", "RefNotPermitted");
    assert!(
        resolved.message.contains("t/c") && resolved.message.contains("parentRefs"),
        "the message names the child and the missing parentRef: {}",
        resolved.message
    );
}

#[tokio::test]
async fn parent_wildcard_without_children_is_clean_grpc() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("pkg.Api", "t", "*")]),
        ))
        .await;
    assert_condition(
        gateway_condition(&parent, "ResolvedRefs").unwrap(),
        "True",
        "ResolvedRefs",
    );
    assert!(gateway_condition(&parent, "PartiallyInvalid").is_none());
}

#[tokio::test]
async fn parent_status_ignores_child_errors_grpc() {
    let h = Harness::new();
    let mut with_hostnames = child("t", "c1", &[("infra", "p")]);
    with_hostnames.spec.hostnames = Some(vec!["child.example.com".to_string()]);
    h.apply(with_hostnames).await;
    h.apply(child_with(
        "t",
        "c2",
        &[("infra", "p")],
        serde_json::json!([{
            "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x", "value": "1"}]}}],
            "backendRefs": [{"name": "missing-svc", "port": 50051}]
        }]),
    ))
    .await;
    let mut rule = delegating_rule("pkg.Api", "t", "c1");
    rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "y", "value": "2"}]}}]);
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([rule, delegating_rule("pkg.B", "t", "c2")]),
    ))
    .await;
    h.settle().await;
    let parent = h.reprocess("infra/p").await;
    assert_condition(
        gateway_condition(&parent, "ResolvedRefs").unwrap(),
        "True",
        "ResolvedRefs",
    );
    assert!(gateway_condition(&parent, "PartiallyInvalid").is_none());
    assert!(parent.spec.invalid_rule_indices.is_none());
}

// =====================================================================
// Child entries (§7.2)
// =====================================================================

#[tokio::test]
async fn child_accepted_when_confirmed_and_reachable_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    // Child entries carry the route's own ResolvedRefs as well.
    let _ = entry_condition(&c, "p", "ResolvedRefs");
    assert!(condition(&c, Some("GRPCRoute"), "p", "PartiallyInvalid").is_none());
}

#[tokio::test]
async fn child_no_matching_parent_grpc() {
    let h = Harness::new();
    // Missing parent.
    let c = h.apply(child("t", "c", &[("infra", "missing")])).await;
    assert_condition(entry_condition(&c, "missing", "Accepted"), "False", "NoMatchingParent");

    // Existing parent that delegates elsewhere.
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "other")]),
    ))
    .await;
    let c = h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c2 = h.reprocess("t/c").await;
    for route in [&c, &c2] {
        assert_condition(entry_condition(route, "p", "Accepted"), "False", "NoMatchingParent");
    }
}

#[tokio::test]
async fn child_not_attached_when_parent_unreachable_grpc() {
    let h = Harness::new();
    h.apply(route(
        "infra",
        "p",
        serde_json::json!({"rules": [delegating_rule("pkg.Api", "t", "c")]}),
    ))
    .await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("infra/p"), "{}", accepted.message);
    assert_eq!(
        c.spec.resolved_delegation_keys.len(),
        1,
        "the edge is confirmed even when unreachable"
    );
}

#[tokio::test]
async fn child_depth_exceeded_when_parent_level_five_grpc() {
    let h = Harness::new();
    // l1 (root) -> l2 -> l3 -> l4 -> l5 -> l6
    h.apply(root(
        "t",
        "l1",
        serde_json::json!([delegating_rule("pkg.A", "t", "l2")]),
    ))
    .await;
    for level in 2..=5 {
        h.apply(child_with(
            "t",
            &format!("l{level}"),
            &[("t", &format!("l{}", level - 1))],
            serde_json::json!([delegating_rule("pkg.A", "t", &format!("l{}", level + 1))]),
        ))
        .await;
    }
    h.apply(child("t", "l6", &[("t", "l5")])).await;
    h.settle().await;
    let l5 = h.reprocess("t/l5").await;
    assert_condition(entry_condition(&l5, "l4", "Accepted"), "True", "DelegationAccepted");
    let l6 = h.reprocess("t/l6").await;
    let accepted = entry_condition(&l6, "l5", "Accepted");
    assert_condition(accepted, "False", "DepthExceeded");
    assert!(accepted.message.contains('5'), "{}", accepted.message);
}

#[tokio::test]
async fn child_conflict_keeps_accepted_and_adds_partially_invalid_grpc() {
    let h = Harness::new();
    let mut rule = delegating_rule("pkg.Api", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "p", "value": "1"}]}}]);
    h.apply(root("infra", "p", serde_json::json!([rule]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "c", "value": "1"}]}}],
             "backendRefs": [{"name": "svc", "port": 50051}]},
            plain_rule("pkg.Api", "svc")
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    let partially = entry_condition(&c, "p", "PartiallyInvalid");
    assert_condition(partially, "True", "RuleInvalid");
    assert!(
        partially.message.contains("rules[0]") && !partially.message.contains("rules[1]"),
        "{}",
        partially.message
    );
    assert!(partially.message.contains("infra/p"), "{}", partially.message);
    assert!(
        partially.message.starts_with("Conflicting Rule(s) with parent")
            && !partially.message.contains("Dropped")
            && !partially.message.contains("rules active"),
        "conflicting rules are not dropped: {}",
        partially.message
    );
}

/// Regression guard (C1): a backendRef-level filter of a mandatory type
/// conflicts exactly like a rule-level one.
#[tokio::test]
async fn child_backend_ref_filter_cannot_smuggle_mandatory_override_grpc() {
    let h = Harness::new();
    let mut rule = delegating_rule("pkg.Api", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
        "set": [{"name": "x-parent", "value": "1"}]
    }}]);
    h.apply(root("infra", "p", serde_json::json!([rule]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("pkg.Api", "svc"),
            {"matches": [{"method": {"service": "pkg.Api", "method": "Get"}}],
             "backendRefs": [{"name": "svc", "port": 50051, "filters": [
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"remove": ["x-parent"]}}
             ]}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    let partially = entry_condition(&c, "p", "PartiallyInvalid");
    assert_condition(partially, "True", "RuleInvalid");
    assert!(
        partially.message.contains("rules[1]") && !partially.message.contains("rules[0]"),
        "{}",
        partially.message
    );
    assert!(partially.message.contains("infra/p"), "{}", partially.message);
}

#[tokio::test]
async fn child_invalid_parent_refs_all_entries_false_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    // Mixed Gateway and route-kind parentRefs.
    let mixed = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [route_ref("infra", "p"), {"name": "gw"}],
            "rules": [plain_rule("pkg.Api", "svc")]
        }),
    );
    let c = h.apply(mixed).await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "False", "InvalidParentRefs");
    assert_condition(gateway_condition(&c, "Accepted").unwrap(), "False", "InvalidParentRefs");
    assert!(c.spec.resolved_delegation_keys.is_empty());

    // More than three route-kind parentRefs.
    let many = child(
        "t",
        "c",
        &[("infra", "p"), ("infra", "a"), ("infra", "b"), ("infra", "d")],
    );
    let c = h.apply(many).await;
    for parent in ["p", "a", "b", "d"] {
        assert_condition(entry_condition(&c, parent, "Accepted"), "False", "InvalidParentRefs");
    }
}

#[tokio::test]
async fn child_hostnames_unsupported_value_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    let mut c = child("t", "c", &[("infra", "p")]);
    c.spec.hostnames = Some(vec!["child.example.com".to_string()]);
    h.apply(c).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "UnsupportedValue");
    assert!(accepted.message.contains("hostnames"), "{}", accepted.message);
    assert!(
        c.spec.resolved_delegation_keys.is_empty(),
        "an inert child publishes no keys"
    );
}

#[tokio::test]
async fn root_detach_requeues_descendants_and_they_become_not_attached_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c1")]),
    ))
    .await;
    h.apply(child_with(
        "t",
        "c1",
        &[("infra", "p")],
        serde_json::json!([delegating_rule("pkg.Api", "t", "c2")]),
    ))
    .await;
    h.apply(child("t", "c2", &[("t", "c1")])).await;
    h.settle().await;
    assert_condition(
        entry_condition(&h.reprocess("t/c2").await, "c1", "Accepted"),
        "True",
        "DelegationAccepted",
    );
    h.drain().await;

    // The root loses its Gateway attachment.
    h.apply(route(
        "infra",
        "p",
        serde_json::json!({"rules": [delegating_rule("pkg.Api", "t", "c1")]}),
    ))
    .await;
    assert_eq!(h.drain().await, BTreeSet::from(["t/c1".to_string()]));
    let c1 = h.reprocess("t/c1").await;
    assert_condition(entry_condition(&c1, "p", "Accepted"), "False", "NotAttached");
    assert_eq!(h.drain().await, BTreeSet::from(["t/c2".to_string()]));
    let c2 = h.reprocess("t/c2").await;
    assert_condition(entry_condition(&c2, "c1", "Accepted"), "False", "NotAttached");
    assert!(h.drain().await.is_empty(), "propagation stops once levels are stable");
}

#[tokio::test]
async fn named_target_create_and_delete_requeue_parent_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    h.drain().await;

    // An ordinary route (no parentRefs) appears under the named target.
    h.apply(route(
        "t",
        "c",
        serde_json::json!({"rules": [plain_rule("pkg.Api", "svc")]}),
    ))
    .await;
    assert_eq!(h.drain().await, BTreeSet::from(["infra/p".to_string()]));
    let parent = h.reprocess("infra/p").await;
    assert_condition(
        gateway_condition(&parent, "ResolvedRefs").unwrap(),
        "False",
        "RefNotPermitted",
    );

    h.delete("t/c").await;
    assert_eq!(h.drain().await, BTreeSet::from(["infra/p".to_string()]));
    let parent = h.reprocess("infra/p").await;
    assert_condition(
        gateway_condition(&parent, "ResolvedRefs").unwrap(),
        "False",
        "BackendNotFound",
    );
}

#[tokio::test]
async fn invalid_shape_child_not_reported_on_parent_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    // The named child declares the parent, but mixes in a Gateway parentRef.
    h.apply(route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [route_ref("infra", "p"), {"name": "gw"}],
            "rules": [plain_rule("pkg.Api", "svc")]
        }),
    ))
    .await;
    h.settle().await;
    let parent = h.reprocess("infra/p").await;
    assert_condition(
        gateway_condition(&parent, "ResolvedRefs").unwrap(),
        "True",
        "ResolvedRefs",
    );
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "False", "InvalidParentRefs");
    assert!(c.spec.resolved_delegation_keys.is_empty());
    let graph = &h.ctx.runtime().grpc_route_delegation_graph;
    assert!(
        graph.confirmed_keys("t/c").is_empty(),
        "an invalid child is never a confirmed child"
    );
    assert_eq!(graph.level("t/c"), None);
    assert!(graph.children_of("infra/p").is_empty());
}

// =====================================================================
// Deep-review fixes
// =====================================================================

/// Design M1 (gRPC): a child method match naming a service other than the
/// delegated one is out of scope; serviceless and regex matches are not
/// checked.
#[tokio::test]
async fn child_rule_outside_delegated_service_is_partially_invalid_grpc() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("pkg.Api", "t", "c")]),
    ))
    .await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("pkg.Api", "svc"),
            plain_rule("pkg.Other", "svc"),
            {"matches": [{"method": {"method": "Get"}}], "backendRefs": [{"name": "svc", "port": 50051}]},
            {"matches": [{"method": {"type": "RegularExpression", "service": "pkg\\..*"}}],
             "backendRefs": [{"name": "svc", "port": 50051}]},
            {"backendRefs": [{"name": "svc", "port": 50051}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    assert_eq!(
        entry_condition(&c, "p", "PartiallyInvalid").message,
        "Rule(s) outside parent GRPCRoute 'infra/p' scope: rules[1]"
    );
}

/// Controller M1 (gRPC root): an invalid-hostname root confirms no delegation.
#[tokio::test]
async fn root_rejected_on_own_validation_leaves_children_not_attached_grpc() {
    let h = Harness::new();
    let mut broken = root("infra", "p", serde_json::json!([delegating_rule("pkg.Api", "t", "c")]));
    broken.spec.hostnames = Some(vec!["bad_host!".to_string()]);
    h.apply(broken).await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "False", "NotAttached");
}

/// Design M5 (gRPC): self-delegation gets a dedicated message.
#[tokio::test]
async fn root_delegating_to_itself_gets_a_dedicated_message_grpc() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("pkg.Api", "infra", "p"), plain_rule("pkg.Plain", "svc")]),
        ))
        .await;
    let resolved = gateway_condition(&parent, "ResolvedRefs").unwrap();
    assert_condition(resolved, "False", "RefNotPermitted");
    assert!(
        resolved.message.contains("cannot delegate to itself"),
        "{}",
        resolved.message
    );
}

// =====================================================================
// Round-3 fixes
// =====================================================================

fn rhm(name: &str) -> serde_json::Value {
    serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": name, "value": "1"}]}}])
}

/// Round 3 M5 (gRPC): with several delegating rules of different services,
/// a child rule conflicts only when it conflicts with every delegating rule
/// that can reach it.
#[tokio::test]
async fn conflict_is_reported_only_against_every_reaching_rule_grpc() {
    let h = Harness::new();
    let mut guarded = delegating_rule("pkg.A", "t", "c");
    guarded["filters"] = rhm("p");
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([guarded, delegating_rule("pkg.B", "t", "c")]),
    ))
    .await;
    let rule = |service: Option<&str>| {
        let mut rule = serde_json::json!({"filters": rhm("c"), "backendRefs": [{"name": "svc", "port": 50051}]});
        if let Some(service) = service {
            rule["matches"] = serde_json::json!([{"method": {"service": service}}]);
        }
        rule
    };
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        // pkg.A: only through the guarded rule (conflicts); pkg.B: only
        // through the unguarded one (serves); matchless: through both
        // (serves through pkg.B).
        serde_json::json!([rule(Some("pkg.A")), rule(Some("pkg.B")), rule(None)]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    assert_eq!(
        entry_condition(&c, "p", "PartiallyInvalid").message,
        "Conflicting Rule(s) with parent GRPCRoute 'infra/p': rules[0]: a standard filter conflicts with a mandatory filter of the same type"
    );
}

/// Round 3 M1 (gRPC): a child with no rule that can serve under the parent
/// (out of scope or conflicting) is not accepted under it.
#[tokio::test]
async fn child_with_no_serving_rule_under_parent_is_not_accepted_grpc() {
    let h = Harness::new();
    let mut guarded = delegating_rule("pkg.Api", "t", "c");
    guarded["filters"] = rhm("p");
    h.apply(root("infra", "p", serde_json::json!([guarded]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("pkg.Other", "svc"),
            {"matches": [{"method": {"service": "pkg.Api"}}], "filters": rhm("c"),
             "backendRefs": [{"name": "svc", "port": 50051}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "UnsupportedValue");
    assert_eq!(
        accepted.message,
        "No rule of this route can serve under parent GRPCRoute 'infra/p': rules[0], rules[1] are outside its scope or conflict with its filters"
    );

    // A serviceless (undecidable) rule counts as reachable and keeps it accepted.
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("pkg.Other", "svc"),
            {"matches": [{"method": {"method": "Get"}}], "backendRefs": [{"name": "svc", "port": 50051}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
}

/// Round 3 M-1 (gRPC): a blocked flip on an unattached parent refreshes the
/// child's `NotAttached` message.
#[tokio::test]
async fn blocked_flip_on_unattached_parent_refreshes_child_message_grpc() {
    let h = Harness::new();
    let unattached = || {
        route(
            "infra",
            "p",
            serde_json::json!({"rules": [delegating_rule("pkg.Api", "t", "c")]}),
        )
    };
    h.apply(unattached()).await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    h.drain().await;
    let mut broken = unattached();
    broken.spec.hostnames = Some(vec!["bad_host!".to_string()]);
    h.apply(broken).await;
    assert_eq!(h.drain().await, BTreeSet::from(["t/c".to_string()]));
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("is not accepted"), "{}", accepted.message);
}
