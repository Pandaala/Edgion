use super::*;
use crate::ctx::{EdgionHttpContext, NormalizedHostname};
use crate::routes::grpc::{try_match_grpc_route, GrpcMatchEngine};
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
use crate::runtime::GatewayInfo;
use pingora_proxy::Session;
use serde_json::json;
use tokio::io::AsyncWriteExt;

async fn matching_session(path: &str, hostname: &str, headers: &str) -> (Session, EdgionHttpContext) {
    let (mut client, server) = tokio::io::duplex(8192);
    client
        .write_all(format!("POST {path} HTTP/1.1\r\nHost: {hostname}\r\nContent-Length: 0\r\n{headers}\r\n").as_bytes())
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.normalized_current_path = path.to_string();
    ctx.request_info.hostname = NormalizedHostname::new(hostname);
    (session, ctx)
}

fn matching_index() -> GatewayInfoIndex {
    matching_index_with_hostname(None)
}

fn matching_index_with_hostname(hostname: Option<&str>) -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(vec![(
        "default/test-gw".to_string(),
        ListenerEntry {
            port: 19235,
            info: Arc::new(GatewayInfo::new(
                Some("default".to_string()),
                "test-gw".to_string(),
                Some("grpc".to_string()),
                None,
                None,
                None,
                vec![],
                None,
            )),
            gateway_proof: None,
            hostname: hostname.map(str::to_string),
            allowed_routes: None,
        },
    )])
}

fn matching_route(name: &str, host: &str, matcher: serde_json::Value) -> Arc<GRPCRoute> {
    let route: GRPCRoute = crate::test_current_status::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
            "metadata": {"name": name, "namespace": "default"},
            "spec": {"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "test-gw", "sectionName": "grpc"}],
                "hostnames": [host], "resolvedListeners": ["127.0.0.1:19235/HTTP"],
                "rules": [{"matches": [matcher], "backendRefs": [{"name": "svc", "port": 80}]}]}
        }))
        .unwrap();
    // Exercise the same serialized fields carried by conf_sync.
    Arc::new(serde_json::from_slice(&serde_json::to_vec(&route).unwrap()).unwrap())
}

#[tokio::test]
async fn compiled_grpc_parent_ports_select_the_matching_gateway_on_shared_listener() {
    use crate::plugins::publication::{inputs::GatewayInput, PluginPublisher};

    let publisher = PluginPublisher::new();
    let gateways = [("a", vec![8080, 8081]), ("b", vec![8081])]
        .into_iter()
        .map(|(name, ports)| {
            let gateway: edgion_resources::Gateway = crate::test_current_status::from_value(json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
                "metadata": {"namespace": "default", "name": name},
                "spec": {"gatewayClassName": "test", "listeners": ports.into_iter().map(|port|
                    json!({"name": format!("grpc-{port}"), "port": port, "protocol": "HTTP"})
                ).collect::<Vec<_>>()}
            }))
            .unwrap();
            (format!("default/{name}"), GatewayInput::from_gateway(&gateway))
        })
        .collect();
    publisher.replace_gateways(gateways);
    let publication = publisher.snapshot();
    let mut route = (*matching_route("port-owner", "example.com", json!({"method": {"method": "Get"}}))).clone();
    route.spec.parent_refs = Some(
        serde_json::from_value(json!([
            {"name": "a", "port": 8080}, {"name": "b", "port": 8081}
        ]))
        .unwrap(),
    );
    route.spec.current_status = None;
    let route = crate::test_current_status::processed(route);
    let units = parse_route_to_units("default/port-owner", &Arc::new(route));
    assert_eq!(units.len(), 1);
    let engine = GrpcMatchEngine::new(units);
    let (session, ctx) = matching_session("/svc/Get", "example.com", "").await;
    for (port, owner) in [(8080, "default/a"), (8081, "default/b")] {
        let key = ListenerKey::new(format!("0.0.0.0:{port}").parse().unwrap(), "HTTP");
        let index = publication.listener_index(&key).unwrap();
        let (_, gateway) = engine.match_route(&session, &ctx, index, "example.com").unwrap();
        assert_eq!(gateway.gateway_key(), owner);
    }
}

