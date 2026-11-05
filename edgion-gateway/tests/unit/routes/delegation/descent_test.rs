use super::*;
use crate::ctx::{EdgionHttpContext, NormalizedHostname};
use crate::routes::http::match_unit::RouteMatchResult;
use crate::routes::http::routes_mgr::{DomainRouteRules, GlobalHttpRouteManagers};
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::route_delegation::{
    key_for, DelegationKey, DelegationTarget, DelegationTargetName, RouteKindName,
};
use edgion_resources::{HTTPRoute, ListenerKey, ResourceMeta};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tokio::io::AsyncWriteExt;

/// Test-only views of the private descent bookkeeping.
trait DescentStateProbe {
    /// Successful slot entries made by this request so far (each one is
    /// exactly one subtree match).
    fn entries(&self) -> usize;
    fn depth(&self) -> usize;
    fn chain_mask(&self) -> u32;
    /// Enter a slot through the production path, discarding the subtree.
    fn enter<S>(&mut self, slot: &Arc<DelegationSlot<S>>, unit_mask: u32) -> bool;
}

impl DescentStateProbe for DescentState {
    fn entries(&self) -> usize {
        self.visited.len()
    }
    fn depth(&self) -> usize {
        self.depth as usize
    }
    fn chain_mask(&self) -> u32 {
        self.chain_mask
    }
    fn enter<S>(&mut self, slot: &Arc<DelegationSlot<S>>, unit_mask: u32) -> bool {
        self.enter_loaded(slot, unit_mask).is_some()
    }
}

// ---------------------------------------------------------------------------
// DescentState (pure)
// ---------------------------------------------------------------------------

fn slot() -> Arc<DelegationSlot<()>> {
    crate::routes::delegation::DelegationSlotTable::<()>::new().get_or_create(&key("a", "b"))
}

#[test]
fn descent_state_enter_marks_visited_and_grows_chain() {
    let (a, b) = (loaded_slot(), loaded_slot());
    let mut state = DescentState::new();
    assert_eq!(state.depth(), 1);
    assert!(state.enter(&a, 0b01));
    assert_eq!((state.depth(), state.chain_mask()), (2, 0b01));
    assert!(!state.enter(&a, 0), "a visited slot is never entered again");
    assert!(!state.enter(&b, 0b01), "a mask conflict refuses the descent");
    assert!(state.enter(&b, 0b10));
    assert_eq!((state.depth(), state.chain_mask()), (3, 0b11));
}

#[test]
fn descent_state_leave_restores_chain_but_keeps_visited() {
    let a = loaded_slot();
    let mut state = DescentState::new();
    let mark = state.mark();
    assert!(state.enter(&a, 0b100));
    state.leave(mark);
    assert_eq!((state.depth(), state.chain_mask()), (1, 0));
    assert!(!state.enter(&a, 0), "first entry wins for the whole request");
}

#[test]
fn descent_state_depth_caps_at_max_levels() {
    let mut state = DescentState::new();
    let slots: Vec<_> = (0..MAX_ROUTE_DELEGATION_LEVELS).map(|_| loaded_slot()).collect();
    for s in &slots[..MAX_ROUTE_DELEGATION_LEVELS - 1] {
        assert!(state.enter(s, 0));
    }
    assert_eq!(state.depth(), MAX_ROUTE_DELEGATION_LEVELS);
    assert!(!state.enter(&slots[MAX_ROUTE_DELEGATION_LEVELS - 1], 0));
}

fn loaded_slot() -> Arc<DelegationSlot<()>> {
    let s = slot();
    s.store(Some(Arc::new(())));
    s
}

