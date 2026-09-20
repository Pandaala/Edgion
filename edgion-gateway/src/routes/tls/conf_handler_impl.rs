use crate::routes::l4_partial_update::apply_l4_partial_update;
use crate::routes::tls::route_unit::TlsRouteUnit;
use crate::routes::tls::routes_mgr::resolved_listeners_for_route;
use crate::routes::tls::{get_global_tls_route_managers, GlobalTlsRouteManagers};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::TLSRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Create a TLS route handler for registration with ConfigClient.
pub fn create_tls_route_handler() -> Box<dyn ConfHandler<TLSRoute> + Send + Sync> {
    Box::new(get_global_tls_route_managers())
}

impl ConfHandler<TLSRoute> for GlobalTlsRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<TLSRoute>>) {
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        let start_time = Instant::now();
        tracing::info!(
            component = "global_tls_route_managers",
            cnt = data.len(),
            "full set start"
        );

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
                        "Failed to initialize TLSRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_listener_managers_locked();
        tracing::info!(
            component = "global_tls_route_managers",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().replace_routes(
            crate::plugins::publication::inputs::RouteKind::Tls,
            data.iter()
                .map(|(key, route)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::RouteInput::from_tls(route),
                    )
                })
                .collect(),
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<TLSRoute>>,
        update: HashMap<String, Arc<TLSRoute>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, route)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::RouteInput::from_tls(route),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        // Serialize against a concurrent Gateway-driven rebuild_all (see rebuild_lock).
        let _guard = self.lock_rebuild();
        tracing::info!(
            component = "global_tls_route_managers",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        apply_l4_partial_update(
            "TLSRoute",
            add,
            update,
            remove,
            |key| self.get_route(key),
            |u| resolved_listeners_for_route(&u.route),
            |key| self.remove_route(key),
            |route| {
                self.initialize_route(route).map(|unit| {
                    let lks = resolved_listeners_for_route(&unit.route);
                    self.insert_route(unit);
                    lks
                })
            },
            |affected| self.rebuild_affected_listener_managers(affected),
        );
        drop(_guard);
        crate::plugins::publication::global_publisher().update_routes(
            crate::plugins::publication::inputs::RouteKind::Tls,
            policy_inputs,
            &policy_removed,
        );
    }
}