#[test]
fn owner_and_status_updates_replace_acceptance_with_unchanged_listeners() {
    let managers = GlobalGrpcRouteManagers::new();
    let key = "default/owner-update-grpc".to_string();
    let mut route = (*matching_route("owner-update-grpc", "example.com", json!({"method": {"method": "Get"}}))).clone();
    let parent = route.spec.parent_refs.as_ref().unwrap()[0].clone();
    route.status = Some(
        serde_json::from_value(json!({"parents": [{
            "parentRef": parent, "controllerName": "example.net/owner",
            "conditions": [{"type": "Accepted", "status": "False", "reason": "Rejected",
                "message": "", "lastTransitionTime": "2026-09-20T00:00:00Z"}]
        }]}))
        .unwrap(),
    );
    managers.full_set(&HashMap::from([(key.clone(), Arc::new(route.clone()))]));
    let old = managers.route_units_cache.lock()[&key][0].clone();
    route.spec.resolved_status_controller = Some("example.net/owner".into());
    route.spec.current_status = route.status.clone();
    route.status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
    managers.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(route.clone()))]),
        HashSet::new(),
    );
    assert!(managers.route_units_cache.lock()[&key].is_empty());
    route.spec.current_status.as_mut().unwrap().parents[0].controller_name = "example.net/owner".into();
    route.spec.current_status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
    route.status.as_mut().unwrap().parents[0].conditions[0].status = "False".into();
    managers.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(route.clone()))]),
        HashSet::new(),
    );
    assert_eq!(managers.route_units_cache.lock()[&key].len(), 1);
    assert!(!Arc::ptr_eq(&old, &managers.route_units_cache.lock()[&key][0]));
    route.spec.resolved_status_controller = None;
    managers.full_set(&HashMap::from([(key.clone(), Arc::new(route))]));
    assert!(managers.route_units_cache.lock()[&key].is_empty());
}

#[tokio::test]
async fn built_grpc_route_cannot_cross_an_empty_exact_listener_in_another_gateway() {
    let route = matching_route(
        "broad-owner-route",
        "*.example.com",
        json!({"method": {"method": "Get"}}),
    );
    let units = parse_route_to_units("default/broad-owner-route", &route);
    assert_eq!(units.len(), 1);
    let unit = Arc::clone(&units[0]);
    let engine = GrpcMatchEngine::new(units);
    let broad = matching_index_with_hostname(Some("*.example.com"));
    let blocked = GatewayInfoIndex::from_entries_for_test(vec![
        (
            "default/test-gw".into(),
            broad.get("default/test-gw", "grpc").unwrap().clone(),
        ),
        (
            "default/empty-exact".into(),
            ListenerEntry {
                port: 19235,
                info: Arc::new(GatewayInfo::new(
                    Some("default".into()),
                    "empty-exact".into(),
                    Some("grpc".into()),
                    None,
                    None,
                    None,
                    Vec::new(),
                    None,
                )),
                gateway_proof: None,
                hostname: Some("api.example.com".into()),
                allowed_routes: None,
            },
        ),
    ]);
    let (session, ctx) = matching_session("/svc/Get", "api.example.com", "").await;
    let (selected, owner) = engine.match_route(&session, &ctx, &broad, "api.example.com").unwrap();
    assert!(Arc::ptr_eq(&selected, &unit));
    assert_eq!(owner.gateway_key(), "default/test-gw");
    assert!(matches!(
        engine.match_route(&session, &ctx, &blocked, "api.example.com"),
        Err(edgion_resources::EdError::RouteNotFound())
    ));
    let (other_session, other_ctx) = matching_session("/svc/Get", "other.example.com", "").await;
    let (selected, owner) = engine
        .match_route(&other_session, &other_ctx, &blocked, "other.example.com")
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &unit));
    assert_eq!(owner.gateway_key(), "default/test-gw");
    assert!(engine.match_route(&session, &ctx, &broad, "api.example.com").is_ok());
}

