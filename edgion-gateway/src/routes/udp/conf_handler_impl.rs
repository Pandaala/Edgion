use crate::routes::l4_partial_update::apply_l4_partial_update;
use crate::routes::udp::route_unit::UdpRouteUnit;
use crate::routes::udp::routes_mgr::{
    get_global_udp_route_managers, resolved_listeners_for_route, GlobalUdpRouteManagers,
};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::UDPRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Create a handler for registration with ConfigClient.
pub fn create_udp_route_handler() -> Box<dyn ConfHandler<UDPRoute> + Send + Sync> {
    Box::new(get_global_udp_route_managers())
}

impl ConfHandler<UDPRoute> for GlobalUdpRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<UDPRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "udp_route_manager", cnt = data.len(), "full set start");

        self.clear_route_cache();

        for (key, route) in data {
            match self.initialize_route(Arc::clone(route)) {
                Ok(initialized) => {
                    self.insert_route(initialized);
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to initialize UDPRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_listener_managers_locked();
        tracing::info!(
            component = "udp_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Udp,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_udp(route),
                    )
                })
                .collect(),
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<UDPRoute>>,
        update: HashMap<String, Arc<UDPRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_udp(route),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        tracing::info!(
            component = "udp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        apply_l4_partial_update(
            "UDPRoute",
            add,
            update,
            remove,
            |key| self.get_route(key),
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |key| self.remove_route(key),
            |route| {
                self.initialize_route(route).map(|unit| {
                    let lks = resolved_listeners_for_route(&unit.route).to_vec();
                    self.insert_route(unit);
                    lks
                })
            },
            |affected| self.rebuild_affected_listener_managers(affected),
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Udp,
            policy_inputs,
            &policy_removed,
        );
    }
}

impl GlobalUdpRouteManagers {
    /// Compile a raw UDPRoute schema into its gateway-side [`UdpRouteUnit`].
    ///
    /// Built ONCE per route here (in the conf-handler), then cached and reused
    /// across listener rebuilds, so a sibling route change on the same listener
    /// never resets this route's `backend_finder` WRR cursor. The compiled
    /// runtime (BackendSelector, stream-plugin store key) lives on the unit,
    /// not on the schema — see [`UdpRouteUnit::from_route`].
    fn initialize_route(&self, route: Arc<UDPRoute>) -> Result<Arc<UdpRouteUnit>, String> {
        Ok(Arc::new(UdpRouteUnit::from_route(route)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_status_updates_replace_acceptance_with_unchanged_listeners() {
        let managers = GlobalUdpRouteManagers::new();
        let listener = make_listener_key(19443, "UDP");
        let mut route = create_test_udp_route("default", "owner-update", "gateway", "udp", 19443, listener.clone());
        let key = "default/owner-update".to_string();
        let parent = route.spec.parent_refs.as_ref().unwrap()[0].clone();
        route.status = Some(
            serde_json::from_value(serde_json::json!({"parents": [{
                "parentRef": parent, "controllerName": "example.net/owner",
                "conditions": [{"type": "Accepted", "status": "False", "reason": "Rejected",
                    "message": "", "lastTransitionTime": "2026-09-20T00:00:00Z"}]
            }]}))
            .unwrap(),
        );
        route.spec.resolved_status_controller = Some("example.net/other".into());
        managers.full_set(&HashMap::from([(key.clone(), Arc::new(route.clone()))]));
        let original = managers.get_route(&key).unwrap();
        assert_eq!(original.resolved_parent_refs.len(), 1);

        // Only the stamped owner changes: the previously foreign rejection now applies.
        route.spec.resolved_status_controller = Some("example.net/owner".into());
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::new(route.clone()))]),
            HashSet::new(),
        );
        assert!(managers.get_route(&key).unwrap().resolved_parent_refs.is_empty());
        assert_eq!(
            original.resolved_parent_refs.len(),
            1,
            "in-flight snapshot stays immutable"
        );

