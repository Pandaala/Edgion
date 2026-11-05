use super::*;
use crate::routes::http::routes_mgr::{DomainRouteRules, GlobalHttpRouteManagers};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::route_delegation::{
    key_for, standard_filter_mask_http, DelegationTarget, DelegationTargetName, RouteKindName,
};
use edgion_resources::{ListenerKey, ResourceMeta};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn listener(port: u16) -> ListenerKey {
    ListenerKey::new(format!("0.0.0.0:{port}").parse().unwrap(), "HTTP")
}

fn key(parent: &str, child: &str) -> DelegationKey {
    key_for(
        RouteKindName::HTTPRoute,
        "default",
        parent,
        &DelegationTarget {
            ns: "default".into(),
            name: DelegationTargetName::Named(child.into()),
        },
    )
}

/// Root route attached to a Gateway with one delegating rule (`/api` -> child)
/// and one ordinary sibling rule (`/other`).
fn parent(name: &str, child: &str, port: u16) -> Arc<HTTPRoute> {
    let mut route: HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"}],
            "rules": [
                {"matches": [{"path": {"type": "PathPrefix", "value": format!("/{name}")}}],
                 "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": child}]},
                {"matches": [{"path": {"type": "PathPrefix", "value": "/other"}}],
                 "backendRefs": [{"name": "svc", "port": 80}]}
            ]
        }
    }))
    .unwrap();
    route.spec.resolved_listeners = Some(vec![listener(port)]);
    route.pre_parse();
    Arc::new(route)
}

/// Child route declaring `parent` and carrying the Controller-confirmed key.
fn child(name: &str, parent: &str, path: &str) -> Arc<HTTPRoute> {
    let mut route: HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": parent}],
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": path}}],
                       "backendRefs": [{"name": "svc", "port": 80}]}]
        }
    }))
    .unwrap();
    route.spec.resolved_delegation_keys = vec![key(parent, name)];
    route.pre_parse();
    Arc::new(route)
}

fn ordinary(name: &str, port: u16) -> Arc<HTTPRoute> {
    let mut route: HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"}],
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": format!("/{name}")}}],
                       "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}],
                       "backendRefs": [{"name": "svc", "port": 80}]}]
        }
    }))
    .unwrap();
    route.spec.resolved_listeners = Some(vec![listener(port)]);
    route.pre_parse();
    Arc::new(route)
}

fn data(routes: &[&Arc<HTTPRoute>]) -> HashMap<String, Arc<HTTPRoute>> {
    routes
        .iter()
        .map(|route| (route.key_name(), Arc::clone(route)))
        .collect()
}

fn table(mgr: &GlobalHttpRouteManagers, port: u16) -> Arc<DomainRouteRules> {
    Arc::clone(&mgr.get_or_create_listener_manager(&listener(port)).load_route_table())
}

fn root_unit(table: &DomainRouteRules, path: &str) -> Arc<crate::routes::http::HttpRouteRuleUnit> {
    table
        .catch_all_routes
        .as_ref()
        .expect("catch-all group")
        .route_rules_list
        .iter()
        .find(|unit| unit.matched_info.m.path.as_ref().and_then(|p| p.value.as_deref()) == Some(path))
        .cloned()
        .expect("unit for path")
}