#[tokio::test]
async fn method_only_matches_through_real_session_and_header_failure_falls_back() {
    let index = matching_index();
    for method in [
        json!({"method":"Get"}),
        json!({"type":"Exact", "method":"Get"}),
        json!({"type":"RegularExpression", "method":"^Get.*$"}),
    ] {
        let route = matching_route(
            "method",
            "api.example.com",
            json!({"method":method,
                "headers":[{"name":"x-version", "value":"v2"}]}),
        );
        let fallback = matching_route("fallback", "*.example.com", json!({}));
        let mut units = parse_route_to_units("default/method", &route);
        units.extend(parse_route_to_units("default/fallback", &fallback));
        let engine = GrpcMatchEngine::new(units);
        for service in ["svc.User", "other.Service"] {
            let (session, ctx) =
                matching_session(&format!("/{service}/Get"), "api.example.com", "x-version: v2\r\n").await;
            assert_eq!(
                engine
                    .match_route(&session, &ctx, &index, "api.example.com")
                    .unwrap()
                    .0
                    .resource_key,
                "default/method"
            );
        }
        let (session, ctx) = matching_session("/svc.User/Get", "api.example.com", "x-version: v1\r\n").await;
        assert_eq!(
            engine
                .match_route(&session, &ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            "default/fallback"
        );
        let (session, ctx) = matching_session("/svc.User/Set", "api.example.com", "x-version: v2\r\n").await;
        assert_eq!(
            engine
                .match_route(&session, &ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            "default/fallback"
        );
    }
}

#[tokio::test]
async fn duplicate_headers_compile_rank_and_match_the_same_canonical_vector() {
    let route = matching_route(
        "duplicate-headers",
        "api.example.com",
        json!({"headers": [
            {"name": "X-Mode", "type": "RegularExpression", "value": "^one$"},
            {"name": "x-mode", "type": "RegularExpression", "value": "["},
            {"name": "X-Region", "value": "west"},
            {"name": "x-region", "value": "ignored"}
        ]}),
    );
    let units = parse_route_to_units("default/duplicate-headers", &route);
    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(unit.specificity.header_count, 2);
    assert_eq!(unit.matched_info.compiled_header_regexes.len(), 2);
    assert!(unit.matched_info.compiled_header_regexes[0].is_some());
    assert!(unit.matched_info.compiled_header_regexes[1].is_none());

    let engine = GrpcMatchEngine::new(units);
    let index = matching_index();
    for (headers, expected) in [
        ("x-mode: one\r\nx-region: west\r\n", true),
        ("x-mode: two\r\nx-region: west\r\n", false),
        ("x-mode: one\r\nx-region: east\r\n", false),
    ] {
        let (session, ctx) = matching_session("/svc.User/Get", "api.example.com", headers).await;
        assert_eq!(
            engine.match_route(&session, &ctx, &index, "api.example.com").is_ok(),
            expected
        );
    }
}

#[test]
fn invalid_method_withdraws_whole_rule_even_without_controller_status() {
    for bad in [
        json!({}),
        json!({"service":"", "method":"Get"}),
        json!({"type":"RegularExpression", "service":"svc", "method":"["}),
    ] {
        let mut route = (*matching_route("invalid", "api.example.com", json!({"method":bad}))).clone();
        let rule = &mut route.spec.rules.as_mut().unwrap()[0];
        rule.matches
            .as_mut()
            .unwrap()
            .push(serde_json::from_value(json!({"method":{"method":"Get"}})).unwrap());
        let mut valid = rule.clone();
        valid.matches = Some(vec![
            serde_json::from_value(json!({"method":{"method":"List"}})).unwrap()
        ]);
        route.spec.rules.as_mut().unwrap().push(valid);
        for resolved in [false, true] {
            if resolved {
                route.spec.resolved_rules = route.spec.rules.clone();
            }
            let units = parse_route_to_units("default/invalid", &Arc::new(route.clone()));
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].matched_info.rule_id, 1);
            assert_eq!(
                units[0].matched_info.matched.method.as_ref().unwrap().method.as_deref(),
                Some("List")
            );
        }
    }
}

