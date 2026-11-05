//! HTTPRoute delegation status (design §7): parent-side `PartiallyInvalid` /
//! `ResolvedRefs`, one child entry per route-kind parentRef, and the requeue
//! paths that keep both sides fresh.

use super::HttpRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler, ResourceProcessor,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::HTTPRoute;
use edgion_resources::resources::common::Condition;
use std::collections::BTreeSet;
use std::sync::Arc;

// =====================================================================
// Harness: a registered HTTPRoute processor provides the cache (route
// existence) and the workqueue that receives the handler's requeues.
// =====================================================================

struct Harness {
    ctx: HandlerContext,
    routes: Arc<ResourceProcessor<HTTPRoute>>,
    handler: HttpRouteHandler,
}

impl Harness {
    fn new() -> Self {
        let runtime = Arc::new(ControllerRuntime::default());
        let routes = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            64,
            Arc::new(HttpRouteHandler::default()),
            runtime.clone(),
        ));
        runtime.processor_registry.register(routes.clone());
        Self {
            ctx: HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime),
            routes,
            handler: HttpRouteHandler::default(),
        }
    }

    /// One worker pass: parse, status, on_change, then save to the cache.
    async fn apply(&self, route: HTTPRoute) -> HTTPRoute {
        let mut parsed = match self.handler.parse(route, &self.ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };
        self.handler.update_status(&mut parsed, &self.ctx, &[]);
        self.handler.on_change(&parsed, &self.ctx).await;
        self.routes.save(parsed.clone(), None);
        parsed
    }

    async fn reprocess(&self, key: &str) -> HTTPRoute {
        let cached = self.routes.get(key).unwrap_or_else(|| panic!("{key} is cached"));
        self.apply(cached).await
    }

    async fn delete(&self, key: &str) {
        let cached = self.routes.get(key).unwrap_or_else(|| panic!("{key} is cached"));
        self.handler.on_delete(&cached, &self.ctx).await;
        self.routes.remove(key, None);
    }

    /// Collect every requeued HTTPRoute key until the queue stays quiet.
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

fn route(ns: &str, name: &str, spec: serde_json::Value) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"namespace": ns, "name": name, "generation": 1},
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

fn plain_rule(prefix: &str, service: &str) -> serde_json::Value {
    serde_json::json!({
        "matches": [{"path": {"type": "PathPrefix", "value": prefix}}],
        "backendRefs": [{"name": service, "port": 80}]
    })
}

fn route_ref(ns: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": ns, "name": name})
}

fn root(ns: &str, name: &str, rules: serde_json::Value) -> HTTPRoute {
    route(
        ns,
        name,
        serde_json::json!({"parentRefs": [{"name": "gw"}], "rules": rules}),
    )
}

fn child_with(ns: &str, name: &str, parents: &[(&str, &str)], rules: serde_json::Value) -> HTTPRoute {
    let refs: Vec<_> = parents.iter().map(|(ns, name)| route_ref(ns, name)).collect();
    route(ns, name, serde_json::json!({"parentRefs": refs, "rules": rules}))
}

fn child(ns: &str, name: &str, parents: &[(&str, &str)]) -> HTTPRoute {
    child_with(ns, name, parents, serde_json::json!([plain_rule("/api/v1", "svc")]))
}

fn condition<'a>(route: &'a HTTPRoute, kind: Option<&str>, parent: &str, type_: &str) -> Option<&'a Condition> {
    let status = route.status.as_ref().expect("status is set");
    let entry = status
        .parents
        .iter()
        .find(|entry| entry.parent_ref.name == parent && entry.parent_ref.kind.as_deref() == kind)
        .unwrap_or_else(|| panic!("no status entry for {kind:?} {parent}"));
    entry.conditions.iter().find(|c| c.type_ == type_)
}

fn gateway_condition<'a>(route: &'a HTTPRoute, type_: &str) -> Option<&'a Condition> {
    condition(route, None, "gw", type_)
}