fn subtree_paths(slot: &crate::routes::delegation::DelegationSlot<RouteRules>) -> Vec<String> {
    let mut paths: Vec<String> = slot
        .load()
        .map(|rules| {
            rules
                .route_rules_list
                .iter()
                .filter_map(|unit| unit.matched_info.m.path.as_ref().and_then(|p| p.value.clone()))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

#[test]
fn delegating_unit_holds_slot() {
    let mgr = GlobalHttpRouteManagers::new();
    let p = parent("dhs", "dhs-child", 18601);
    mgr.full_set(&data(&[&p]));
    let table = table(&mgr, 18601);
    let delegating = root_unit(&table, "/dhs");
    let slot = delegating.delegate.as_ref().expect("delegating unit holds a slot");
    assert!(Arc::ptr_eq(
        slot,
        &mgr.delegation_slots().get_or_create(&key("dhs", "dhs-child"))
    ));
    assert!(delegating.backends.is_empty(), "a delegating unit selects no backend");
    assert!(slot.load().is_none(), "no child published yet");
    let sibling = root_unit(&table, "/other");
    assert!(sibling.delegate.is_none());
    assert_eq!(sibling.backends.len(), 1);
}

#[test]
fn ordinary_unit_has_no_delegate_and_zero_mask_cost() {
    let mgr = GlobalHttpRouteManagers::new();
    let route = ordinary("oun", 18602);
    mgr.full_set(&data(&[&route]));
    let unit = root_unit(&table(&mgr, 18602), "/oun");
    assert!(unit.delegate.is_none());
    // The mask is precomputed once at build time; nothing is derived per request.
    let rule = &route.spec.rules.as_ref().unwrap()[0];
    assert_eq!(unit.filter_mask, standard_filter_mask_http(rule));
    assert_ne!(unit.filter_mask, 0);
    assert!(mgr.delegation_slots().is_empty());
}

#[test]
fn child_change_rebuilds_only_its_slots_not_listener() {
    let mgr = GlobalHttpRouteManagers::new();
    let (p1, p2) = (parent("ccr1", "ccr1-child", 18603), parent("ccr2", "ccr2-child", 18603));
    let (c1, c2) = (
        child("ccr1-child", "ccr1", "/ccr1/a"),
        child("ccr2-child", "ccr2", "/ccr2/a"),
    );
    mgr.full_set(&data(&[&p1, &p2, &c1, &c2]));
    let before = table(&mgr, 18603);
    let slot1 = mgr.delegation_slots().get_or_create(&key("ccr1", "ccr1-child"));
    let slot2 = mgr.delegation_slots().get_or_create(&key("ccr2", "ccr2-child"));
    assert_eq!(subtree_paths(&slot1), ["/ccr1/a"]);
    let other_before = slot2.load().unwrap();

    let updated = child("ccr1-child", "ccr1", "/ccr1/b");
    mgr.partial_update(HashMap::new(), data(&[&updated]), HashSet::new());

    assert!(
        Arc::ptr_eq(&before, &table(&mgr, 18603)),
        "a child-only change must not rebuild any listener table"
    );
    assert_eq!(subtree_paths(&slot1), ["/ccr1/b"]);
    assert!(
        Arc::ptr_eq(&other_before, &slot2.load().unwrap()),
        "slots of unrelated keys are not stored"
    );
    assert_eq!(mgr.delegation_slots().len(), 2);
}

#[test]
fn child_before_parent_then_parent_arrives() {
    let mgr = GlobalHttpRouteManagers::new();
    let c = child("cbp-child", "cbp", "/cbp/users");
    mgr.partial_update(data(&[&c]), HashMap::new(), HashSet::new());
    assert_eq!(mgr.delegation_slots().len(), 1);
    let slot = mgr.delegation_slots().get_or_create(&key("cbp", "cbp-child"));
    let subtree = slot.load().expect("child publication fills its slot");
    assert_eq!(subtree_paths(&slot), ["/cbp/users"]);
    let unit = &subtree.route_rules_list[0];
    assert!(unit.parent_refs.is_none(), "child units carry no Gateway eligibility");
    assert_eq!(unit.matched_info.rsc.name, "cbp-child");

    let p = parent("cbp", "cbp-child", 18604);
    mgr.partial_update(data(&[&p]), HashMap::new(), HashSet::new());
    let delegating = root_unit(&table(&mgr, 18604), "/cbp");
    assert!(Arc::ptr_eq(delegating.delegate.as_ref().unwrap(), &slot));
    assert!(
        Arc::ptr_eq(&slot.load().unwrap(), &subtree),
        "parent arrival keeps the subtree"
    );
}

#[test]
fn parent_deleted_slot_retained_until_unreferenced() {
    let mgr = GlobalHttpRouteManagers::new();
    let p = parent("pds", "pds-child", 18605);
    let c = child("pds-child", "pds", "/pds/a");
    mgr.full_set(&data(&[&p, &c]));
    assert_eq!(mgr.delegation_slots().len(), 1);

    // Parent gone: the child still holds a subtree, so the slot stays.
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([p.key_name()]));
    assert_eq!(mgr.delegation_slots().len(), 1);
    // Child gone too: nobody references the slot any more.
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([c.key_name()]));
    assert_eq!(mgr.delegation_slots().len(), 0);

    // Reverse order: the parent unit keeps the emptied slot alive.
    mgr.full_set(&data(&[&p, &c]));
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([c.key_name()]));
    assert_eq!(mgr.delegation_slots().len(), 1);
    let delegating = root_unit(&table(&mgr, 18605), "/pds");
    assert!(delegating.delegate.as_ref().unwrap().load().is_none());
    drop(delegating);
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([p.key_name()]));
    assert_eq!(mgr.delegation_slots().len(), 0);

    // full_set from scratch drops every slot nobody references.
    mgr.full_set(&data(&[&p, &c]));
    assert_eq!(mgr.delegation_slots().len(), 1);
    mgr.full_set(&HashMap::new());
    assert_eq!(mgr.delegation_slots().len(), 0);
}

