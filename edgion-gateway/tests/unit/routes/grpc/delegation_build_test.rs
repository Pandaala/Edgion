use super::*;
use crate::ctx::{EdgionHttpContext, NormalizedHostname};
use crate::routes::grpc::{DomainGrpcRouteRules, GrpcRouteMatchResult};
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::err::EdError;
use edgion_resources::resources::route_delegation::{DelegationTarget, DelegationTargetName};
use edgion_resources::{ListenerKey, ResourceMeta};
use pingora_proxy::Session;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------
// Fixtures: real GRPCRoutes compiled through the manager
// ---------------------------------------------------------------------------

fn listener(port: u16) -> ListenerKey {
    ListenerKey::new(format!("0.0.0.0:{port}").parse().unwrap(), "HTTP")
}

fn key(parent: &str, child: &str) -> DelegationKey {
    key_for(
        RouteKindName::GRPCRoute,
        "default",
        parent,
        &DelegationTarget {
            ns: "default".into(),
            name: DelegationTargetName::Named(child.into()),
        },
    )
}

fn delegate_to(target: &str) -> Value {
    json!([{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": target}])
}

fn svc() -> Value {
    json!([{"name": "svc", "port": 80}])
}

/// Service-level match (the only shape a delegating rule may use).
fn service(name: &str) -> Value {
    json!([{"method": {"service": name}}])
}

fn method(service: &str, method: &str) -> Value {
    json!([{"method": {"service": service, "method": method}}])
}

fn method_only(method: &str) -> Value {
    json!([{"method": {"method": method}}])
}

fn rule(matches: Value, backends: Value) -> Value {
    json!({"matches": matches, "backendRefs": backends})
}

/// Build a GRPCRoute. `gateway_port` makes it a root attached to `gw1`;
/// `route_parents` (with their Controller-confirmed keys) make it a child.
fn route(name: &str, gateway_port: Option<u16>, route_parents: &[&str], rules: Vec<Value>) -> Arc<GRPCRoute> {
    let mut parent_refs: Vec<Value> = route_parents
        .iter()
        .map(|p| json!({"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": p}))
        .collect();
    if gateway_port.is_some() {
        parent_refs.push(json!({"name": "gw1", "namespace": "default", "sectionName": "grpc"}));
    }
    let mut route: GRPCRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": parent_refs,
            "rules": rules,
        }
    }))
    .unwrap();
    if let Some(port) = gateway_port {
        route.spec.resolved_listeners = Some(vec![listener(port)]);
    }
    route.spec.resolved_delegation_keys = route_parents.iter().map(|p| key(p, name)).collect();
    route.preparse();
    route.parse_timeouts();
    route.parse_retry();
    Arc::new(route)
}

fn publish(port: u16, routes: &[&Arc<GRPCRoute>]) -> (GlobalGrpcRouteManagers, Arc<DomainGrpcRouteRules>) {
    let mgr = GlobalGrpcRouteManagers::new();
    let data: HashMap<_, _> = routes.iter().map(|r| (r.key_name(), Arc::clone(r))).collect();
    mgr.full_set(&data);
    let table = Arc::clone(&mgr.get_or_create_listener_manager(&listener(port)).load_route_table());
    (mgr, table)
}

fn index() -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(vec![(
        "default/gw1".to_string(),
        ListenerEntry {
            port: 8080,
            info: Arc::new(GatewayInfo::new(
                Some("default".into()),
                "gw1".into(),
                Some("grpc".into()),
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
            )),
            gateway_proof: None,
            hostname: None,
            allowed_routes: None,
        },
    )])
}