impl GlobalTlsRouteManagers {
    /// Compile a raw TLSRoute schema into its gateway-side [`TlsRouteUnit`].
    ///
    /// Built ONCE per route here (in the conf-handler), then cached and reused
    /// across listener rebuilds (and shared across all SNI hostname buckets), so
    /// a sibling route change on the same listener never resets this route's
    /// `backend_finder` WRR cursor. The compiled runtime (BackendSelector,
    /// proxy-protocol version, connect retry budget, keepalive config,
    /// stream-plugin store key) lives on [`TlsRouteUnit`], not the schema; see
    /// [`TlsRouteUnit::from_route`].
    fn initialize_route(&self, route: Arc<TLSRoute>) -> Result<Arc<TlsRouteUnit>, String> {
        Ok(Arc::new(TlsRouteUnit::from_route(route)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_status_updates_replace_acceptance_with_unchanged_listeners() {
        let managers = GlobalTlsRouteManagers::new();
        let listener = make_listener_key(19443, "TLS");
        let mut route = create_test_tls_route(
            "default",
            "owner-update",
            "gateway",
            "example.com",
            19443,
            listener.clone(),
        );
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
        assert_eq!(original.accepted_attachments.len(), 1);

        // Only the stamped owner changes: the previously foreign rejection now applies.
        route.spec.resolved_status_controller = Some("example.net/owner".into());
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::new(route.clone()))]),
            HashSet::new(),
        );
        assert!(managers.get_route(&key).unwrap().accepted_attachments.is_empty());
        assert_eq!(
            original.accepted_attachments.len(),
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
        assert_eq!(managers.get_route(&key).unwrap().accepted_attachments.len(), 1);
        route.spec.resolved_status_controller = None;
        managers.full_set(&HashMap::from([(key.clone(), Arc::new(route))]));
        assert!(managers.get_route(&key).unwrap().accepted_attachments.is_empty());
        managers.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
        assert!(managers.get_route(&key).is_none());
    }
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::ListenerKey;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tls_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        hostname: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> TLSRoute {
        use edgion_resources::resources::tls_route::*;

        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: gateway.to_string(),
                    section_name: None,
                    port: Some(port as i32),
                }]),
                hostnames: Some(vec![hostname.to_string()]),
                rules: Some(vec![TLSRouteRule {
                    backend_refs: Some(vec![TLSBackendRef {
                        name: "test-service".to_string(),
                        namespace: Some(namespace.to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: Some(vec![ResolvedTlsListener {
                    listener_key,
                    gateway_namespace: namespace.into(),
                    gateway_name: gateway.into(),
                    listener_name: "tls".into(),
                    hostnames: vec![hostname.into()],
                }]),
            },
            status: None,
        }
    }

    #[test]
    fn test_tls_route_full_set() {
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut data = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com", 443, key443.clone());
        let route2 = create_test_tls_route("default", "route2", "gateway1", "api.example.com", 443, key443.clone());

        data.insert("default/route1".to_string(), Arc::new(route1));
        data.insert("default/route2".to_string(), Arc::new(route2));

        managers.full_set(&data);

        let mgr = managers.get_or_create_listener_manager(&key443);
        let table = mgr.load_route_table();
        for (key, schema) in &data {
            assert!(Arc::ptr_eq(&managers.get_route(key).unwrap().route, schema));
        }
        assert!(table.match_route("test.example.com").is_some());
        assert!(table.match_route("api.example.com").is_some());
        assert!(table.match_route("other.example.com").is_none());
    }

    /// Regression for the WRR-cursor-reset blocker, driven through the REAL
    /// ConfHandler path (`full_set` then a sibling `partial_update`). Two
    /// TLSRoutes share one `:443` listener (bucketed by SNI). Adding a sibling
    /// route B must NOT rebuild route A's unit — the data plane must keep
    /// serving the SAME `Arc<TlsRouteUnit>` for A, so A's `backend_finder` WRR
    /// cursor (and load-balancing distribution) is preserved across B's churn.
    #[test]
    fn test_sibling_route_add_preserves_route_a_unit_arc() {
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");
        let mgr = managers.get_or_create_listener_manager(&key443);

        // full_set with only route A on :443.
        let mut full = HashMap::new();
        full.insert(
            "default/route-a".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-a",
                "gw1",
                "a.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.full_set(&full);
        let a_before = mgr.load_route_table().match_route("a.example.com").unwrap();

        // Add a SIBLING route B on the same listener via partial_update.
        let mut add = HashMap::new();
        add.insert(
            "default/route-b".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-b",
                "gw1",
                "b.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());

        let a_after = mgr.load_route_table().match_route("a.example.com").unwrap();
        assert!(
            Arc::ptr_eq(&a_before, &a_after),
            "sibling add must not rebuild route A's unit (WRR cursor must be preserved)"
        );
        assert!(Arc::ptr_eq(&a_before.backend_finder, &a_after.backend_finder));
        assert!(
            mgr.load_route_table().match_route("b.example.com").is_some(),
            "sibling route B must be routable after the add"
        );
    }

    #[test]
    fn test_schema_sharing_preserves_generations_across_listener_migration() {
        let managers = GlobalTlsRouteManagers::new();
        let old_key = make_listener_key(10443, "TLS");
        let new_key = make_listener_key(11443, "TLS");
        let mut original = create_test_tls_route("default", "shared", "gw", "old.example.com", 10443, old_key.clone());
        original.spec.hostnames = Some(vec!["*.example.com".to_string()]);
        original.spec.resolved_listeners.as_mut().unwrap()[0].hostnames =
            vec!["old.example.com".into(), "alias.example.com".into()];
        let original = Arc::new(original);
        managers.partial_update(
            HashMap::from([("default/shared".to_string(), Arc::clone(&original))]),
            HashMap::new(),
            HashSet::new(),
        );
        let old_manager = managers.get_or_create_listener_manager(&old_key);
        let old_table = Arc::clone(&old_manager.load_route_table());
        let old_unit = old_table.match_route("old.example.com").unwrap();
        let alias_unit = old_table.match_route("alias.example.com").unwrap();
        assert!(Arc::ptr_eq(&old_unit.route, &original));
        assert!(Arc::ptr_eq(&old_unit, &alias_unit));
        assert!(Arc::ptr_eq(&old_unit.backend_finder, &alias_unit.backend_finder));
        assert!(old_table.match_route("unresolved.example.com").is_none());
        assert_eq!(old_unit.backend_finder.select().unwrap().name, "test-service");

        let mut replacement =
            create_test_tls_route("default", "shared", "gw", "new.example.com", 11443, new_key.clone());
        replacement.spec.rules.as_mut().unwrap()[0]
            .backend_refs
            .as_mut()
            .unwrap()[0]
            .name = "new-service".to_string();
        let replacement = Arc::new(replacement);
        managers.partial_update(
            HashMap::new(),
            HashMap::from([("default/shared".to_string(), Arc::clone(&replacement))]),
            HashSet::new(),
        );

        // The proxy's retained manager must be cleared, even if its registry entry is removed.
        assert!(old_manager.load_route_table().match_route("old.example.com").is_none());
        assert!(old_manager
            .load_route_table()
            .match_route("alias.example.com")
            .is_none());
        let new_manager = managers.get_or_create_listener_manager(&new_key);
        let new_table = new_manager.load_route_table();
        let new_unit = new_table.match_route("new.example.com").unwrap();
        assert!(Arc::ptr_eq(&new_unit.route, &replacement));
        assert!(Arc::ptr_eq(&managers.get_route("default/shared").unwrap(), &new_unit));
        assert!(!Arc::ptr_eq(&old_unit, &new_unit));
        assert_eq!(new_unit.backend_finder.select().unwrap().name, "new-service");
        assert!(new_table.match_route("old.example.com").is_none());

        // In-flight connections holding the prior snapshot retain the prior schema and runtime.
        assert!(Arc::ptr_eq(
            &old_table.match_route("old.example.com").unwrap(),
            &old_unit
        ));
        assert!(old_table.match_route("new.example.com").is_none());
        assert_eq!(
            original.spec.resolved_listeners.as_ref().unwrap()[0].listener_key,
            old_key
        );
        assert_eq!(old_unit.backend_finder.select().unwrap().name, "test-service");
    }

    #[test]
    fn test_tls_route_partial_update() {
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut add = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com", 443, key443.clone());
        add.insert("default/route1".to_string(), Arc::new(route1));

        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key443);
        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_some());

        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        managers.partial_update(HashMap::new(), HashMap::new(), remove);

        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_none());
    }

    #[test]
    fn test_misclassified_add_rebuilds_old_listener() {
        // Simulates upstream misclassification: an existing key is sent in
        // the `add` bucket. Bug 2: pre-collection of affected listeners
        // excludes keys in the `add` bucket, so L443 is not scheduled for
        // rebuild and retains stale route data.

        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");
        let key8443 = make_listener_key(8443, "TLS");

        // Setup: route-x bound to L443
        let mut add = HashMap::new();
        add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-x",
                "gw1",
                "v1.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());
        let mgr443 = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr443.load_route_table().match_route("v1.example.com").is_some(),
            "test setup failed: v1.example.com should be in L443"
        );

        // Misclassified add: same key, new version bound to L8443
        // (L8443 doesn't exist yet — it will be created lazily)
        let mut misclassified_add = HashMap::new();
        misclassified_add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-x",
                "gw1",
                "v2.example.com",
                8443,
                key8443.clone(),
            )),
        );
        managers.partial_update(misclassified_add, HashMap::new(), HashSet::new());

        // L443 must be rebuilt: v1.example.com should be gone
        assert!(
            mgr443.load_route_table().match_route("v1.example.com").is_none(),
            "Bug 2: L443 still has stale route after misclassified add"
        );
        // L8443 should have the new route (created lazily during partial_update)
        let mgr8443 = managers.get_or_create_listener_manager(&key8443);
        assert!(
            mgr8443.load_route_table().match_route("v2.example.com").is_some(),
            "Bug 2 (corollary): L8443 should have the new route"
        );
    }

    #[test]
    fn test_same_batch_add_remove_same_listener() {
        // Same batch: remove route-a and add route-b on the same listener.
        // Verifies the final state has only route-b (no stale route-a, no
        // missing route-b).
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut add = HashMap::new();
        add.insert(
            "default/route-a".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-a",
                "gw1",
                "old.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Single batch: remove route-a, add route-b on same listener
        let mut add2 = HashMap::new();
        add2.insert(
            "default/route-b".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-b",
                "gw1",
                "new.example.com",
                443,
                key443.clone(),
            )),
        );
        let mut remove = HashSet::new();
        remove.insert("default/route-a".to_string());
        managers.partial_update(add2, HashMap::new(), remove);

        let mgr = managers.get_or_create_listener_manager(&key443);
        let table = mgr.load_route_table();
        assert!(
            table.match_route("new.example.com").is_some(),
            "route-b should be routable"
        );
        assert!(
            table.match_route("old.example.com").is_none(),
            "route-a should be removed"
        );
    }

    #[test]
    fn test_update_listener_migration() {
        // Update moves a route from one listener to another.
        // Both the source (L443) and destination (L9443) listener managers
        // must be rebuilt: L443 loses the route, L9443 gains it.
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");
        let key9443 = make_listener_key(9443, "TLS");

        let mut add = HashMap::new();
        add.insert(
            "default/route-y".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-y",
                "gw1",
                "svc.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Migrate route-y from L443 to L9443
        let mut update = HashMap::new();
        update.insert(
            "default/route-y".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-y",
                "gw1",
                "svc.example.com",
                9443,
                key9443.clone(),
            )),
        );
        managers.partial_update(HashMap::new(), update, HashSet::new());

        let mgr443 = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr443.load_route_table().match_route("svc.example.com").is_none(),
            "L443 should not have route after migration"
        );
        let mgr9443 = managers.get_or_create_listener_manager(&key9443);
        assert!(
            mgr9443.load_route_table().match_route("svc.example.com").is_some(),
            "L9443 should have route after migration"
        );
    }

    #[test]
    fn test_update_nonexistent_key_is_safe() {
        // Updating a key that doesn't exist in cache (remove_cached is a no-op,
        // init_and_insert runs, new route is inserted). Verifies no panic and
        // the new route appears in the target listener.
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        // Update a key that was never added — behaves like an add
        let mut update = HashMap::new();
        update.insert(
            "default/route-ghost".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-ghost",
                "gw1",
                "ghost.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(HashMap::new(), update, HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr.load_route_table().match_route("ghost.example.com").is_some(),
            "update of nonexistent key should insert the new route"
        );
    }
}