#[test]
fn descent_state_entry_budget_stops_descent_after_cap() {
    let slots: Vec<_> = (0..=MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST)
        .map(|_| loaded_slot())
        .collect();
    let mut state = DescentState::new();
    for s in &slots[..MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST] {
        // Backtrack after each entry so only the budget, not the depth, binds.
        let mark = state.mark();
        assert!(state.enter_loaded(s, 0).is_some());
        state.leave(mark);
    }
    assert_eq!(state.entries(), MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST);
    let last = &slots[MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST];
    assert!(
        state.enter_loaded(last, 0).is_none(),
        "budget exhausted: a fresh slot is a miss"
    );
    assert_eq!(state.depth(), 1);
}

#[test]
fn descent_state_empty_slot_is_a_miss_without_budget_or_visit() {
    let empty = slot();
    let mut state = DescentState::new();
    for _ in 0..(MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST * 2) {
        assert!(state.enter_loaded(&empty, 0b1).is_none());
    }
    assert_eq!(state.entries(), 0, "empty slots consume no budget");
    assert_eq!(
        (state.depth(), state.chain_mask()),
        (1, 0),
        "an empty slot does not grow the chain"
    );
    // Not marked visited: once loaded, the same slot is entered normally.
    empty.store(Some(Arc::new(())));
    assert!(state.enter_loaded(&empty, 0b1).is_some());
    assert_eq!((state.entries(), state.depth(), state.chain_mask()), (1, 2, 0b1));
    assert!(state.enter_loaded(&empty, 0).is_none(), "first entry still wins");
}

#[test]
fn descend_with_skips_empty_slot_and_exhausted_budget() {
    let unit = Arc::new(());
    let mut state = DescentState::new();
    let empty = slot();
    let miss = descend_with::<(), ()>(&unit, 0, &empty, &mut state, |_, _| {
        panic!("empty slot must not be matched")
    });
    assert!(miss.unwrap().is_none());
    assert_eq!(state.entries(), 0);

    // Keep every slot alive so no address is reused across iterations.
    let slots: Vec<_> = (0..MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST)
        .map(|_| loaded_slot())
        .collect();
    let mut subtree_matches = 0;
    for s in &slots {
        let miss = descend_with::<(), ()>(&unit, 0, s, &mut state, |_, _| {
            subtree_matches += 1;
            Ok(None)
        });
        assert!(miss.unwrap().is_none());
    }
    assert_eq!(state.entries(), MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST);
    assert_eq!(subtree_matches, MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST);
    let s = loaded_slot();
    let miss = descend_with::<(), ()>(&unit, 0, &s, &mut state, |_, _| panic!("budget exhausted"));
    assert!(miss.unwrap().is_none());
}

/// The slot is swapped while the subtree match is running: the descent keeps
/// matching (and returns a leaf from) the generation it loaded on entry.
#[test]
fn descend_with_matches_loaded_generation_when_slot_swaps_mid_match() {
    let unit = Arc::new(());
    let s: Arc<DelegationSlot<u32>> =
        crate::routes::delegation::DelegationSlotTable::<u32>::new().get_or_create(&key("a", "b"));
    s.store(Some(Arc::new(1)));
    let mut state = DescentState::new();
    let hit = descend_with::<(), u32>(&unit, 0, &s, &mut state, |subtree, _| {
        s.store(Some(Arc::new(2)));
        assert_eq!(*s.load().unwrap(), 2, "the slot now publishes the new generation");
        assert_eq!(*subtree, 1, "the running match keeps the loaded generation");
        Ok(Some(ChildHit {
            leaf: Arc::new(()),
            ancestors: SmallVec::new(),
        }))
    })
    .unwrap()
    .expect("the old generation's hit is returned");
    assert_eq!(hit.ancestors.len(), 1);
    assert!(Arc::ptr_eq(&hit.ancestors[0], &unit));
}

// ---------------------------------------------------------------------------
// Fixtures: real routes compiled through the manager
// ---------------------------------------------------------------------------

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