async fn run(
    table: &DomainGrpcRouteRules,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<GrpcRouteMatchResult, EdError> {
    let (mut client, server) = tokio::io::duplex(8192);
    let mut request = format!("POST {path} HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 0\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    client.write_all(request.as_bytes()).await.unwrap();
    drop(client);
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.normalized_current_path = path.to_string();
    ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
    table.match_route(&mut session, &ctx, &index(), "api.example.com")
}

/// `(leaf route name, leaf rule index, ancestor route names root first)`.
type Chain = (String, usize, Vec<String>);

fn chain(result: &GrpcRouteMatchResult) -> Chain {
    let ancestors = result
        .delegated
        .as_ref()
        .map(|d| {
            assert!(d.ancestors.is_empty(), "a GRPCRoute chain never fills HTTP ancestors");
            d.grpc_ancestors
                .iter()
                .map(|u| u.matched_info.rsc.name.clone())
                .collect()
        })
        .unwrap_or_default();
    (
        result.route_unit.matched_info.rsc.name.clone(),
        result.route_unit.matched_info.rule_id,
        ancestors,
    )
}

async fn matched(table: &DomainGrpcRouteRules, path: &str) -> Option<Chain> {
    matched_with(table, path, &[]).await
}

async fn matched_with(table: &DomainGrpcRouteRules, path: &str, headers: &[(&str, &str)]) -> Option<Chain> {
    match run(table, path, headers).await {
        Ok(result) => Some(chain(&result)),
        Err(EdError::RouteNotFound()) => None,
        Err(error) => panic!("unexpected route error: {error:?}"),
    }
}

fn hit(leaf: &str, rule: usize, ancestors: &[&str]) -> Option<Chain> {
    Some((
        leaf.to_string(),
        rule,
        ancestors.iter().map(|s| s.to_string()).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Cascading match (spec §9)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_child_without_service_matches_delegated_service() {
    let root = route(
        "ns-root",
        Some(19801),
        &[],
        vec![rule(service("svc.Users"), delegate_to("ns-child"))],
    );
    // Method-only matcher: no service, so it matches the delegated service.
    let child = route("ns-child", None, &["ns-root"], vec![rule(method_only("Get"), svc())]);
    let (_mgr, table) = publish(19801, &[&root, &child]);
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("ns-child", 0, &["ns-root"])
    );
    assert_eq!(matched(&table, "/svc.Users/List").await, None);
    assert_eq!(
        matched(&table, "/svc.Orders/Get").await,
        None,
        "the parent level fixes the service"
    );
}

#[tokio::test]
async fn grpc_child_other_service_never_matches() {
    let root = route(
        "os-root",
        Some(19802),
        &[],
        vec![rule(service("svc.Users"), delegate_to("os-child"))],
    );
    let child = route(
        "os-child",
        None,
        &["os-root"],
        vec![rule(method("svc.Orders", "Get"), svc())],
    );
    let (_mgr, table) = publish(19802, &[&root, &child]);
    assert_eq!(matched(&table, "/svc.Users/Get").await, None);
    assert_eq!(matched(&table, "/svc.Orders/Get").await, None);
}

#[tokio::test]
async fn grpc_matchless_child_is_catch_all_inside_scope_only() {
    let root = route(
        "ml-root",
        Some(19803),
        &[],
        vec![rule(service("svc.Users"), delegate_to("ml-child"))],
    );
    let child = route("ml-child", None, &["ml-root"], vec![json!({"backendRefs": svc()})]);
    let (_mgr, table) = publish(19803, &[&root, &child]);
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("ml-child", 0, &["ml-root"])
    );
    assert_eq!(
        matched(&table, "/svc.Users/Anything").await,
        hit("ml-child", 0, &["ml-root"])
    );
    // Another service on the same host never reaches the catch-all child.
    assert_eq!(matched(&table, "/svc.Orders/Get").await, None);
}

#[tokio::test]
async fn grpc_backtrack_on_subtree_miss() {
    let root = route(
        "bt-root",
        Some(19804),
        &[],
        vec![rule(service("svc.Users"), delegate_to("bt-child"))],
    );
    let sibling = route("bt-sibling", Some(19804), &[], vec![rule(method_only("List"), svc())]);
    let child = route("bt-child", None, &["bt-root"], vec![rule(method_only("Get"), svc())]);
    let (_mgr, table) = publish(19804, &[&root, &sibling, &child]);
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("bt-child", 0, &["bt-root"])
    );
    assert_eq!(
        matched(&table, "/svc.Users/List").await,
        hit("bt-sibling", 0, &[]),
        "a subtree miss falls through to the next root candidate"
    );
}

#[tokio::test]
async fn grpc_per_level_precedence() {
    // Root A delegates svc.Users; its child has the more specific
    // service+method. Root B's svc.Users/Get outranks A at the root level.
    let a = route(
        "pl-a",
        Some(19805),
        &[],
        vec![rule(service("svc.Users"), delegate_to("pl-child"))],
    );
    let child = route(
        "pl-child",
        None,
        &["pl-a"],
        vec![
            rule(method("svc.Users", "Get"), svc()),
            rule(method_only("List"), svc()),
        ],
    );
    let b = route("pl-b", Some(19805), &[], vec![rule(method("svc.Users", "Get"), svc())]);
    let (_mgr, table) = publish(19805, &[&a, &child, &b]);
    assert_eq!(matched(&table, "/svc.Users/Get").await, hit("pl-b", 0, &[]));
    assert_eq!(matched(&table, "/svc.Users/List").await, hit("pl-child", 1, &["pl-a"]));
}

#[tokio::test]
async fn grpc_child_precedence_is_its_own_level() {
    // Inside one subtree, service+method beats method-only regardless of order.
    let root = route(
        "cp-root",
        Some(19806),
        &[],
        vec![rule(service("svc.Users"), delegate_to("cp-child"))],
    );
    let child = route(
        "cp-child",
        None,
        &["cp-root"],
        vec![rule(method_only("Get"), svc()), rule(method("svc.Users", "Get"), svc())],
    );
    let (_mgr, table) = publish(19806, &[&root, &child]);
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("cp-child", 1, &["cp-root"])
    );
}

/// Root `lv1` plus children `lv2..=lvN`; every level delegates svc.Users to the next.
fn ladder(prefix_name: &str, port: u16, levels: usize) -> Vec<Arc<GRPCRoute>> {
    let name = |i: usize| format!("{prefix_name}{i}");
    let mut routes = vec![route(
        &name(1),
        Some(port),
        &[],
        vec![rule(service("svc.Users"), delegate_to(&name(2)))],
    )];
    for i in 2..=levels {
        let parent = name(i - 1);
        let backends = if i == levels { svc() } else { delegate_to(&name(i + 1)) };
        routes.push(route(
            &name(i),
            None,
            &[parent.as_str()],
            vec![rule(service("svc.Users"), backends)],
        ));
    }
    routes
}

#[tokio::test]
async fn grpc_five_levels() {
    let five = ladder("gfive", 19807, 5);
    let (_mgr5, table5) = publish(19807, &five.iter().collect::<Vec<_>>());
    assert_eq!(
        matched(&table5, "/svc.Users/Get").await,
        hit("gfive5", 0, &["gfive1", "gfive2", "gfive3", "gfive4"])
    );

    let six = ladder("gsix", 19808, 6);
    let (_mgr6, table6) = publish(19808, &six.iter().collect::<Vec<_>>());
    assert_eq!(matched(&table6, "/svc.Users/Get").await, None);
}

#[tokio::test]
async fn grpc_cycle_terminates_on_revisited_slot() {
    // Trace: root -> slot(root,B) [chain 2] -> slot(B,C) [3] -> slot(C,B) [4]
    // -> B's rule targets slot(B,C) again, which this request already
    // entered, so the visited set refuses it before the depth cap (5) is
    // reached. A longer acyclic ladder is stopped by the depth cap instead
    // (`grpc_five_levels`).
    let root = route(
        "cy-root",
        Some(19809),
        &[],
        vec![rule(service("svc.Users"), delegate_to("cy-b"))],
    );
    let b = route(
        "cy-b",
        None,
        &["cy-root", "cy-c"],
        vec![rule(service("svc.Users"), delegate_to("cy-c"))],
    );
    let c = route(
        "cy-c",
        None,
        &["cy-b"],
        vec![rule(service("svc.Users"), delegate_to("cy-b"))],
    );
    let (_mgr, table) = publish(19809, &[&root, &b, &c]);
    assert_eq!(matched(&table, "/svc.Users/Get").await, None);
}

#[tokio::test]
async fn grpc_child_checks_its_own_headers_and_mask() {
    let modifier =
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}]);
    let root = route(
        "hm-root",
        Some(19810),
        &[],
        vec![json!({"matches": service("svc.Users"), "filters": modifier, "backendRefs": delegate_to("hm-child")})],
    );
    let child = route(
        "hm-child",
        None,
        &["hm-root"],
        vec![
            // Conflicts with the parent's RequestHeaderModifier: skipped.
            json!({"matches": method_only("Get"), "filters": modifier, "backendRefs": svc()}),
            json!({"matches": [{"headers": [{"name": "x-tenant", "value": "t1"}]}], "backendRefs": svc()}),
        ],
    );
    let (_mgr, table) = publish(19810, &[&root, &child]);
    assert_eq!(
        matched_with(&table, "/svc.Users/Get", &[("x-tenant", "t1")]).await,
        hit("hm-child", 1, &["hm-root"])
    );
    assert_eq!(matched(&table, "/svc.Users/Get").await, None);
}