fn entry_condition<'a>(route: &'a HTTPRoute, parent: &str, type_: &str) -> &'a Condition {
    condition(route, Some("HTTPRoute"), parent, type_).unwrap_or_else(|| panic!("{type_} on entry {parent}"))
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
async fn parent_partially_invalid_lists_only_own_malformed_rules() {
    let h = Harness::new();
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([{"filters": [{"type": "CORS"}], "backendRefs": [{"name": "svc", "port": 80}]},
                           plain_rule("/api/v1", "svc")]),
    ))
    .await;
    let mut malformed = delegating_rule("/bad", "t", "c");
    malformed["matches"][0]["path"]["type"] = "Exact".into();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([
                malformed,
                delegating_rule("/api", "t", "c"),
                plain_rule("/plain", "svc")
            ]),
        ))
        .await;
    let partially = gateway_condition(&parent, "PartiallyInvalid").expect("PartiallyInvalid is set");
    assert_condition(partially, "True", "RuleInvalid");
    assert!(partially.message.contains("rules[0]"), "{}", partially.message);
    assert!(partially.message.contains("PathPrefix"), "{}", partially.message);
    assert!(!partially.message.contains("rules[1]"), "{}", partially.message);
    assert!(
        !partially.message.contains("CORS"),
        "child errors never reach the parent"
    );
    assert_eq!(parent.spec.invalid_rule_indices, Some(vec![0]));
}

#[tokio::test]
async fn parent_missing_named_child_backend_not_found() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
        ))
        .await;
    let resolved = gateway_condition(&parent, "ResolvedRefs").unwrap();
    assert_condition(resolved, "False", "BackendNotFound");
    assert!(resolved.message.contains("t/c"), "{}", resolved.message);
}

#[tokio::test]
async fn parent_named_child_not_declaring_ref_not_permitted() {
    let h = Harness::new();
    h.apply(root("t", "c", serde_json::json!([plain_rule("/api", "svc")])))
        .await;
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("/api", "t", "c")]),
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
async fn parent_wildcard_without_children_is_clean() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("/api", "t", "*")]),
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
async fn parent_status_ignores_child_errors() {
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
            "backendRefs": [{"name": "missing-svc", "port": 80}]
        }]),
    ))
    .await;
    let mut rule = delegating_rule("/api", "t", "c1");
    rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "y", "value": "2"}]}}]);
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([rule, delegating_rule("/b", "t", "c2")]),
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
async fn child_accepted_when_confirmed_and_reachable() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    // Child entries carry the route's own ResolvedRefs as well.
    let _ = entry_condition(&c, "p", "ResolvedRefs");
    assert!(condition(&c, Some("HTTPRoute"), "p", "PartiallyInvalid").is_none());
}