#[test]
fn ordinary_only_rebuild_touches_zero_slots() {
    let mgr = GlobalHttpRouteManagers::new();
    let (a, b) = (ordinary("oor-a", 18606), ordinary("oor-b", 18606));
    mgr.full_set(&data(&[&a, &b]));
    assert_eq!(mgr.delegation_slots().len(), 0);
    let b2 = ordinary("oor-b", 18606);
    mgr.partial_update(HashMap::new(), data(&[&b2]), HashSet::from([a.key_name()]));
    assert_eq!(mgr.delegation_slots().len(), 0);
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_warnings(run: impl FnOnce()) -> String {
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let logs = String::from_utf8(buffer.0.lock().clone()).unwrap();
    logs
}

#[test]
fn children_not_bucketed_by_listener_no_warn() {
    let mgr = GlobalHttpRouteManagers::new();
    let p = parent("cnb", "cnb-child", 18607);
    let c = child("cnb-child", "cnb", "/cnb/a");
    let logs = capture_warnings(|| {
        mgr.full_set(&data(&[&p, &c]));
        let c2 = child("cnb-child", "cnb", "/cnb/b");
        mgr.partial_update(HashMap::new(), data(&[&c2]), HashSet::new());
    });
    assert!(!logs.contains("resolved_listeners"), "{logs}");
    let buckets = mgr.bucket_routes_by_listener();
    assert!(buckets.values().all(|routes| !routes.contains_key(&c.key_name())));
    assert!(buckets[&listener(18607)].contains_key(&p.key_name()));
}

#[test]
fn build_subtree_compiles_children_with_their_own_identity() {
    let slots = DelegationSlotTable::new();
    let a = child("bs-a", "bs", "/bs/a");
    let b = child("bs-b", "bs", "/bs/b");
    let subtree = build_subtree(&[a, b], &slots, &[]).unwrap();
    let mut names: Vec<_> = subtree
        .route_rules_list
        .iter()
        .map(|unit| unit.matched_info.rsc.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["bs-a", "bs-b"]);
    assert!(subtree.match_engine.is_some());
    assert!(build_subtree(&[], &slots, &[]).is_none());
    assert!(slots.is_empty());
}

#[test]
fn child_keys_require_own_parent_ref_and_controller_acceptance() {
    // A key whose parent the child does not declare is rejected locally.
    let mut forged = (*child("ck-child", "ck", "/ck/a")).clone();
    forged
        .spec
        .resolved_delegation_keys
        .push(key("someone-else", "ck-child"));
    assert_eq!(child_accepted_keys(&forged), vec![key("ck", "ck-child")]);
    // Controller status rejecting the edge withdraws the key.
    let mut rejected = forged.clone();
    for parent in &mut rejected.spec.current_status.as_mut().unwrap().parents {
        for condition in &mut parent.conditions {
            condition.status = "False".into();
        }
    }
    assert!(child_accepted_keys(&rejected).is_empty());
    // Ordinary routes take the empty fast path.
    assert!(child_accepted_keys(&ordinary("ck-ord", 18608)).is_empty());
}

#[test]
fn child_invalid_rule_indices_are_honored() {
    let mut route = (*child("cir-child", "cir", "/cir/a")).clone();
    route.spec.rules.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/cir/b"}}],
            "backendRefs": [{"name": "svc", "port": 80}]
        }))
        .unwrap(),
    );
    route.spec.invalid_rule_indices = Some(vec![0]);
    route.pre_parse();
    let slots = DelegationSlotTable::new();
    let subtree = build_subtree(&[Arc::new(route)], &slots, &[]).unwrap();
    let paths: Vec<_> = subtree
        .route_rules_list
        .iter()
        .filter_map(|unit| unit.matched_info.m.path.as_ref().and_then(|p| p.value.clone()))
        .collect();
    assert_eq!(paths, ["/cir/b"]);
}