/// Regression guard: a backendRef-level filter of the parent's mandatory type
/// conflicts like a rule-level one, so the child candidate is skipped.
#[tokio::test]
async fn grpc_backend_ref_filter_mask_conflict_skips_child_candidate() {
    let modifier = json!([{"type": "ResponseHeaderModifier",
        "responseHeaderModifier": {"set": [{"name": "x-parent", "value": "1"}]}}]);
    let root = route(
        "bm-root",
        Some(19840),
        &[],
        vec![json!({"matches": service("svc.Users"), "filters": modifier, "backendRefs": delegate_to("bm-child")})],
    );
    let smuggled = json!([{"name": "svc", "port": 80, "filters": [{"type": "ResponseHeaderModifier",
        "responseHeaderModifier": {"remove": ["x-parent"]}}]}]);
    let child = route(
        "bm-child",
        None,
        &["bm-root"],
        vec![
            json!({"matches": method_only("Get"), "backendRefs": smuggled}),
            json!({"backendRefs": svc()}),
        ],
    );
    let (_mgr, table) = publish(19840, &[&root, &child]);
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("bm-child", 1, &["bm-root"])
    );
}

#[tokio::test]
async fn grpc_ordinary_unchanged() {
    let plain = route(
        "or-plain",
        Some(19811),
        &[],
        vec![rule(method("svc.Plain", "Get"), svc())],
    );
    let (mgr, table) = publish(19811, &[&plain]);
    let result = run(&table, "/svc.Plain/Get", &[]).await.unwrap();
    assert!(result.delegated.is_none(), "an ordinary match carries no chain");
    assert_eq!(chain(&result), ("or-plain".to_string(), 0, vec![]));
    let unit = &result.route_unit;
    assert!(unit.delegate.is_none());
    assert_eq!(unit.filter_mask, 0);
    assert!(mgr.delegation_slots().is_empty(), "ordinary routes create no slot");
    assert!(mgr.delegation_index.lock().children.is_empty());
    assert!(matches!(
        run(&table, "/svc.Plain/Nope", &[]).await,
        Err(EdError::RouteNotFound())
    ));
}