#[tokio::test]
async fn child_no_matching_parent() {
    let h = Harness::new();
    // Missing parent.
    let c = h.apply(child("t", "c", &[("infra", "missing")])).await;
    assert_condition(entry_condition(&c, "missing", "Accepted"), "False", "NoMatchingParent");

    // Existing parent that delegates elsewhere.
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "other")]),
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
async fn child_not_attached_when_parent_unreachable() {
    let h = Harness::new();
    h.apply(route(
        "infra",
        "p",
        serde_json::json!({"rules": [delegating_rule("/api", "t", "c")]}),
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
async fn child_depth_exceeded_when_parent_level_five() {
    let h = Harness::new();
    // l1 (root) -> l2 -> l3 -> l4 -> l5 -> l6
    h.apply(root("t", "l1", serde_json::json!([delegating_rule("/a", "t", "l2")])))
        .await;
    for level in 2..=5 {
        h.apply(child_with(
            "t",
            &format!("l{level}"),
            &[("t", &format!("l{}", level - 1))],
            serde_json::json!([delegating_rule("/a", "t", &format!("l{}", level + 1))]),
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
async fn child_conflict_keeps_accepted_and_adds_partially_invalid() {
    let h = Harness::new();
    let mut rule = delegating_rule("/api", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "p", "value": "1"}]}}]);
    h.apply(root("infra", "p", serde_json::json!([rule]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "c", "value": "1"}]}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            plain_rule("/api/v2", "svc")
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

/// Regression guard (C1): a backendRef-level filter runs after the route
/// chain, so a child must not be able to override a parent's mandatory
/// filter by moving the same filter type onto a backendRef.
#[tokio::test]
async fn child_backend_ref_filter_cannot_smuggle_mandatory_override() {
    let h = Harness::new();
    let mut rule = delegating_rule("/api", "t", "c");
    rule["filters"] = serde_json::json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
        "set": [{"name": "Strict-Transport-Security", "value": "max-age=31536000"}]
    }}]);
    h.apply(root("infra", "p", serde_json::json!([rule]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("/api/v1", "svc"),
            {"matches": [{"path": {"type": "PathPrefix", "value": "/api/v2"}}],
             "backendRefs": [{"name": "svc", "port": 80, "filters": [
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"remove": ["Strict-Transport-Security"]}}
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
async fn child_invalid_parent_refs_all_entries_false() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    // Mixed Gateway and route-kind parentRefs.
    let mixed = route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [route_ref("infra", "p"), {"name": "gw"}],
            "rules": [plain_rule("/api", "svc")]
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
async fn child_hostnames_unsupported_value() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
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
async fn root_detach_requeues_descendants_and_they_become_not_attached() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c1")]),
    ))
    .await;
    h.apply(child_with(
        "t",
        "c1",
        &[("infra", "p")],
        serde_json::json!([delegating_rule("/api/v1", "t", "c2")]),
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
        serde_json::json!({"rules": [delegating_rule("/api", "t", "c1")]}),
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
async fn named_target_create_and_delete_requeue_parent() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    h.drain().await;

    // An ordinary route (no parentRefs) appears under the named target.
    h.apply(route(
        "t",
        "c",
        serde_json::json!({"rules": [plain_rule("/api", "svc")]}),
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
async fn invalid_shape_child_not_reported_on_parent() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    // The named child declares the parent, but mixes in a Gateway parentRef.
    h.apply(route(
        "t",
        "c",
        serde_json::json!({
            "parentRefs": [route_ref("infra", "p"), {"name": "gw"}],
            "rules": [plain_rule("/api", "svc")]
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
    let graph = &h.ctx.runtime().http_route_delegation_graph;
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

fn rhm(name: &str) -> serde_json::Value {
    serde_json::json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": name, "value": "1"}]}}])
}

/// Controller M1 (root): a root rejected on its own validation confirms no
/// delegation; its children are `NotAttached` until it is fixed.
#[tokio::test]
async fn root_rejected_on_own_validation_leaves_children_not_attached() {
    let h = Harness::new();
    let mut broken = root("infra", "p", serde_json::json!([delegating_rule("/api", "t", "c")]));
    broken.spec.hostnames = Some(vec!["bad_host!".to_string()]);
    let parent = h.apply(broken).await;
    assert_condition(
        gateway_condition(&parent, "Accepted").unwrap(),
        "False",
        "UnsupportedValue",
    );
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("is not accepted"), "{}", accepted.message);
    h.drain().await;

    // Fixing the root restores the child through the level-change requeue.
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    assert_eq!(h.drain().await, BTreeSet::from(["t/c".to_string()]));
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
}

/// Controller M1 (middle): a middle route with its own validation error
/// cuts its subtree off.
#[tokio::test]
async fn middle_rejected_on_own_validation_leaves_grandchild_not_attached() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "m")]),
    ))
    .await;
    let bad_regex = serde_json::json!({
        "matches": [{"path": {"type": "RegularExpression", "value": "/api/("}}],
        "backendRefs": [{"name": "svc", "port": 80}]
    });
    h.apply(child_with(
        "t",
        "m",
        &[("infra", "p")],
        serde_json::json!([delegating_rule("/api/v1", "t", "g"), bad_regex]),
    ))
    .await;
    h.apply(child("t", "g", &[("t", "m")])).await;
    h.settle().await;
    let graph = &h.ctx.runtime().http_route_delegation_graph;
    assert!(graph.is_blocked("t/m"));
    assert_eq!(graph.level("t/m"), None);
    let g = h.reprocess("t/g").await;
    let accepted = entry_condition(&g, "m", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("t/m"), "{}", accepted.message);
}

/// Design M1: a child rule that can never intersect the parent's delegated
/// prefix is reported (segment boundary as in the Gateway matcher); regex
/// and pathless rules are not checked.
#[tokio::test]
async fn child_rule_outside_parent_scope_is_partially_invalid() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api/v1/billing/", "t", "c")]),
    ))
    .await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("/api/v1/billing/pay", "svc"),
            plain_rule("/api/v1/billingx", "svc"),
            {"matches": [{"path": {"type": "Exact", "value": "/api/v1/biling/pay"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            {"matches": [{"path": {"type": "RegularExpression", "value": "/zzz/.*"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            {"backendRefs": [{"name": "svc", "port": 80}]},
            plain_rule("/api", "svc"),
            {"matches": [{"path": {"type": "Exact", "value": "/api/v1/billing"}}],
             "backendRefs": [{"name": "svc", "port": 80}]},
            {"matches": [{"path": {"type": "Exact", "value": "/other"}},
                         {"path": {"type": "PathPrefix", "value": "/api/v1/billing/x"}}],
             "backendRefs": [{"name": "svc", "port": 80}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
    let partially = entry_condition(&c, "p", "PartiallyInvalid");
    assert_condition(partially, "True", "RuleInvalid");
    assert_eq!(
        partially.message,
        "Rule(s) outside parent HTTPRoute 'infra/p' scope: rules[1], rules[2]"
    );
}

/// Design M3: a child rule is conflicting only when it conflicts with every
/// delegating rule that can reach it.
#[tokio::test]
async fn conflict_is_reported_only_against_every_reaching_rule() {
    let h = Harness::new();
    let mut guarded = delegating_rule("/a", "t", "c");
    guarded["filters"] = rhm("p");
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([guarded, delegating_rule("/b", "t", "c")]),
    ))
    .await;
    let rule = |prefix: Option<&str>| {
        let mut rule = serde_json::json!({"filters": rhm("c"), "backendRefs": [{"name": "svc", "port": 80}]});
        if let Some(prefix) = prefix {
            rule["matches"] = serde_json::json!([{"path": {"type": "PathPrefix", "value": prefix}}]);
        }
        rule
    };
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([rule(Some("/a/x")), rule(Some("/b/x")), rule(None)]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let partially = entry_condition(&c, "p", "PartiallyInvalid");
    assert_eq!(
        partially.message,
        "Conflicting Rule(s) with parent HTTPRoute 'infra/p': rules[0]: a standard filter conflicts with a mandatory filter of the same type"
    );
}

/// Design M5: self-delegation gets a dedicated message, not the
/// misleading "does not list this route in its parentRefs" hint.
#[tokio::test]
async fn root_delegating_to_itself_gets_a_dedicated_message() {
    let h = Harness::new();
    let parent = h
        .apply(root(
            "infra",
            "p",
            serde_json::json!([delegating_rule("/api", "infra", "p"), plain_rule("/plain", "svc")]),
        ))
        .await;
    let resolved = gateway_condition(&parent, "ResolvedRefs").unwrap();
    assert_condition(resolved, "False", "RefNotPermitted");
    assert!(
        resolved.message.contains("cannot delegate to itself"),
        "{}",
        resolved.message
    );
    assert!(!resolved.message.contains("parentRefs"), "{}", resolved.message);
}

// =====================================================================
// Round-3 fixes
// =====================================================================

/// Round 3 M1: when every child rule is outside the parent's scope or
/// conflicts with its mandatory filters, nothing can serve under that
/// parent, so the entry is not accepted (as on the parent side).
#[tokio::test]
async fn child_with_no_serving_rule_under_parent_is_not_accepted() {
    let h = Harness::new();
    let mut guarded = delegating_rule("/api", "t", "c");
    guarded["filters"] = rhm("p");
    h.apply(root("infra", "p", serde_json::json!([guarded]))).await;
    h.apply(child_with(
        "t",
        "c",
        &[("infra", "p")],
        serde_json::json!([
            plain_rule("/other", "svc"),
            {"matches": [{"path": {"type": "PathPrefix", "value": "/api/x"}}],
             "filters": rhm("c"), "backendRefs": [{"name": "svc", "port": 80}]}
        ]),
    ))
    .await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "UnsupportedValue");
    assert_eq!(
        accepted.message,
        "No rule of this route can serve under parent HTTPRoute 'infra/p': rules[0], rules[1] are outside its scope or conflict with its filters"
    );
    assert!(condition(&c, Some("HTTPRoute"), "p", "PartiallyInvalid").is_none_or(|p| p.status != "True"));
}

/// A rule the scope check cannot decide (regex, no path) counts as
/// reachable, so it keeps the entry accepted.
#[tokio::test]
async fn undecidable_child_rule_keeps_entry_accepted() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "c")]),
    ))
    .await;
    for undecidable in [
        serde_json::json!({"matches": [{"path": {"type": "RegularExpression", "value": "/zzz/.*"}}],
                           "backendRefs": [{"name": "svc", "port": 80}]}),
        serde_json::json!({"matches": [{"headers": [{"name": "x", "value": "1"}]}],
                           "backendRefs": [{"name": "svc", "port": 80}]}),
    ] {
        h.apply(child_with(
            "t",
            "c",
            &[("infra", "p")],
            serde_json::json!([plain_rule("/other", "svc"), undecidable]),
        ))
        .await;
        h.settle().await;
        let c = h.reprocess("t/c").await;
        assert_condition(entry_condition(&c, "p", "Accepted"), "True", "DelegationAccepted");
        assert_eq!(
            entry_condition(&c, "p", "PartiallyInvalid").message,
            "Rule(s) outside parent HTTPRoute 'infra/p' scope: rules[0]"
        );
    }
}

/// Round 3 M-1: a parent without a level that becomes blocked (or is
/// fixed) requeues its children, whose `NotAttached` message changes.
#[tokio::test]
async fn blocked_flip_on_unattached_parent_refreshes_child_message() {
    let h = Harness::new();
    let unattached = || {
        route(
            "infra",
            "p",
            serde_json::json!({"rules": [delegating_rule("/api", "t", "c")]}),
        )
    };
    h.apply(unattached()).await;
    h.apply(child("t", "c", &[("infra", "p")])).await;
    h.settle().await;
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("is not reachable"), "{}", accepted.message);
    h.drain().await;

    let mut broken = unattached();
    broken.spec.hostnames = Some(vec!["bad_host!".to_string()]);
    h.apply(broken).await;
    assert_eq!(h.drain().await, BTreeSet::from(["t/c".to_string()]));
    let c = h.reprocess("t/c").await;
    let accepted = entry_condition(&c, "p", "Accepted");
    assert_condition(accepted, "False", "NotAttached");
    assert!(accepted.message.contains("is not accepted"), "{}", accepted.message);
    h.drain().await;

    h.apply(unattached()).await;
    assert_eq!(h.drain().await, BTreeSet::from(["t/c".to_string()]));
    let c = h.reprocess("t/c").await;
    assert!(
        entry_condition(&c, "p", "Accepted")
            .message
            .contains("is not reachable"),
        "{}",
        entry_condition(&c, "p", "Accepted").message
    );
}

/// Round 3 M5: fixing a blocked middle route restores its grandchild.
#[tokio::test]
async fn unblocking_middle_restores_grandchild() {
    let h = Harness::new();
    h.apply(root(
        "infra",
        "p",
        serde_json::json!([delegating_rule("/api", "t", "m")]),
    ))
    .await;
    let bad_regex = serde_json::json!({
        "matches": [{"path": {"type": "RegularExpression", "value": "/api/("}}],
        "backendRefs": [{"name": "svc", "port": 80}]
    });
    h.apply(child_with(
        "t",
        "m",
        &[("infra", "p")],
        serde_json::json!([delegating_rule("/api/v1", "t", "g"), bad_regex]),
    ))
    .await;
    h.apply(child("t", "g", &[("t", "m")])).await;
    h.settle().await;
    assert_condition(
        entry_condition(&h.reprocess("t/g").await, "m", "Accepted"),
        "False",
        "NotAttached",
    );
    h.drain().await;

    h.apply(child_with(
        "t",
        "m",
        &[("infra", "p")],
        serde_json::json!([delegating_rule("/api/v1", "t", "g")]),
    ))
    .await;
    let graph = &h.ctx.runtime().http_route_delegation_graph;
    assert!(!graph.is_blocked("t/m"));
    assert_eq!(graph.level("t/m"), Some(2));
    assert_eq!(h.drain().await, BTreeSet::from(["t/g".to_string()]));
    let g = h.reprocess("t/g").await;
    assert_condition(entry_condition(&g, "m", "Accepted"), "True", "DelegationAccepted");
    assert_eq!(graph.level("t/g"), Some(3));
}