#[tokio::test]
async fn exact_listener_intersection_does_not_promote_wildcard_or_hostless_routes() {
    let mut exact = (*matching_route(
        "exact-host",
        "api.example.com",
        json!({"method":{"method":"Get"},
            "headers":[{"name":"x-version", "value":"v2"}]}),
    ))
    .clone();
    let mut wildcard = (*matching_route("wildcard-host", "*.example.com", json!({"method":{"method":"Get"}}))).clone();
    let mut hostless = (*matching_route(
        "hostless",
        "unused.example.com",
        json!({"method":{"service":"svc.User", "method":"Get"}}),
    ))
    .clone();
    hostless.spec.hostnames = None;
    // Controller intersection with an exact Listener yields the same
    // effective hostname for all three, but their Route priorities differ.
    let mut units = Vec::new();
    for route in [&mut hostless, &mut wildcard, &mut exact] {
        route.spec.resolved_hostnames = Some(vec!["api.example.com".to_string()]);
        let key = format!("default/{}", route.metadata.name.as_deref().unwrap());
        units.extend(parse_route_to_units(&key, &Arc::new(route.clone())));
    }
    let engine = GrpcMatchEngine::new(units);
    let index = matching_index_with_hostname(Some("api.example.com"));
    for (headers, expected) in [
        ("x-version: v2\r\n", "default/exact-host"),
        ("", "default/wildcard-host"),
    ] {
        let (session, ctx) = matching_session("/svc.User/Get", "api.example.com", headers).await;
        assert_eq!(
            engine
                .match_route(&session, &ctx, &index, "api.example.com")
                .unwrap()
                .0
                .resource_key,
            expected
        );
    }
    let (session, ctx) = matching_session("/svc.User/Get", "other.example.com", "").await;
    assert!(engine.match_route(&session, &ctx, &index, "other.example.com").is_err());
}

#[tokio::test]
async fn method_and_hostname_indexes_replace_atomically_on_partial_update_and_delete() {
    let mgr = GlobalGrpcRouteManagers::new();
    let key = "default/method-reload".to_string();
    let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
    let initial = matching_route("method-reload", "old.example.com", json!({"method":{"method":"Get"}}));
    mgr.full_set(&HashMap::from([(key.clone(), initial)]));
    let manager = mgr.by_listener.get(&listener).unwrap().clone();
    let old = Arc::clone(&manager.load_route_table());
    let index = matching_index();
    let (mut old_session, old_ctx) = matching_session("/svc.User/Get", "old.example.com", "").await;
    assert!(old
        .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
        .is_ok());
    let updated = matching_route(
        "method-reload",
        "new.example.com",
        json!({"method":{"type":"RegularExpression", "method":"^List.*$"}}),
    );
    mgr.partial_update(HashMap::new(), HashMap::from([(key.clone(), updated)]), HashSet::new());
    let new = Arc::clone(&manager.load_route_table());
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(new
        .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
        .is_err());
    assert!(old
        .match_route(&mut old_session, &old_ctx, &index, "old.example.com")
        .is_ok());
    let (mut new_session, new_ctx) = matching_session("/other.Service/ListAll", "new.example.com", "").await;
    assert!(new
        .match_route(&mut new_session, &new_ctx, &index, "new.example.com")
        .is_ok());
    mgr.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key]));
    assert!(!manager.load_route_table().has_routes());
    assert!(new
        .match_route(&mut new_session, &new_ctx, &index, "new.example.com")
        .is_ok());
}