#[test]
fn mid_level_child_delegating_rule_holds_next_slot() {
    let mgr = GlobalHttpRouteManagers::new();
    let p = parent("mlc", "mlc-mid", 18609);
    // The mid route is a child of `mlc` and delegates `/mlc/deep` onward.
    let mut mid = (*child("mlc-mid", "mlc", "/mlc/a")).clone();
    mid.spec.rules.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/mlc/deep"}}],
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "mlc-leaf"}]
        }))
        .unwrap(),
    );
    mid.pre_parse();
    let mid = Arc::new(mid);
    let leaf = child("mlc-leaf", "mlc-mid", "/mlc/deep/x");
    mgr.full_set(&data(&[&p, &mid, &leaf]));
    assert_eq!(mgr.delegation_slots().len(), 2);
    let first = mgr.delegation_slots().get_or_create(&key("mlc", "mlc-mid"));
    let second = mgr.delegation_slots().get_or_create(&key("mlc-mid", "mlc-leaf"));
    let subtree = first.load().unwrap();
    let delegating = subtree
        .route_rules_list
        .iter()
        .find(|unit| unit.delegate.is_some())
        .expect("mid-level delegating unit");
    assert!(Arc::ptr_eq(delegating.delegate.as_ref().unwrap(), &second));
    assert_eq!(subtree_paths(&second), ["/mlc/deep/x"]);
    drop((first, second, subtree));
    mgr.full_set(&HashMap::new());
    assert_eq!(mgr.delegation_slots().len(), 0);
}

fn gateways(names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .map(|name| ("default".to_string(), name.to_string()))
        .collect()
}

/// Cached child units prebuild leaf metric handles for the process Gateway
/// set; a Gateway add/remove recompiles them and restores their slots, while
/// an unchanged set touches nothing.
#[test]
fn gateway_set_change_recompiles_cached_children_and_restores_slots() {
    let mgr = GlobalHttpRouteManagers::new();
    let p = parent("gsc", "gsc-child", 18610);
    let c = child("gsc-child", "gsc", "/gsc/a");
    mgr.full_set(&data(&[&p, &c]));
    let before_table = table(&mgr, 18610);
    let slot = mgr.delegation_slots().get_or_create(&key("gsc", "gsc-child"));

    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(gateways(&["gw1"]));
    }
    let first = slot.load().unwrap();
    let unit = &first.route_rules_list[0];
    assert!(unit.metric_handles.lookup("default", "gw1").is_some());
    assert_eq!(unit.matched_info.rsc.name, "gsc-child");
    assert!(unit.metric_handles.lookup("default", "gw2").is_none());

    // Unchanged set: nothing recompiled, the subtree Arc is kept.
    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(gateways(&["gw1"]));
    }
    assert!(Arc::ptr_eq(&first, &slot.load().unwrap()));

    // Gateway added: children recompiled for both, slot re-stored.
    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(gateways(&["gw1", "gw2"]));
    }
    let second = slot.load().unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(subtree_paths(&slot), ["/gsc/a"]);
    let unit = &second.route_rules_list[0];
    assert!(unit.metric_handles.lookup("default", "gw1").is_some());
    assert!(unit.metric_handles.lookup("default", "gw2").is_some());

    // Gateway removed.
    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(gateways(&["gw2"]));
    }
    let unit = &slot.load().unwrap().route_rules_list[0];
    assert!(unit.metric_handles.lookup("default", "gw1").is_none());
    assert!(unit.metric_handles.lookup("default", "gw2").is_some());

    assert!(
        Arc::ptr_eq(&before_table, &table(&mgr, 18610)),
        "a Gateway-set refresh never rebuilds listener tables"
    );
    assert_eq!(mgr.delegation_slots().len(), 1);
}

#[test]
fn gateway_set_refresh_is_noop_without_children() {
    let mgr = GlobalHttpRouteManagers::new();
    let route = ordinary("gsn", 18611);
    mgr.full_set(&data(&[&route]));
    let before = table(&mgr, 18611);
    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(gateways(&["gw1", "gw2"]));
    }
    mgr.refresh_delegation_gateways();
    assert!(Arc::ptr_eq(&before, &table(&mgr, 18611)));
    assert!(mgr.delegation_slots().is_empty());
}
