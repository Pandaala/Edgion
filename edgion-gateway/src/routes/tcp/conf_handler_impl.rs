use crate::routes::l4_partial_update::apply_l4_partial_update;
use crate::routes::tcp::route_unit::TcpRouteUnit;
use crate::routes::tcp::routes_mgr::{
    get_global_tcp_route_managers, resolved_listeners_for_route, GlobalTcpRouteManagers,
};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::TCPRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Create a handler for registration with ConfigClient.
pub fn create_tcp_route_handler() -> Box<dyn ConfHandler<TCPRoute> + Send + Sync> {
    Box::new(get_global_tcp_route_managers())
}

impl ConfHandler<TCPRoute> for GlobalTcpRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<TCPRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(component = "tcp_route_manager", cnt = data.len(), "full set start");

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
                        "Failed to initialize TCPRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_listener_managers_locked();
        tracing::info!(
            component = "tcp_route_manager",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<TCPRoute>>,
        update: HashMap<String, Arc<TCPRoute>>,
        remove: HashSet<String>,
    ) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        tracing::info!(
            component = "tcp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        apply_l4_partial_update(
            "TCPRoute",
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
    }
}

impl GlobalTcpRouteManagers {
    /// Compile a raw TCPRoute schema into its gateway-side [`TcpRouteUnit`].
    ///
    /// Built ONCE per route here (in the conf-handler), then cached and reused
    /// across listener rebuilds, so a sibling route change on the same listener
    /// never resets this route's `backend_finder` WRR cursor. The compiled
    /// runtime (BackendSelector, stream-plugin store key, keepalive config) lives
    /// on the unit, not on the schema — see [`TcpRouteUnit::from_route`].
    fn initialize_route(&self, route: Arc<TCPRoute>) -> Result<Arc<TcpRouteUnit>, String> {
        Ok(Arc::new(TcpRouteUnit::from_route(route)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::tcp_route::*;
    use edgion_resources::ListenerKey;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tcp_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        listener_name: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> TCPRoute {
        TCPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TCPRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: gateway.to_string(),
                    section_name: Some(listener_name.to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![TCPRouteRule {
                    backend_refs: Some(vec![TCPBackendRef {
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
    fn test_tcp_route_full_set() {
        let managers = GlobalTcpRouteManagers::new();
        let key9000 = make_listener_key(9000, "TCP");
        let key9001 = make_listener_key(9001, "TCP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        let mut data = HashMap::new();
        let route1 = create_test_tcp_route("default", "route1", "gateway1", "tcp-9000", 9000, key9000.clone());
        let route2 = create_test_tcp_route("default", "route2", "gateway1", "tcp-9001", 9001, key9001.clone());

        data.insert("default/route1".to_string(), Arc::new(route1));
        data.insert("default/route2".to_string(), Arc::new(route2));

        managers.full_set(&data);

        for (key, source) in &data {
            let cached = managers.get_route(key).unwrap();
            assert!(Arc::ptr_eq(source, &cached.route));
        }

        let mgr9000 = managers.get_or_create_listener_manager(&key9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_listener_manager(&key9001);
        assert!(mgr9001.load_route_table().match_route().is_some());

        let key9002 = make_listener_key(9002, "TCP");
        let mgr9002 = managers.get_or_create_listener_manager(&key9002);
        assert!(mgr9002.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_partial_update_shares_schema_and_preserves_prior_generation() {
        let managers = GlobalTcpRouteManagers::new();
        let old_listener = make_listener_key(9400, "TCP");
        let new_listener = make_listener_key(9401, "TCP");
        let source = Arc::new(create_test_tcp_route(
            "default",
            "a-moving",
            "gateway",
            "old",
            9400,
            old_listener.clone(),
        ));
        let sibling_source = Arc::new(create_test_tcp_route(
            "default",
            "z-sibling",
            "gateway",
            "new",
            9401,
            new_listener.clone(),
        ));
        managers.partial_update(
            HashMap::from([
                ("default/a-moving".to_string(), Arc::clone(&source)),
                ("default/z-sibling".to_string(), Arc::clone(&sibling_source)),
            ]),
            HashMap::new(),
            HashSet::new(),
        );
        let prior = managers.get_route("default/a-moving").unwrap();
        let sibling = managers.get_route("default/z-sibling").unwrap();
        assert!(Arc::ptr_eq(&source, &prior.route));
        assert!(Arc::ptr_eq(&sibling_source, &sibling.route));
        let old_manager = managers.get_or_create_listener_manager(&old_listener);
        let old_snapshot = Arc::clone(&old_manager.load_route_table());
        assert!(Arc::ptr_eq(&prior, &old_snapshot.match_route().unwrap()));

        let mut updated = (*source).clone();
        updated.spec.resolved_listeners = Some(vec![new_listener.clone()]);
        let parent = &mut updated.spec.parent_refs.as_mut().unwrap()[0];
        parent.section_name = Some("new".to_string());
        parent.port = Some(9401);
        updated.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name = "new-service".to_string();
        let updated = Arc::new(updated);
        managers.partial_update(
            HashMap::new(),
            HashMap::from([("default/a-moving".to_string(), Arc::clone(&updated))]),
            HashSet::new(),
        );

        let current = managers.get_route("default/a-moving").unwrap();
        assert!(Arc::ptr_eq(&updated, &current.route));
        assert!(!Arc::ptr_eq(&prior, &current));
        assert!(old_manager.load_route_table().match_route().is_none());
        let new_manager = managers.get_or_create_listener_manager(&new_listener);
        assert!(Arc::ptr_eq(
            &current,
            &new_manager.load_route_table().match_route().unwrap()
        ));
        assert_eq!(current.backend_finder.select().unwrap().name, "new-service");

        // Retained input and in-flight snapshots keep the previous generation.
        assert_eq!(resolved_listeners_for_route(&source), &[old_listener]);
        assert!(Arc::ptr_eq(&prior, &old_snapshot.match_route().unwrap()));
        assert!(Arc::ptr_eq(&source, &prior.route));
        assert_eq!(prior.backend_finder.select().unwrap().name, "test-service");
        let sibling_after = managers.get_route("default/z-sibling").unwrap();
        assert!(Arc::ptr_eq(&sibling, &sibling_after));
        assert!(Arc::ptr_eq(&sibling.backend_finder, &sibling_after.backend_finder));
    }

    #[test]
    fn test_misclassified_add_rebuilds_old_listener() {
        // Simulates upstream misclassification: an existing key is sent in
        // the `add` bucket instead of `update`. Pre-collection of affected
        // listeners must include add.keys() so the old listener is scheduled
        // for rebuild when the route migrates to a new listener.
        let managers = GlobalTcpRouteManagers::new();
        let key9000 = make_listener_key(9000, "TCP");
        let key9001 = make_listener_key(9001, "TCP");

        // Setup: route-x on L9000
        let mut add = HashMap::new();
        add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tcp_route(
                "default",
                "route-x",
                "gw1",
                "tcp-9000",
                9000,
                key9000.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());
        let mgr9000 = managers.get_or_create_listener_manager(&key9000);
        assert!(
            mgr9000.load_route_table().match_route().is_some(),
            "test setup failed: route-x should be in L9000"
        );

        // Misclassified add: same key, new version bound to L9001
        let mut misclassified_add = HashMap::new();
        misclassified_add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tcp_route(
                "default",
                "route-x",
                "gw1",
                "tcp-9001",
                9001,
                key9001.clone(),
            )),
        );
        managers.partial_update(misclassified_add, HashMap::new(), HashSet::new());

        // L9000 must be rebuilt: route-x should be gone
        assert!(
            mgr9000.load_route_table().match_route().is_none(),
            "Bug 2: L9000 still has stale route after misclassified add"
        );
        // L9001 should have the new route (created lazily during partial_update)
        let mgr9001 = managers.get_or_create_listener_manager(&key9001);
        assert!(
            mgr9001.load_route_table().match_route().is_some(),
            "Bug 2 (corollary): L9001 should have the new route"
        );
    }
}