#[tokio::test]
async fn invalid_authored_hostname_list_withdraws_the_whole_published_route() {
    let managers = GlobalGrpcRouteManagers::new();
    let resource_key = "default/validated-hosts".to_string();
    let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
    let valid = matching_route("validated-hosts", "grpc", json!({}));
    let index = matching_index();
    let (mut session, ctx) = matching_session("/svc/Get", "grpc", "").await;
    for hosts in [
        vec!["*".to_string()],
        vec!["grpc".to_string(), "*".to_string()],
        vec!["grpc".to_string(), "UPPER.example.com".to_string()],
        vec!["grpc".to_string(), "127.0.0.1".to_string()],
        vec!["grpc".to_string(); 17],
    ] {
        managers.full_set(&HashMap::from([(resource_key.clone(), valid.clone())]));
        let manager = managers.by_listener.get(&listener).unwrap().clone();
        let old = Arc::clone(&manager.load_route_table());
        assert!(old.match_route(&mut session, &ctx, &index, "grpc").is_ok());
        let mut invalid = (*valid).clone();
        invalid.spec.hostnames = Some(hosts);
        // Forged/stale resolved projection must not salvage authored invalidity.
        invalid.spec.resolved_hostnames = Some(vec!["grpc".to_string()]);
        let invalid = Arc::new(invalid);
        assert!(parse_route_to_units(&resource_key, &invalid).is_empty());
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(resource_key.clone(), invalid)]),
            HashSet::new(),
        );
        assert!(!manager.load_route_table().has_routes());
        assert!(old.match_route(&mut session, &ctx, &index, "grpc").is_ok());
    }
    for hosts in [None, Some(Vec::new())] {
        let mut hostless = (*valid).clone();
        hostless.spec.hostnames = hosts;
        assert!(!parse_route_to_units(&resource_key, &Arc::new(hostless)).is_empty());
    }
}

#[tokio::test]
async fn partial_and_full_rebuild_share_hostname_tree_with_coherent_payloads() {
    let managers = GlobalGrpcRouteManagers::new();
    let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
    let first = matching_route("first", "api.example.com", json!({"method":{"method":"Get"}}));
    let fallback = matching_route("fallback", "*.example.com", json!({}));
    managers.full_set(&HashMap::from([
        ("default/first".to_string(), first),
        ("default/fallback".to_string(), fallback.clone()),
    ]));
    let manager = managers.by_listener.get(&listener).unwrap().clone();
    let old = Arc::clone(&manager.load_route_table());
    let unchanged = {
        let cache = managers.route_units_cache.lock();
        Arc::clone(&cache["default/fallback"][0])
    };
    let replacement = matching_route("first", "api.example.com", json!({"method":{"method":"List"}}));
    managers.partial_update(
        HashMap::new(),
        HashMap::from([("default/first".to_string(), replacement.clone())]),
        HashSet::new(),
    );
    let updated = Arc::clone(&manager.load_route_table());
    let old_engine = old.grpc_routes.match_engine.as_ref().unwrap();
    let new_engine = updated.grpc_routes.match_engine.as_ref().unwrap();
    assert!(old_engine.shares_hosts(new_engine));
    let index = matching_index();
    let (mut session, ctx) = matching_session("/svc/Get", "api.example.com", "").await;
    assert_eq!(
        old.match_route(&mut session, &ctx, &index, "api.example.com")
            .unwrap()
            .0
            .resource_key,
        "default/first"
    );
    let selected = updated
        .match_route(&mut session, &ctx, &index, "api.example.com")
        .unwrap()
        .0;
    assert_eq!(selected.resource_key, "default/fallback");
    assert!(Arc::ptr_eq(&selected, &unchanged));
    assert!(Arc::ptr_eq(&selected.plugin_runtime, &unchanged.plugin_runtime));
    assert!(Arc::ptr_eq(&selected.backend_finder, &unchanged.backend_finder));
    let (mut list_session, list_ctx) = matching_session("/svc/List", "api.example.com", "").await;
    assert_eq!(
        updated
            .match_route(&mut list_session, &list_ctx, &index, "api.example.com")
            .unwrap()
            .0
            .resource_key,
        "default/first"
    );

    managers.full_set(&HashMap::from([
        ("default/first".to_string(), replacement),
        ("default/fallback".to_string(), fallback),
    ]));
    let reconnected = Arc::clone(&manager.load_route_table());
    assert!(new_engine.shares_hosts(reconnected.grpc_routes.match_engine.as_ref().unwrap()));
    managers.rebuild_all_listener_managers();
    let rebuilt = Arc::clone(&manager.load_route_table());
    assert!(new_engine.shares_hosts(rebuilt.grpc_routes.match_engine.as_ref().unwrap()));
    managers.full_set(&HashMap::new());
    assert!(!manager.load_route_table().has_routes());
    assert!(reconnected.has_routes());
}