        // Only status changes, with exactly the same listener attachment.
        route.status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::new(route.clone()))]),
            HashSet::new(),
        );
        assert_eq!(managers.get_route(&key).unwrap().resolved_parent_refs.len(), 1);
        route.spec.resolved_status_controller = None;
        managers.full_set(&HashMap::from([(key.clone(), Arc::new(route))]));
        assert!(managers.get_route(&key).unwrap().resolved_parent_refs.is_empty());
        managers.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
        assert!(managers.get_route(&key).is_none());
    }
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::udp_route::*;
    use edgion_resources::ListenerKey;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_udp_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        listener_name: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> UDPRoute {
        UDPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: UDPRouteSpec {
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: gateway.to_string(),
                    section_name: Some(listener_name.to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![UDPRouteRule {
                    backend_refs: Some(vec![UDPBackendRef {
                        name: "test-service".to_string(),
                        namespace: Some(namespace.to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
    }

    #[test]
    fn schema_arc_is_shared_across_full_set_and_listener_snapshots() {
        let managers = GlobalUdpRouteManagers::new();
        let first = make_listener_key(9700, "UDP");
        let second = make_listener_key(9701, "UDP");
        let mut route = create_test_udp_route("default", "shared", "gw", "udp", 9700, first.clone());
        route.spec.resolved_listeners = Some(vec![first.clone(), second.clone()]);
        let route = Arc::new(route);
        let data = HashMap::from([("default/shared".to_string(), Arc::clone(&route))]);
        managers.full_set(&data);

        let cached = managers.get_route("default/shared").unwrap();
        assert!(Arc::ptr_eq(&cached.route, &route));
        let first_manager = managers.get_or_create_listener_manager(&first);
        let second_manager = managers.get_or_create_listener_manager(&second);
        for manager in [&first_manager, &second_manager] {
            let selected = manager.load_route_table().match_route().unwrap();
            assert!(Arc::ptr_eq(&selected, &cached));
            assert!(Arc::ptr_eq(&selected.backend_finder, &cached.backend_finder));
        }

        managers.full_set(&HashMap::new());
        assert_eq!(managers.stats().route_cache, 0);
        // Check the retained handles, not newly created empty managers.
        assert!(first_manager.load_route_table().match_route().is_none());
        assert!(second_manager.load_route_table().match_route().is_none());
        assert!(Arc::ptr_eq(&cached.route, &route));
    }

    #[test]
    fn delta_schema_sharing_preserves_siblings_and_old_generations() {
        let managers = GlobalUdpRouteManagers::new();
        let old_listener = make_listener_key(9710, "UDP");
        let new_listener = make_listener_key(9711, "UDP");
        let mut original = create_test_udp_route("default", "a", "gw", "udp", 9710, old_listener.clone());
        original.metadata.annotations =
            Some([("edgion.io/edgion-stream-plugins".to_string(), "old-policy".to_string())].into());
        let original = Arc::new(original);
        let sibling = Arc::new(create_test_udp_route(
            "default",
            "b",
            "gw",
            "udp",
            9710,
            old_listener.clone(),
        ));
        managers.partial_update(
            HashMap::from([
                ("default/a".to_string(), Arc::clone(&original)),
                ("default/b".to_string(), Arc::clone(&sibling)),
            ]),
            HashMap::new(),
            HashSet::new(),
        );
        let old_unit = managers.get_route("default/a").unwrap();
        let sibling_unit = managers.get_route("default/b").unwrap();
        assert!(Arc::ptr_eq(&old_unit.route, &original));
        assert!(Arc::ptr_eq(&sibling_unit.route, &sibling));
        let old_manager = managers.get_or_create_listener_manager(&old_listener);
        let old_snapshot = Arc::clone(&old_manager.load_route_table());
        assert!(Arc::ptr_eq(&old_snapshot.match_route().unwrap(), &old_unit));

        let mut updated = create_test_udp_route("default", "a", "gw", "udp", 9711, new_listener.clone());
        updated.metadata.annotations = Some(
            [(
                "edgion.io/edgion-stream-plugins".to_string(),
                "other/new-policy".to_string(),
            )]
            .into(),
        );
        updated.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name = "new-service".to_string();
        let updated = Arc::new(updated);
        managers.partial_update(
            HashMap::new(),
            HashMap::from([("default/a".to_string(), Arc::clone(&updated))]),
            HashSet::new(),
        );
        let new_unit = managers.get_route("default/a").unwrap();
        let new_manager = managers.get_or_create_listener_manager(&new_listener);
        assert!(Arc::ptr_eq(&new_unit.route, &updated));
        assert!(!Arc::ptr_eq(&new_unit, &old_unit));
        assert!(!Arc::ptr_eq(&new_unit.backend_finder, &old_unit.backend_finder));
        assert!(Arc::ptr_eq(
            &new_manager.load_route_table().match_route().unwrap(),
            &new_unit
        ));
        let remaining = old_manager.load_route_table().match_route().unwrap();
        assert!(Arc::ptr_eq(&remaining, &sibling_unit));
        assert!(Arc::ptr_eq(&remaining.backend_finder, &sibling_unit.backend_finder));
        assert!(Arc::ptr_eq(&old_snapshot.match_route().unwrap(), &old_unit));
        assert_eq!(old_unit.backend_finder.select().unwrap().name, "test-service");
        assert_eq!(new_unit.backend_finder.select().unwrap().name, "new-service");
        assert_eq!(old_unit.stream_plugin_store_key.as_deref(), Some("default/old-policy"));
        assert_eq!(new_unit.stream_plugin_store_key.as_deref(), Some("other/new-policy"));
        assert_eq!(
            old_unit.route.spec.resolved_listeners.as_ref().unwrap(),
            &[old_listener]
        );
        assert_eq!(
            new_unit.route.spec.resolved_listeners.as_ref().unwrap(),
            &[new_listener]
        );

        managers.partial_update(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["default/a".to_string(), "default/b".to_string()]),
        );
        assert_eq!(managers.stats().route_cache, 0);
        assert!(old_manager.load_route_table().match_route().is_none());
        assert!(new_manager.load_route_table().match_route().is_none());
        assert!(Arc::ptr_eq(&old_snapshot.match_route().unwrap(), &old_unit));
    }

    #[test]
    fn delta_listener_migration_clears_retained_old_manager() {
        let managers = GlobalUdpRouteManagers::new();
        let old_listener = make_listener_key(9720, "UDP");
        let new_listener = make_listener_key(9721, "UDP");
        let original = Arc::new(create_test_udp_route(
            "default",
            "a",
            "gw",
            "udp",
            9720,
            old_listener.clone(),
        ));
        managers.full_set(&HashMap::from([("default/a".to_string(), original)]));
        let retained = managers.get_or_create_listener_manager(&old_listener);
        assert_eq!(retained.load_route_table().route_count(), 1);
        let updated = Arc::new(create_test_udp_route(
            "default",
            "a",
            "gw",
            "udp",
            9721,
            new_listener.clone(),
        ));
        managers.partial_update(
            HashMap::new(),
            HashMap::from([("default/a".to_string(), Arc::clone(&updated))]),
            HashSet::new(),
        );
        assert!(retained.load_route_table().match_route().is_none());
        let selected = managers
            .get_or_create_listener_manager(&new_listener)
            .load_route_table()
            .match_route()
            .unwrap();
        assert!(Arc::ptr_eq(&selected.route, &updated));
    }

    #[test]
    fn test_udp_route_full_set() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        let mut data = HashMap::new();
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001, key9001.clone());

        data.insert("default/route1".to_string(), Arc::new(route1));
        data.insert("default/route2".to_string(), Arc::new(route2));

        managers.full_set(&data);

        let mgr9000 = managers.get_or_create_listener_manager(&key9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_listener_manager(&key9001);
        assert!(mgr9001.load_route_table().match_route().is_some());

        let key9002 = make_listener_key(9002, "UDP");
        let mgr9002 = managers.get_or_create_listener_manager(&key9002);
        assert!(mgr9002.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_udp_route_partial_update_add_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // Add a route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), Arc::new(route1));
        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key9000);
        assert!(mgr.load_route_table().match_route().is_some());

        // Remove the route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        managers.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(mgr.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_udp_route_partial_update_update() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Add initial route on listener 9000
        let route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), Arc::new(route));
        managers.partial_update(add, HashMap::new(), HashSet::new());

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Update route to listener 9001
        let updated_route = create_test_udp_route("default", "route1", "gateway1", "udp-9001", 9001, key9001.clone());
        let mut update = HashMap::new();
        update.insert("default/route1".to_string(), Arc::new(updated_route));
        managers.partial_update(HashMap::new(), update, HashSet::new());

        // Listener 9000 should be empty, listener 9001 should have the route
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_full_set_clears_previous_state() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // First full_set
        let mut data = HashMap::new();
        data.insert(
            "default/route1".to_string(),
            Arc::new(create_test_udp_route(
                "default",
                "route1",
                "gateway1",
                "udp-9000",
                9000,
                key9000.clone(),
            )),
        );
        managers.full_set(&data);
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Second full_set with empty data should clear everything
        managers.full_set(&HashMap::new());
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert_eq!(managers.stats().route_cache, 0);
    }

    #[test]
    fn test_route_without_resolved_listeners_skipped() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // Create a route without resolved_listeners
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        route.spec.resolved_listeners = None;

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), Arc::new(route));
        managers.full_set(&data);

        // Route is in cache but not assigned to any listener
        assert_eq!(managers.stats().route_cache, 1);
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
    }

    #[test]
    fn test_multi_listener_route() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Create a route that resolves to multiple listeners
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        route.spec.resolved_listeners = Some(vec![key9000.clone(), key9001.clone()]);

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), Arc::new(route));
        managers.full_set(&data);

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_partial_update_concurrent_add_and_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Add initial route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), Arc::new(route1));
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Simultaneously add route2 on 9001 and remove route1 on 9000
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001, key9001.clone());
        let mut add = HashMap::new();
        add.insert("default/route2".to_string(), Arc::new(route2));
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());

        managers.partial_update(add, HashMap::new(), remove);

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
        assert_eq!(managers.stats().route_cache, 1);
    }
}
