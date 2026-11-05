use super::*;
use edgion_resources::resources::grpc_route::GRPCRouteSpec;
use edgion_resources::resources::http_route::HTTPRouteSpec;

fn key(kind: RouteKindName, parent: &str, target: &str) -> DelegationKey {
    DelegationKey {
        kind,
        parent_ns: "ns".to_string(),
        parent_name: parent.to_string(),
        target_ns: "ns".to_string(),
        target_name: DelegationTargetName::Named(target.to_string()),
    }
}

fn pref(kind: Option<&str>, name: &str) -> ParentReference {
    ParentReference {
        group: None,
        kind: kind.map(str::to_string),
        namespace: None,
        name: name.to_string(),
        section_name: None,
        port: None,
    }
}

fn http_child(refs: Vec<ParentReference>) -> HTTPRoute {
    let spec: HTTPRouteSpec = serde_json::from_value(serde_json::json!({})).unwrap();
    let mut r = HTTPRoute::new("child", spec);
    r.metadata.namespace = Some("ns".to_string());
    r.spec.parent_refs = Some(refs);
    r
}

fn grpc_child(refs: Vec<ParentReference>) -> GRPCRoute {
    let spec: GRPCRouteSpec = serde_json::from_value(serde_json::json!({})).unwrap();
    let mut r = GRPCRoute::new("child", spec);
    r.metadata.namespace = Some("ns".to_string());
    r.spec.parent_refs = Some(refs);
    r
}

#[test]
fn get_or_create_is_idempotent() {
    let table: DelegationSlotTable<u32> = DelegationSlotTable::new();
    let k = key(RouteKindName::HTTPRoute, "p", "c");
    let a = table.get_or_create(&k);
    let b = table.get_or_create(&k);
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(table.len(), 1);
}

#[test]
fn slot_swap_keeps_old_arc_alive_for_readers() {
    let table: DelegationSlotTable<u32> = DelegationSlotTable::new();
    let slot = table.get_or_create(&key(RouteKindName::HTTPRoute, "p", "c"));
    slot.store(Some(Arc::new(1)));
    let old = slot.load().unwrap();
    slot.store(Some(Arc::new(2)));
    assert_eq!(*old, 1);
    assert_eq!(*slot.load().unwrap(), 2);
    slot.store(None);
    assert!(slot.load().is_none());
    assert_eq!(*old, 1);
}

#[test]
fn slot_rejects_key_not_in_child_parent_refs() {
    let child = http_child(vec![pref(Some("HTTPRoute"), "p1")]);
    let good = key(RouteKindName::HTTPRoute, "p1", "child");
    let wrong_parent = key(RouteKindName::HTTPRoute, "other", "child");
    let wrong_kind = key(RouteKindName::GRPCRoute, "p1", "child");
    let accepted = accept_child_keys(&child, &[good.clone(), wrong_parent, wrong_kind]);
    assert_eq!(accepted, vec![good]);

    let g = grpc_child(vec![pref(Some("GRPCRoute"), "p1")]);
    let gk = key(RouteKindName::GRPCRoute, "p1", "child");
    assert_eq!(accept_child_keys(&g, std::slice::from_ref(&gk)), vec![gk]);
}

#[test]
fn slot_rejects_key_whose_target_is_not_this_child() {
    let child = http_child(vec![pref(Some("HTTPRoute"), "p1")]);
    let named = key(RouteKindName::HTTPRoute, "p1", "child");
    let other_name = key(RouteKindName::HTTPRoute, "p1", "someone-else");
    let mut other_ns = named.clone();
    other_ns.target_ns = "elsewhere".to_string();
    let mut wildcard = named.clone();
    wildcard.target_name = DelegationTargetName::Wildcard;
    let mut wildcard_other_ns = wildcard.clone();
    wildcard_other_ns.target_ns = "elsewhere".to_string();
    let accepted = accept_child_keys(
        &child,
        &[named.clone(), other_name, other_ns, wildcard.clone(), wildcard_other_ns],
    );
    assert_eq!(accepted, vec![named, wildcard]);
}

#[test]
fn slot_rejects_keys_of_invalid_shape_child() {
    let k = key(RouteKindName::HTTPRoute, "p1", "child");
    // Mixed gateway + route refs.
    let mixed = http_child(vec![pref(Some("HTTPRoute"), "p1"), pref(None, "gw")]);
    assert!(accept_child_keys(&mixed, std::slice::from_ref(&k)).is_empty());
    // Root-shaped (gateway refs only).
    let root = http_child(vec![pref(None, "p1")]);
    assert!(accept_child_keys(&root, std::slice::from_ref(&k)).is_empty());
    // Too many parents.
    let many = http_child((0..4).map(|i| pref(Some("HTTPRoute"), &format!("p{i}"))).collect());
    assert!(accept_child_keys(&many, &[k]).is_empty());
}

#[test]
fn retain_referenced_is_key_scoped() {
    let table: DelegationSlotTable<u32> = DelegationSlotTable::new();
    let k1 = key(RouteKindName::HTTPRoute, "p", "a");
    let k2 = key(RouteKindName::HTTPRoute, "p", "b");
    let k3 = key(RouteKindName::HTTPRoute, "p", "c");
    for k in [&k1, &k2, &k3] {
        drop(table.get_or_create(k));
    }
    assert_eq!(table.len(), 3);
    table.retain_referenced(std::slice::from_ref(&k1));
    assert_eq!(table.len(), 2);

    // Held by a parent: kept. Non-empty subtree: kept.
    let held = table.get_or_create(&k2);
    let full = table.get_or_create(&k3);
    full.store(Some(Arc::new(7)));
    drop(full);
    table.retain_referenced(&[k2.clone(), k3.clone()]);
    assert_eq!(table.len(), 2);
    drop(held);
    table.retain_referenced(&[k2]);
    assert_eq!(table.len(), 1);
}

#[test]
fn empty_table_retain_is_noop() {
    let table: DelegationSlotTable<u32> = DelegationSlotTable::new();
    table.retain_referenced(&[]);
    table.retain_referenced(&[key(RouteKindName::HTTPRoute, "p", "c")]);
    assert!(table.is_empty());
}