#[tokio::test]
async fn grpc_first_and_http_fallback_keep_overlapping_hostnames_usable() {
    use crate::routes::http::routes_mgr::GlobalHttpRouteManagers;
    let grpc_mgr = GlobalGrpcRouteManagers::new();
    let http_mgr = GlobalHttpRouteManagers::new();
    let listener = ListenerKey("127.0.0.1:19235/HTTP".to_string());
    let grpc = matching_route("grpc-first", "api.example.com", json!({"method":{"method":"Get"}}));
    grpc_mgr.full_set(&HashMap::from([("default/grpc-first".to_string(), grpc)]));
    let http = Arc::new(
            crate::test_current_status::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute",
                "metadata":{"name":"http-fallback", "namespace":"default"},
                "spec":{"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs":[{"name":"test-gw", "sectionName":"grpc"}],
                    "hostnames":["api.example.com"], "resolvedListeners":["127.0.0.1:19235/HTTP"],
                    "rules":[{"matches":[{"path":{"type":"PathPrefix", "value":"/"}}],
                        "backendRefs":[{"name":"svc", "port":80}]}]}
            }))
            .unwrap(),
        );
    http_mgr.full_set(&HashMap::from([("default/http-fallback".to_string(), http)]));
    let grpc_manager = grpc_mgr.by_listener.get(&listener).unwrap().clone();
    let http_manager = http_mgr.by_listener.get(&listener).unwrap().clone();
    let grpc_table = Arc::clone(&grpc_manager.load_route_table());
    let http_table = http_manager.load_route_table();
    let index = matching_index();
    for (method, expect_grpc) in [("Get", true), ("Set", false)] {
        let (mut session, mut ctx) = matching_session(&format!("/svc.User/{method}"), "api.example.com", "").await;
        let matched = try_match_grpc_route(&grpc_table, &mut session, &mut ctx, &index)
            .await
            .unwrap();
        assert_eq!(matched, expect_grpc);
        assert_eq!(ctx.is_grpc_route_matched, expect_grpc);
        if !matched {
            assert!(http_table.match_route(&mut session, &ctx, &index).is_ok());
        }
    }
}

/// Build a minimal GRPCRoute suitable for unit tests.
///
/// The route has a single parent ref (so `filter_accepted_parent_refs` returns
/// `Some`) and one catch-all rule with a backend ref (so `parse_route_to_units`
/// produces at least one unit).
fn build_test_grpc_route(namespace: &str, name: &str) -> GRPCRoute {
    let json = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "namespace": namespace, "name": name },
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
            "rules": [{
                "backendRefs": [{ "name": "svc1", "port": 8080 }]
            }]
        }
    });
    let mut route: GRPCRoute = crate::test_current_status::from_value(json).expect("Failed to build GRPCRoute");
    route.pre_parse();
    route
}

/// Build a GRPCRoute with two `spec.rules`, each with a distinct backend
/// name, so tests can tell which rule(s) actually compiled.
fn build_grpc_route_with_two_rules(namespace: &str, name: &str) -> GRPCRoute {
    let json = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "namespace": namespace, "name": name },
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
            "rules": [
                { "matches": [{ "method": { "service": "svc.SpecA" } }],
                  "backendRefs": [{ "name": "spec-svc-a", "port": 8080 }] },
                { "matches": [{ "method": { "service": "svc.SpecB" } }],
                  "backendRefs": [{ "name": "spec-svc-b", "port": 8080 }] }
            ]
        }
    });
    let mut route: GRPCRoute = crate::test_current_status::from_value(json).expect("Failed to build GRPCRoute");
    route.pre_parse();
    route
}

/// Extract backend Service names from every unit's compiled rule, so
/// tests can assert exactly which backendRefs were compiled.
fn unit_backend_names(units: &[Arc<GrpcRouteRuleUnit>]) -> Vec<String> {
    let mut names: Vec<String> = units
        .iter()
        .flat_map(|u| u.rule.backend_refs.iter().flatten())
        .map(|br| br.name.clone())
        .collect();
    names.sort();
    names
}

