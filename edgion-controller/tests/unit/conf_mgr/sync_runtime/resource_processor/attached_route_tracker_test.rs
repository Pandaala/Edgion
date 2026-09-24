use super::*;

fn att(gw: &str, listener: &str) -> Attachment {
    Attachment {
        gateway_key: gw.to_string(),
        listener_name: listener.to_string(),
    }
}

#[test]
fn test_explicit_listener() {
    let t = AttachedRouteTracker::new();
    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "http")]),
    );

    assert_eq!(t.count_for_listener("default/gw", "http"), 1);
    assert_eq!(t.count_for_listener("default/gw", "https"), 0);
}

#[test]
fn test_wildcard_listener() {
    let t = AttachedRouteTracker::new();
    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "")]),
    );

    assert_eq!(t.count_for_listener("default/gw", "http"), 1);
    assert_eq!(t.count_for_listener("default/gw", "https"), 1);
    assert_eq!(t.count_for_listener("default/gw", ""), 1);
}

#[test]
fn test_mixed_explicit_and_wildcard() {
    let t = AttachedRouteTracker::new();

    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "http")]),
    );
    t.update_route(
        ResourceKind::TCPRoute,
        "default/t1",
        HashSet::from([att("default/gw", "")]),
    );

    assert_eq!(t.count_for_listener("default/gw", "http"), 2);
    assert_eq!(t.count_for_listener("default/gw", "https"), 1);
}

#[test]
fn test_update_replaces_old() {
    let t = AttachedRouteTracker::new();

    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "http")]),
    );
    assert_eq!(t.count_for_listener("default/gw", "http"), 1);

    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "https")]),
    );
    assert_eq!(t.count_for_listener("default/gw", "http"), 0);
    assert_eq!(t.count_for_listener("default/gw", "https"), 1);
}

#[test]
fn test_remove_route() {
    let t = AttachedRouteTracker::new();
    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "http")]),
    );
    assert_eq!(t.count_for_listener("default/gw", "http"), 1);

    t.remove_route(ResourceKind::HTTPRoute, "default/r1");
    assert_eq!(t.count_for_listener("default/gw", "http"), 0);
}

#[test]
fn test_multiple_routes_same_listener() {
    let t = AttachedRouteTracker::new();
    let a = HashSet::from([att("default/gw", "http")]);

    t.update_route(ResourceKind::HTTPRoute, "default/r1", a.clone());
    t.update_route(ResourceKind::HTTPRoute, "default/r2", a.clone());
    t.update_route(ResourceKind::GRPCRoute, "default/g1", a);

    assert_eq!(t.count_for_listener("default/gw", "http"), 3);
}

#[test]
fn test_route_arrives_before_gateway() {
    let t = AttachedRouteTracker::new();

    t.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        HashSet::from([att("default/gw", "http")]),
    );

    assert_eq!(t.count_for_listener("default/gw", "http"), 1);
}

#[test]
fn test_route_multi_gateway() {
    let t = AttachedRouteTracker::new();

    t.update_route(
        ResourceKind::HTTPRoute,
        "ns-a/r1",
        HashSet::from([att("ns-a/gw-a", "http"), att("ns-b/gw-b", "https")]),
    );

    assert_eq!(t.count_for_listener("ns-a/gw-a", "http"), 1);
    assert_eq!(t.count_for_listener("ns-b/gw-b", "https"), 1);
    assert_eq!(t.count_for_listener("ns-a/gw-a", "https"), 0);
}