fn delegate_to(target: &str) -> Value {
    json!([{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": target}])
}

fn svc() -> Value {
    json!([{"name": "svc", "port": 80}])
}

fn prefix(path: &str) -> Value {
    json!([{"path": {"type": "PathPrefix", "value": path}}])
}

fn exact(path: &str) -> Value {
    json!([{"path": {"type": "Exact", "value": path}}])
}

fn rule(matches: Value, backends: Value) -> Value {
    json!({"matches": matches, "backendRefs": backends})
}

/// Build a route. `gateway_port` makes it a root attached to `gw1`;
/// `route_parents` (with their Controller-confirmed keys) make it a child.
fn route(name: &str, gateway_port: Option<u16>, route_parents: &[&str], rules: Vec<Value>) -> Arc<HTTPRoute> {
    let mut parent_refs: Vec<Value> = route_parents
        .iter()
        .map(|p| json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": p}))
        .collect();
    if gateway_port.is_some() {
        parent_refs.push(
            json!({"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"}),
        );
    }
    let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
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
    route.pre_parse();
    Arc::new(route)
}

fn publish(port: u16, routes: &[&Arc<HTTPRoute>]) -> (GlobalHttpRouteManagers, Arc<DomainRouteRules>) {
    let mgr = GlobalHttpRouteManagers::new();
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
                Some("web".into()),
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
    table: &DomainRouteRules,
    path: &str,
    headers: &[(&str, &str)],
) -> (Result<RouteMatchResult, EdError>, Option<DescentState>) {
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(server));
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: api.example.com\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    client.write_all(request.as_bytes()).await.unwrap();
    drop(client);
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
    ctx.request_info.entry_raw_path = path.to_string();
    ctx.request_info.normalized_current_path = path.to_string();
    let mut state = None;
    let result = table.match_route_with_descent(&mut session, &ctx, &index(), &mut state);
    (result, state)
}

/// `(leaf route name, leaf rule index, ancestor route names root first)`.
fn chain(result: &RouteMatchResult) -> (String, usize, Vec<String>) {
    let ancestors = result
        .delegated
        .as_ref()
        .map(|d| d.ancestors.iter().map(|u| u.matched_info.rsc.name.clone()).collect())
        .unwrap_or_default();
    (
        result.route_unit.matched_info.rsc.name.clone(),
        result.route_unit.matched_info.rule_id,
        ancestors,
    )
}

async fn matched(table: &DomainRouteRules, path: &str) -> Option<(String, usize, Vec<String>)> {
    match run(table, path, &[]).await.0 {
        Ok(result) => Some(chain(&result)),
        Err(EdError::RouteNotFound()) => None,
        Err(error) => panic!("unexpected route error: {error:?}"),
    }
}

fn hit(leaf: &str, rule: usize, ancestors: &[&str]) -> Option<(String, usize, Vec<String>)> {
    Some((
        leaf.to_string(),
        rule,
        ancestors.iter().map(|s| s.to_string()).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Cascading match
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_level_precedence_root_b_beats_child_exact() {
    let a = route(
        "pl-a",
        Some(18701),
        &[],
        vec![rule(prefix("/api"), delegate_to("pl-child"))],
    );
    let child = route("pl-child", None, &["pl-a"], vec![rule(exact("/api/users"), svc())]);
    let b = route("pl-b", Some(18701), &[], vec![rule(prefix("/api/users"), svc())]);
    let (_mgr, table) = publish(18701, &[&a, &child, &b]);
    assert_eq!(matched(&table, "/api/users").await, hit("pl-b", 0, &[]));
    assert_eq!(
        matched(&table, "/api/other").await,
        None,
        "A wins at the root for /api/other but its child has no match"
    );
}

#[tokio::test]
async fn backtrack_to_ordinary_sibling_on_subtree_miss() {
    let a = route(
        "bt-a",
        Some(18702),
        &[],
        vec![rule(prefix("/api"), delegate_to("bt-child")), rule(prefix("/"), svc())],
    );
    let child = route("bt-child", None, &["bt-a"], vec![rule(prefix("/api/users"), svc())]);
    let (_mgr, table) = publish(18702, &[&a, &child]);
    assert_eq!(matched(&table, "/api/users/1").await, hit("bt-child", 0, &["bt-a"]));
    assert_eq!(matched(&table, "/api/orders").await, hit("bt-a", 1, &[]));
}

#[tokio::test]
async fn child_path_disjoint_from_parent_never_matches() {
    let a = route(
        "dj-a",
        Some(18703),
        &[],
        vec![rule(prefix("/api"), delegate_to("dj-child"))],
    );
    let child = route("dj-child", None, &["dj-a"], vec![rule(prefix("/admin"), svc())]);
    let (_mgr, table) = publish(18703, &[&a, &child]);
    assert_eq!(matched(&table, "/admin").await, None);
    assert_eq!(matched(&table, "/api/admin").await, None);
}

#[tokio::test]
async fn child_without_path_covers_whole_scope() {
    let a = route(
        "np-a",
        Some(18704),
        &[],
        vec![rule(prefix("/api"), delegate_to("np-child"))],
    );
    let child = route("np-child", None, &["np-a"], vec![json!({"backendRefs": svc()})]);
    let (_mgr, table) = publish(18704, &[&a, &child]);
    assert_eq!(
        matched(&table, "/api/anything/deep").await,
        hit("np-child", 0, &["np-a"])
    );
    assert_eq!(matched(&table, "/other").await, None, "scope is the parent prefix");
}

#[tokio::test]
async fn child_regex_intersects_parent_prefix() {
    let a = route(
        "rx-a",
        Some(18705),
        &[],
        vec![rule(prefix("/api"), delegate_to("rx-child"))],
    );
    let child = route(
        "rx-child",
        None,
        &["rx-a"],
        vec![rule(
            json!([{"path": {"type": "RegularExpression", "value": "^/api/?secret$"}}]),
            svc(),
        )],
    );
    let (_mgr, table) = publish(18705, &[&a, &child]);
    assert_eq!(matched(&table, "/api/secret").await, hit("rx-child", 0, &["rx-a"]));
    assert_eq!(matched(&table, "/apisecret").await, None);
}

/// Root `lv1` plus children `lv2..=lvN`; every level delegates `/api` to the next.
fn ladder(prefix_name: &str, port: u16, levels: usize) -> Vec<Arc<HTTPRoute>> {
    let name = |i: usize| format!("{prefix_name}{i}");
    let mut routes = vec![route(
        &name(1),
        Some(port),
        &[],
        vec![rule(prefix("/api"), delegate_to(&name(2)))],
    )];
    for i in 2..=levels {
        let parent = name(i - 1);
        let backends = if i == levels { svc() } else { delegate_to(&name(i + 1)) };
        routes.push(route(
            &name(i),
            None,
            &[parent.as_str()],
            vec![rule(prefix("/api"), backends)],
        ));
    }
    routes
}

#[tokio::test]
async fn five_levels_match_sixth_does_not() {
    let five = ladder("five", 18706, 5);
    let (_mgr5, table5) = publish(18706, &five.iter().collect::<Vec<_>>());
    assert_eq!(
        matched(&table5, "/api/x").await,
        hit("five5", 0, &["five1", "five2", "five3", "five4"])
    );

    let six = ladder("six", 18707, 6);
    let (_mgr6, table6) = publish(18707, &six.iter().collect::<Vec<_>>());
    assert_eq!(matched(&table6, "/api/x").await, None);
}

#[tokio::test]
async fn cycle_terminates() {
    let root = route(
        "cy-root",
        Some(18708),
        &[],
        vec![rule(prefix("/api"), delegate_to("cy-b"))],
    );
    let b = route(
        "cy-b",
        None,
        &["cy-root", "cy-c"],
        vec![rule(prefix("/api"), delegate_to("cy-c"))],
    );
    let c = route("cy-c", None, &["cy-b"], vec![rule(prefix("/api"), delegate_to("cy-b"))]);
    let (_mgr, table) = publish(18708, &[&root, &b, &c]);
    let (result, state) = run(&table, "/api/x", &[]).await;
    assert!(matches!(result, Err(EdError::RouteNotFound())));
    // root->B, B->C, C->B are distinct slots; B->C is then visited.
    assert_eq!(state.expect("descent ran").entries(), 3);
}

#[tokio::test]
async fn visited_slot_entered_once() {
    let p = route(
        "vs-p",
        Some(18709),
        &[],
        vec![
            rule(prefix("/api/v1"), delegate_to("vs-child")),
            rule(prefix("/api"), delegate_to("vs-child")),
        ],
    );
    let child = route("vs-child", None, &["vs-p"], vec![rule(prefix("/other"), svc())]);
    let (_mgr, table) = publish(18709, &[&p, &child]);
    let (result, state) = run(&table, "/api/v1/x", &[]).await;
    assert!(matches!(result, Err(EdError::RouteNotFound())));
    assert_eq!(state.expect("descent ran").entries(), 1);
}

#[tokio::test]
async fn mask_conflict_skips_child_candidate() {
    let modifier =
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}]);
    let a = route(
        "mc-a",
        Some(18710),
        &[],
        vec![json!({"matches": prefix("/api"), "filters": modifier, "backendRefs": delegate_to("mc-child")})],
    );
    let child = route(
        "mc-child",
        None,
        &["mc-a"],
        vec![
            json!({"matches": exact("/api/a"), "filters": modifier, "backendRefs": svc()}),
            rule(prefix("/api"), svc()),
        ],
    );
    let (_mgr, table) = publish(18710, &[&a, &child]);
    assert_eq!(matched(&table, "/api/a").await, hit("mc-child", 1, &["mc-a"]));
}

/// Regression guard: a backendRef-level filter runs after the route chain, so
/// a child candidate carrying the parent's mandatory filter type on a
/// backendRef conflicts exactly like a rule-level one and is skipped.
#[tokio::test]
async fn backend_ref_filter_mask_conflict_skips_child_candidate() {
    let modifier = json!([{"type": "ResponseHeaderModifier",
        "responseHeaderModifier": {"set": [{"name": "Strict-Transport-Security", "value": "max-age=1"}]}}]);
    let a = route(
        "bc-a",
        Some(18760),
        &[],
        vec![json!({"matches": prefix("/api"), "filters": modifier, "backendRefs": delegate_to("bc-child")})],
    );
    let smuggled = json!([{"name": "svc", "port": 80, "filters": [{"type": "ResponseHeaderModifier",
        "responseHeaderModifier": {"remove": ["Strict-Transport-Security"]}}]}]);
    let child = route(
        "bc-child",
        None,
        &["bc-a"],
        vec![
            json!({"matches": exact("/api/a"), "backendRefs": smuggled}),
            rule(prefix("/api"), svc()),
        ],
    );
    let (_mgr, table) = publish(18760, &[&a, &child]);
    assert_eq!(matched(&table, "/api/a").await, hit("bc-child", 1, &["bc-a"]));
}

#[tokio::test]
async fn slot_swap_after_match_keeps_matched_leaf_and_next_request_sees_new_generation() {
    let a = route(
        "mg-a",
        Some(18711),
        &[],
        vec![rule(prefix("/api"), delegate_to("mg-child"))],
    );
    let child = route("mg-child", None, &["mg-a"], vec![rule(prefix("/api/old"), svc())]);
    let (mgr, table) = publish(18711, &[&a, &child]);
    let slot = mgr.delegation_slots().get_or_create(&key("mg-a", "mg-child"));
    let old_subtree = slot.load().unwrap();

    let (result, _) = run(&table, "/api/old", &[]).await;
    let result = result.unwrap();

    let updated = route("mg-child", None, &["mg-a"], vec![rule(prefix("/api/new"), svc())]);
    let changed: HashMap<_, _> = [(updated.key_name(), Arc::clone(&updated))].into();
    mgr.partial_update(HashMap::new(), changed, HashSet::new());
    assert!(!Arc::ptr_eq(&old_subtree, &slot.load().unwrap()), "slot swapped");

    // The matched leaf still belongs to the generation loaded during the match.
    assert!(Arc::ptr_eq(&result.route_unit, &old_subtree.route_rules_list[0]));
    assert_eq!(
        result.route_unit.matched_info.m.path.as_ref().unwrap().value.as_deref(),
        Some("/api/old")
    );
    assert_eq!(matched(&table, "/api/old").await, None);
    assert_eq!(matched(&table, "/api/new").await, hit("mg-child", 0, &["mg-a"]));
}

#[tokio::test]
async fn ordinary_match_allocates_no_descent_state() {
    let a = route("om-a", Some(18712), &[], vec![rule(prefix("/plain"), svc())]);
    let d = route(
        "om-d",
        Some(18712),
        &[],
        vec![rule(prefix("/api"), delegate_to("om-child"))],
    );
    let child = route("om-child", None, &["om-d"], vec![rule(prefix("/api"), svc())]);
    let (_mgr, table) = publish(18712, &[&a, &d, &child]);

    let (result, state) = run(&table, "/plain/x", &[]).await;
    let result = result.unwrap();
    assert!(result.delegated.is_none());
    assert!(state.is_none(), "an ordinary match never creates descent state");

    let (result, state) = run(&table, "/nothing", &[]).await;
    assert!(matches!(result, Err(EdError::RouteNotFound())));
    assert!(state.is_none(), "an ordinary miss never creates descent state");

    let (result, state) = run(&table, "/api/x", &[]).await;
    assert_eq!(
        chain(&result.unwrap()),
        ("om-child".to_string(), 0, vec!["om-d".to_string()])
    );
    assert!(state.is_some(), "a delegating hit creates descent state lazily");
}

#[tokio::test]
async fn child_level_checks_its_own_header_conditions() {
    let a = route(
        "hc-a",
        Some(18713),
        &[],
        vec![rule(prefix("/api"), delegate_to("hc-child"))],
    );
    let child = route(
        "hc-child",
        None,
        &["hc-a"],
        vec![rule(
            json!([{"path": {"type": "PathPrefix", "value": "/api"}, "headers": [{"name": "x-tenant", "value": "t1"}]}]),
            svc(),
        )],
    );
    let (_mgr, table) = publish(18713, &[&a, &child]);
    let (result, _) = run(&table, "/api/x", &[("x-tenant", "t1")]).await;
    assert_eq!(chain(&result.unwrap()).0, "hc-child");
    assert!(matches!(
        run(&table, "/api/x", &[]).await.0,
        Err(EdError::RouteNotFound())
    ));
}

#[tokio::test]
async fn backtrack_restores_chain_mask_for_next_candidate() {
    let modifier =
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}]);
    let p = route(
        "bm-p",
        Some(18714),
        &[],
        vec![
            json!({"matches": prefix("/api/v1"), "filters": modifier, "backendRefs": delegate_to("bm-x")}),
            rule(prefix("/api"), delegate_to("bm-y")),
        ],
    );
    let x = route("bm-x", None, &["bm-p"], vec![rule(prefix("/nowhere"), svc())]);
    let y = route(
        "bm-y",
        None,
        &["bm-p"],
        vec![json!({"matches": prefix("/api"), "filters": modifier, "backendRefs": svc()})],
    );
    let (_mgr, table) = publish(18714, &[&p, &x, &y]);
    // The first candidate's mask must not leak into the second candidate's chain.
    assert_eq!(matched(&table, "/api/v1/z").await, hit("bm-y", 0, &["bm-p"]));
}