// ---------------------------------------------------------------------------
// Publication routing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_delegating_unit_holds_slot_and_compiles_no_backends() {
    let root = route(
        "ds-root",
        Some(19812),
        &[],
        vec![rule(service("svc.Users"), delegate_to("ds-child"))],
    );
    let (mgr, _table) = publish(19812, &[&root]);
    let units = mgr.route_units_cache.lock()[&root.key_name()].clone();
    assert_eq!(units.len(), 1);
    let slot = units[0].delegate.as_ref().expect("delegating unit holds its slot");
    assert!(Arc::ptr_eq(
        slot,
        &mgr.delegation_slots().get_or_create(&key("ds-root", "ds-child"))
    ));
    assert!(units[0].backends.is_empty());
    assert!(slot.load().is_none(), "no child yet");
}

#[tokio::test]
async fn grpc_child_change_rebuilds_only_its_slots_not_listener() {
    let root = route(
        "cc-root",
        Some(19813),
        &[],
        vec![
            rule(service("svc.Users"), delegate_to("cc-child")),
            rule(service("svc.Orders"), delegate_to("cc-other")),
        ],
    );
    let child = route("cc-child", None, &["cc-root"], vec![rule(method_only("Get"), svc())]);
    let other = route("cc-other", None, &["cc-root"], vec![rule(method_only("Get"), svc())]);
    let (mgr, table) = publish(19813, &[&root, &child, &other]);
    let manager = mgr.get_or_create_listener_manager(&listener(19813));
    let other_subtree = mgr
        .delegation_slots()
        .get_or_create(&key("cc-root", "cc-other"))
        .load()
        .unwrap();

    let updated = route("cc-child", None, &["cc-root"], vec![rule(method_only("List"), svc())]);
    mgr.partial_update(
        HashMap::new(),
        HashMap::from([(updated.key_name(), Arc::clone(&updated))]),
        HashSet::new(),
    );

    assert!(
        Arc::ptr_eq(&table, &manager.load_route_table()),
        "a child-only change never rebuilds a listener table"
    );
    assert!(Arc::ptr_eq(
        &other_subtree,
        &mgr.delegation_slots()
            .get_or_create(&key("cc-root", "cc-other"))
            .load()
            .unwrap()
    ));
    assert_eq!(matched(&table, "/svc.Users/Get").await, None);
    assert_eq!(
        matched(&table, "/svc.Users/List").await,
        hit("cc-child", 0, &["cc-root"])
    );
}