/// Build-site wiring, gRPC side. Mirrors the HTTP test in
/// `routes::http::conf_handler_impl`: only this test would fail if
/// `parse_route_to_units` ever handed `resolve_parent_refs` something other
/// than the route namespace that the request path later passes as `route_ns`.
///
/// gRPC uses its own `unwrap_or("default")` fallback, independent of HTTP's
/// stricter "reject a route with no namespace" rule, so it needs its own guard.
#[test]
fn build_site_resolves_parent_ref_keys_with_the_route_namespace() {
    // The fixture declares `parentRefs: [{ name: "test-gw" }]` with no
    // namespace, so the resolved key must fall back to the route namespace.
    let route = build_grpc_route_with_two_rules("route-ns", "wiring");
    let units = parse_route_to_units("route-ns/wiring", &Arc::new(route));
    assert!(!units.is_empty(), "the fixture must compile to at least one unit");

    for unit in &units {
        let resolved = unit
            .route_info
            .parent_refs
            .as_ref()
            .expect("the build site always populates parent_refs");
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].gateway_key, "route-ns/test-gw",
            "the omitted parentRef namespace must fall back to the ROUTE namespace"
        );
        assert_eq!(resolved[0].parent_ns, "route-ns");
        // The same namespace the request path passes as `route_ns`.
        assert_eq!(unit.matched_info.rsc.ns, "route-ns");
    }
}

#[test]
fn test_grpc_resolved_rules_compiled_instead_of_spec_rules() {
    // Delegation contract: when resolved_rules is present the gateway
    // compiles it INSTEAD of spec.rules, including a resolved Service
    // that is absent from spec.rules entirely (the flattened child).
    let mut route = build_grpc_route_with_two_rules("test-grpc-deleg", "flat");
    route.spec.resolved_rules = Some(
        serde_json::from_value(serde_json::json!([
            { "matches": [{ "method": { "service": "svc.Resolved" } }],
              "backendRefs": [{ "name": "resolved-svc", "namespace": "test-grpc-deleg", "port": 8080 }] }
        ]))
        .unwrap(),
    );
    route.pre_parse();

    let route = Arc::new(route);
    let units = parse_route_to_units("test-grpc-deleg/flat", &route);
    assert_eq!(
        unit_backend_names(&units),
        vec!["resolved-svc"],
        "resolved_rules must replace spec.rules, compiling the resolved service"
    );
}

#[test]
fn test_grpc_invalid_rule_indices_ignored_when_resolved_rules_present() {
    // invalid_rule_indices refers to spec.rules; a flattened route must
    // not have its (pre-filtered) resolved_rules dropped by stale indices.
    let mut route = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "flat");
    route.spec.invalid_rule_indices = Some(vec![0]);
    route.spec.resolved_rules = Some(
        serde_json::from_value(serde_json::json!([
            { "matches": [{ "method": { "service": "svc.RA" } }],
              "backendRefs": [{ "name": "svc-r-a", "port": 8080 }] },
            { "matches": [{ "method": { "service": "svc.RB" } }],
              "backendRefs": [{ "name": "svc-r-b", "port": 8080 }] }
        ]))
        .unwrap(),
    );
    route.pre_parse();

    let route = Arc::new(route);
    let units = parse_route_to_units("test-grpc-deleg-inv/flat", &route);
    assert_eq!(
        unit_backend_names(&units),
        vec!["svc-r-a", "svc-r-b"],
        "invalid_rule_indices (index 0) must not apply to resolved_rules"
    );

    // Control: without resolved_rules the same indices DO drop rule 0.
    let mut plain = build_grpc_route_with_two_rules("test-grpc-deleg-inv", "plain");
    plain.spec.invalid_rule_indices = Some(vec![0]);
    let plain = Arc::new(plain);
    let units = parse_route_to_units("test-grpc-deleg-inv/plain", &plain);
    assert_eq!(
        unit_backend_names(&units),
        vec!["spec-svc-b"],
        "indices must still apply to spec.rules"
    );
}

