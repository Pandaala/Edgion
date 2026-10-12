use super::*;
use crate::lb::BackendSelector;
use crate::plugins::PluginRuntime;
use crate::routes::grpc::match_unit::{GrpcMatchInfo, GrpcMatchSpecificity};
use crate::routes::grpc::GrpcRouteInfo;
use edgion_resources::{GRPCMethodMatch, GRPCRouteMatch, GRPCRouteRule};

#[test]
fn hostname_build_failure_clears_the_complete_grpc_listener_table() {
    let unit = route_unit("discarded");
    let result: Result<GrpcMatchEngine, &str> = Err("injected capacity failure");
    let table = domain_table_from_engine_result(result, HashSet::from([unit.resource_key.clone()]), vec![unit]);
    assert!(!table.has_routes());
    assert!(table.grpc_routes.resource_keys.is_empty());
    assert!(table.grpc_routes.route_rules_list.is_empty());
    assert!(table.grpc_routes.match_engine.is_none());
}

fn make_listener_key(port: u16) -> ListenerKey {
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
    ListenerKey::new(addr, "GRPC")
}

fn route_unit(name: &str) -> Arc<GrpcRouteRuleUnit> {
    let route_match = GRPCRouteMatch {
        method: Some(GRPCMethodMatch {
            match_type: None,
            service: Some("retained.Service".to_string()),
            method: Some("Call".to_string()),
        }),
        headers: None,
    };
    Arc::new(GrpcRouteRuleUnit {
        resource_key: format!("default/{name}"),
        matched_info: GrpcMatchInfo::new("default".to_string(), name.to_string(), 0, 0, route_match, 0, None),
        rule: Arc::new(GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_allow_non_idempotent_retry: None,
        }),
        route: None,
        route_info: Arc::new(GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        }),
        specificity: GrpcMatchSpecificity {
            header_count: 0,
            creation_ts_ms: 0,
            route_key: format!("default/{name}"),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: Arc::new(PluginRuntime::new()),
        backend_finder: Arc::new(BackendSelector::new(vec![], vec![])),
        backends: Arc::from(Vec::new()),
    })
}

fn seed_nonempty_manager(managers: &GlobalGrpcRouteManagers, listener_key: &ListenerKey) -> Arc<GrpcPortRouteManager> {
    let manager = managers.get_or_create_listener_manager(listener_key);
    let units = HashMap::from([("default/prior".to_string(), vec![route_unit("prior")])]);
    let table = build_grpc_route_rules_from_units(&units, None);
    assert!(table.has_routes());
    manager.store_route_table(table);
    manager
}

#[test]
fn outer_swap_keeps_each_domain_grpc_snapshot_immutable() {
    let manager = GrpcPortRouteManager::new();
    let units = HashMap::from([("default/prior".to_string(), vec![route_unit("prior")])]);
    manager.store_route_table(build_grpc_route_rules_from_units(&units, None));
    let retained_snapshot = manager.load_route_table();
    assert!(retained_snapshot.has_routes());

    manager.store_route_table(DomainGrpcRouteRules::new());

    assert!(retained_snapshot.has_routes());
    let current = manager.load_route_table();
    assert!(!current.has_routes());
    assert!(!Arc::ptr_eq(&retained_snapshot.grpc_routes, &current.grpc_routes));
}

#[test]
fn rebuild_all_clears_a_nonempty_retained_manager_before_drop() {
    let managers = GlobalGrpcRouteManagers::new();
    let stale = make_listener_key(9830);
    let retained = seed_nonempty_manager(&managers, &stale);

    managers.rebuild_all_listener_managers_with_active(Vec::new());

    assert!(!managers.by_listener.contains_key(&stale));
    assert!(!retained.load_route_table().has_routes());
    let recreated = managers.get_or_create_listener_manager(&stale);
    assert!(!Arc::ptr_eq(&retained, &recreated));
}

#[test]
fn affected_orphan_sweep_clears_a_nonempty_retained_manager_before_drop() {
    let managers = GlobalGrpcRouteManagers::new();
    let affected = make_listener_key(9840);
    let orphan = make_listener_key(9841);
    let retained = seed_nonempty_manager(&managers, &orphan);
    let affected_set = HashSet::from([affected.clone()]);

    managers.rebuild_affected_listener_managers_with_active(&affected_set, vec![affected]);

    assert!(!managers.by_listener.contains_key(&orphan));
    assert!(!retained.load_route_table().has_routes());
    let recreated = managers.get_or_create_listener_manager(&orphan);
    assert!(!Arc::ptr_eq(&retained, &recreated));
}