#[tokio::test]
async fn grpc_child_before_parent_then_parent_arrives() {
    let child = route("cb-child", None, &["cb-root"], vec![rule(method_only("Get"), svc())]);
    let (mgr, _) = publish(19814, &[&child]);
    let slot = mgr.delegation_slots().get_or_create(&key("cb-root", "cb-child"));
    assert!(slot.load().is_some(), "the child side creates and fills its slot");

    let root = route(
        "cb-root",
        Some(19814),
        &[],
        vec![rule(service("svc.Users"), delegate_to("cb-child"))],
    );
    mgr.partial_update(
        HashMap::from([(root.key_name(), Arc::clone(&root))]),
        HashMap::new(),
        HashSet::new(),
    );
    let table = Arc::clone(&mgr.get_or_create_listener_manager(&listener(19814)).load_route_table());
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("cb-child", 0, &["cb-root"])
    );
}

#[tokio::test]
async fn grpc_parent_deleted_slot_collected_when_unreferenced() {
    let root = route(
        "pd-root",
        Some(19815),
        &[],
        vec![rule(service("svc.Users"), delegate_to("pd-child"))],
    );
    let (mgr, _table) = publish(19815, &[&root]);
    drop(_table);
    assert_eq!(mgr.delegation_slots().len(), 1);
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([root.key_name()]));
    drop(root);
    // The listener table no longer pins the parent units; GC is key-scoped.
    mgr.collect_delegation_slots(&[key("pd-root", "pd-child")]);
    assert_eq!(mgr.delegation_slots().len(), 0);
}

#[test]
fn grpc_children_not_bucketed_by_listener() {
    let child = route("nb-child", None, &["nb-root"], vec![rule(method_only("Get"), svc())]);
    assert!(is_grpc_delegation_child(&child));
    let mgr = GlobalGrpcRouteManagers::new();
    mgr.full_set(&HashMap::from([(child.key_name(), Arc::clone(&child))]));
    assert!(mgr.bucket_units_by_listener().is_empty());
    assert!(
        mgr.route_units_cache.lock()[&child.key_name()].is_empty(),
        "children are compiled into slots, not listener units"
    );
    assert!(mgr.delegation_index.lock().children.contains_key(&child.key_name()));
}

#[test]
fn grpc_child_keys_require_own_parent_ref() {
    let mut child = (*route("ck-child", None, &["ck-root"], vec![rule(method_only("Get"), svc())])).clone();
    child.spec.resolved_delegation_keys.push(key("ck-stranger", "ck-child"));
    assert_eq!(grpc_child_accepted_keys(&child), vec![key("ck-root", "ck-child")]);
    let plain = route("ck-plain", Some(19816), &[], vec![rule(method_only("Get"), svc())]);
    assert!(grpc_child_accepted_keys(&plain).is_empty());
}

#[test]
fn grpc_from_grpc_flags_accepted_children_for_every_scope() {
    use crate::plugins::publication::inputs::RouteInput;
    let child = route("pub-child", None, &["pub-root"], vec![rule(method_only("Get"), svc())]);
    let middle = route(
        "pub-mid",
        None,
        &["pub-root"],
        vec![rule(service("svc.Users"), delegate_to("pub-leaf"))],
    );
    let plain = route("pub-plain", Some(19817), &[], vec![rule(method_only("Get"), svc())]);
    assert!(RouteInput::from_grpc(&child).delegation_child);
    assert!(
        RouteInput::from_grpc(&middle).delegation_child,
        "middle levels are children too"
    );
    assert!(!RouteInput::from_grpc(&plain).delegation_child);
    let mut withdrawn = (*child).clone();
    withdrawn.spec.resolved_delegation_keys.clear();
    assert!(!RouteInput::from_grpc(&withdrawn).delegation_child);
}

#[test]
fn grpc_child_metric_handles_cover_process_gateways() {
    let child = route("mh-child", None, &["mh-root"], vec![rule(method_only("Get"), svc())]);
    let gateways = vec![
        ("default".to_string(), "gw1".to_string()),
        ("default".to_string(), "gw2".to_string()),
    ];
    let units = compile_child_units(&child, &DelegationSlotTable::new(), &gateways);
    assert_eq!(units.len(), 1);
    assert!(
        units[0].route_info.parent_refs.is_none(),
        "children carry no Gateway eligibility"
    );
    for (ns, name) in &gateways {
        assert!(units[0].metric_handles.lookup(ns, name).is_some());
    }
    assert!(units[0].metric_handles.lookup("default", "gw3").is_none());
}