#[test]
fn test_grpc_full_set_shares_arc_and_unit_carries_route() {
    let mgr = GlobalGrpcRouteManagers::new();
    let key = "test-grpc-arc/route1";
    let route = std::sync::Arc::new(build_test_grpc_route("test-grpc-arc", "route1"));
    let mut data = std::collections::HashMap::new();
    data.insert(key.to_string(), route.clone());
    mgr.full_set(&data);

    let cached = mgr.route_cache.get(key).expect("cached");
    assert!(std::sync::Arc::ptr_eq(cached.value(), &route), "cache shares Arc");

    let units = mgr.route_units_cache.lock();
    let unit = units.get(key).and_then(|v| v.first()).expect("a unit");
    assert!(
        std::sync::Arc::ptr_eq(unit.route.as_ref().expect("unit route"), &route),
        "unit shares the cache Arc"
    );
}

#[test]
fn metric_handles_are_shared_within_each_rule_only() {
    let json = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "namespace": "route-ns", "name": "metric-sharing" },
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{ "name": "test-gw", "kind": "Gateway" }],
            "rules": [
                {
                    "matches": [
                        { "method": { "service": "svc.RuleA", "method": "One" } },
                        { "method": { "service": "svc.RuleA", "method": "Two" } }
                    ],
                    "backendRefs": [
                        { "name": "rule-a-primary", "port": 8080 },
                        { "name": "rule-a-secondary", "port": 8081 }
                    ]
                },
                {
                    "matches": [{ "method": { "service": "svc.RuleB", "method": "Only" } }],
                    "backendRefs": [{ "name": "rule-b", "port": 9090 }]
                }
            ]
        }
    });
    let mut route: GRPCRoute = crate::test_current_status::from_value(json).expect("Failed to build GRPCRoute");
    route.pre_parse();

    let route = Arc::new(route);
    let units = parse_route_to_units("route-ns/metric-sharing", &route);
    assert_eq!(units.len(), 3);
    assert!(Arc::ptr_eq(&units[0].metric_handles, &units[1].metric_handles));
    assert!(!Arc::ptr_eq(&units[0].metric_handles, &units[2].metric_handles));

    assert_eq!((units[0].matched_info.rule_id, units[0].matched_info.match_id), (0, 0));
    assert_eq!((units[1].matched_info.rule_id, units[1].matched_info.match_id), (0, 1));
    assert_eq!((units[2].matched_info.rule_id, units[2].matched_info.match_id), (1, 0));
    let matched_methods: Vec<_> = units
        .iter()
        .map(|unit| {
            let method = unit.matched_info.matched.method.as_ref().expect("method match");
            (method.service.as_deref(), method.method.as_deref())
        })
        .collect();
    assert_eq!(
        matched_methods,
        vec![
            (Some("svc.RuleA"), Some("One")),
            (Some("svc.RuleA"), Some("Two")),
            (Some("svc.RuleB"), Some("Only")),
        ]
    );

    let first_rule_metrics = units[0]
        .metric_handles
        .lookup("route-ns", "test-gw")
        .expect("first rule gateway metrics");
    let second_rule_metrics = units[2]
        .metric_handles
        .lookup("route-ns", "test-gw")
        .expect("second rule gateway metrics");
    assert_eq!(first_rule_metrics.counters.len(), 2);
    assert_eq!(second_rule_metrics.counters.len(), 1);

    let retained = Arc::clone(&units[1]);
    assert!(Arc::ptr_eq(&units[0].backend_finder, &retained.backend_finder));
    assert!(!Arc::ptr_eq(&retained.backend_finder, &units[2].backend_finder));
    assert_eq!(units[0].backend_finder.select(), Ok(0));
    assert_eq!(retained.backend_finder.select(), Ok(1));
    assert_eq!(units[0].backend_finder.select(), Ok(0));
    let replacement = parse_route_to_units("route-ns/metric-sharing", &route);
    assert!(!Arc::ptr_eq(&retained.backend_finder, &replacement[0].backend_finder));
    drop(units);
    assert_eq!(replacement[0].backend_finder.select(), Ok(0));
    assert_eq!(retained.backend_finder.select(), Ok(1));
    assert_eq!(replacement[1].backend_finder.select(), Ok(1));
}

#[test]
fn compiled_grpc_empty_backend_selector_retains_no_backend_error() {
    let selector = GrpcRouteRuleUnit::build_backend_selector(&[]);
    assert_eq!(selector.select(), Err(crate::lb::ERR_NO_BACKEND_REFS));
}