#[test]
fn grpc_gateway_set_change_recompiles_cached_children() {
    let child = route("gs-child", None, &["gs-root"], vec![rule(method_only("Get"), svc())]);
    let mgr = GlobalGrpcRouteManagers::new();
    mgr.full_set(&HashMap::from([(child.key_name(), Arc::clone(&child))]));
    let slot = mgr.delegation_slots().get_or_create(&key("gs-root", "gs-child"));
    let before = slot.load().unwrap();
    {
        let _guard = mgr.lock_rebuild();
        // Bind first: a temporary guard would live to the end of the statement and
        // deadlock against the non-reentrant lock taken inside the refresh.
        let current = mgr.delegation_index.lock().gateways.clone();
        mgr.refresh_delegation_gateways_locked(current);
    }
    assert!(
        Arc::ptr_eq(&before, &slot.load().unwrap()),
        "unchanged set keeps the subtree"
    );
    {
        let _guard = mgr.lock_rebuild();
        mgr.refresh_delegation_gateways_locked(vec![("default".into(), "gw-new".into())]);
    }
    let after = slot.load().unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    let index = mgr.delegation_index.lock();
    let unit = &index.children[&child.key_name()].units[0];
    assert!(
        unit.metric_handles.lookup("default", "gw-new").is_some(),
        "recompiled child prebuilds handles for the new Gateway"
    );
    assert!(unit.metric_handles.lookup("default", "gw1").is_none());
}

#[tokio::test]
async fn grpc_visited_slot_entered_once() {
    // Two rules of one parent delegate to the same target (one slot). The
    // header-specific rule ranks first, enters the slot and misses there (its
    // RequestHeaderModifier masks the child's only rule). The second rule
    // points at the already-visited slot and is skipped: first entry wins.
    let modifier =
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}]);
    let parent = route(
        "vs-p",
        Some(19820),
        &[],
        vec![
            json!({"matches": [{"method": {"service": "svc.Users"}, "headers": [{"name": "x-v1", "value": "1"}]}],
                "filters": modifier, "backendRefs": delegate_to("vs-child")}),
            rule(service("svc.Users"), delegate_to("vs-child")),
        ],
    );
    let child = route(
        "vs-child",
        None,
        &["vs-p"],
        vec![json!({"matches": method_only("Get"), "filters": modifier, "backendRefs": svc()})],
    );
    let (mgr, table) = publish(19820, &[&parent, &child]);
    assert_eq!(mgr.delegation_slots().len(), 1, "both rules share one slot");
    assert_eq!(
        matched(&table, "/svc.Users/Get").await,
        hit("vs-child", 0, &["vs-p"]),
        "control: through the second rule alone the child matches"
    );
    assert_eq!(
        matched_with(&table, "/svc.Users/Get", &[("x-v1", "1")]).await,
        None,
        "the second entry into the visited slot is skipped"
    );
}

// ---------------------------------------------------------------------------
// Chain values and observability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_inherited_timeouts_retry_and_session_persistence() {
    let root = route(
        "ih-root",
        Some(19818),
        &[],
        vec![json!({
            "matches": service("svc.Users"),
            "backendRefs": delegate_to("ih-child"),
            "timeouts": {"request": "7s", "backendRequest": "3s"},
            "retry": {"attempts": 4},
            "sessionPersistence": {"sessionName": "root-sp", "type": "Header"}
        })],
    );
    let child = route(
        "ih-child",
        None,
        &["ih-root"],
        vec![json!({
            "matches": method_only("Get"),
            "backendRefs": svc(),
            "timeouts": {"request": "5s"}
        })],
    );
    let (_mgr, table) = publish(19818, &[&root, &child]);
    let result = run(&table, "/svc.Users/Get", &[]).await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.grpc_route_unit = Some(result.route_unit);
    ctx.delegated = result.delegated;
    assert_eq!(ctx.request_timeout(), Some(Duration::from_secs(5)), "leaf wins");
    assert_eq!(
        ctx.route_backend_request_timeout().and_then(|t| t.as_duration()),
        Some(Duration::from_secs(3)),
        "unset leaf sub-field falls back to the ancestor"
    );
    assert_eq!(ctx.retry_attempts(), Some(4));
    assert!(ctx.retry_configured());
    assert_eq!(
        ctx.effective_session_persistence()
            .and_then(|sp| sp.session_name.as_deref()),
        Some("root-sp")
    );
    assert_eq!(ctx.ancestor_count(), 1);
    assert!(ctx.ancestor_runtime(0).is_some());
}
