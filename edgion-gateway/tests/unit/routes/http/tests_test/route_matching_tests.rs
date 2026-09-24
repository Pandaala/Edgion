    use crate::routes::http::conf_handler_impl::HttpRouteChanges;
    use crate::routes::http::routes_mgr::DomainRouteRules;
    use crate::routes::http::routes_mgr::{GlobalHttpRouteManagers, HttpPortRouteManager};
    use crate::runtime::store::get_global_gateway_store;
    use edgion_algo::matcher::radix_hostname::RadixHostnamePattern;
    use edgion_algo::matcher::radix_hostname::RadixHostnameQuery;
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::{Gateway, HTTPRoute, ListenerKey};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn contract_route(name: &str, hostnames: Vec<&str>, path: &str) -> HTTPRoute {
        create_test_httproute_with_paths(
            "hostname-contract",
            name,
            hostnames,
            vec![("hostname-contract", "shared")],
            vec![("Exact", path)],
        )
    }

    fn contract_index(
        listeners: &[(&str, Option<&str>)],
    ) -> crate::runtime::store::listener_gateway_info::GatewayInfoIndex {
        use crate::runtime::gateway_info::GatewayInfo;
        use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
        GatewayInfoIndex::from_entries_for_test(
            listeners
                .iter()
                .map(|(name, hostname)| {
                    (
                        format!("hostname-contract/{name}"),
                        ListenerEntry {
                            port: 8080,
                            info: Arc::new(GatewayInfo::new(
                                Some("hostname-contract".into()),
                                (*name).into(),
                                Some("web".into()),
                                None,
                                None,
                                None,
                                Vec::new(),
                                None,
                            )),
                            gateway_proof: None,
                            hostname: hostname.map(str::to_owned),
                            allowed_routes: None,
                        },
                    )
                })
                .collect(),
        )
    }

    async fn match_contract(
        table: &DomainRouteRules,
        hostname: &str,
        path: &str,
        index: &crate::runtime::store::listener_gateway_info::GatewayInfoIndex,
    ) -> Option<(String, String)> {
        use crate::ctx::{EdgionHttpContext, NormalizedHostname};
        use edgion_resources::EdError;
        use tokio::io::AsyncWriteExt;
        let (mut client, server) = tokio::io::duplex(4096);
        let mut session = pingora_proxy::Session::new_h1(Box::new(server));
        client
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: {hostname}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        drop(client);
        assert!(session.read_request().await.unwrap());
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.hostname = NormalizedHostname::new(hostname);
        ctx.request_info.entry_raw_path = path.to_string();
        ctx.request_info.normalized_current_path = path.to_string();
        match table.match_route(&mut session, &ctx, index) {
            Ok(result) => Some((
                result.route_unit.resource_key.clone(),
                result.matched_gateway.gateway_key().to_owned(),
            )),
            Err(EdError::RouteNotFound()) => None,
            Err(error) => panic!("unexpected terminal route error: {error:?}"),
        }
    }

    async fn match_contract_route(
        table: &DomainRouteRules,
        hostname: &str,
        path: &str,
        index: &crate::runtime::store::listener_gateway_info::GatewayInfoIndex,
    ) -> Option<String> {
        match_contract(table, hostname, path, index)
            .await
            .map(|(route, _)| route)
    }

    fn contract_table(routes: Vec<HTTPRoute>) -> DomainRouteRules {
        use crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes;
        let data = routes
            .into_iter()
            .map(|route| {
                (
                    format!("hostname-contract/{}", route.metadata.name.as_deref().unwrap()),
                    Arc::new(route),
                )
            })
            .collect();
        build_domain_route_rules_from_routes(&data, &DomainRouteRules::new())
    }

    #[test]
    fn duplicate_hostnames_share_one_unit_and_invalid_lists_never_publish() {
        let table = contract_table(vec![
            contract_route("duplicate", vec!["api.example.com", "api.example.com"], "/"),
            contract_route("invalid", vec!["other.example.com", "*"], "/"),
            contract_route("too-many", vec!["excess.example.com"; 17], "/"),
        ]);
        assert_eq!(table.hosts.len(), 1);
        let rules = table
            .hosts
            .get(&RadixHostnamePattern::parse("api.example.com").unwrap())
            .unwrap();
        assert_eq!(rules.route_rules_list.len(), 1);
        assert!(table.catch_all_routes.is_none());
    }

    #[tokio::test]
    async fn hostname_path_misses_continue_through_all_wildcards_and_hostless() {
        let table = contract_table(vec![
            contract_route("exact", vec!["api.sub.example.com"], "/exact"),
            contract_route("narrow", vec!["*.sub.example.com"], "/narrow"),
            contract_route("wide", vec!["*.example.com"], "/wide"),
            contract_route("hostless", vec![], "/hostless"),
        ]);
        let index = contract_index(&[("shared", None)]);
        for (path, route) in [
            ("/exact", "exact"),
            ("/narrow", "narrow"),
            ("/wide", "wide"),
            ("/hostless", "hostless"),
        ] {
            assert_eq!(
                match_contract_route(&table, "api.sub.example.com", path, &index).await,
                Some(format!("hostname-contract/{route}"))
            );
        }
        assert_eq!(match_contract_route(&table, "example.com", "/wide", &index).await, None);
        assert_eq!(
            match_contract_route(&table, "badexample.com", "/wide", &index).await,
            None
        );
        assert_eq!(
            match_contract_route(&table, "127.0.0.1", "/hostless", &index).await,
            Some("hostname-contract/hostless".into())
        );
    }

    #[tokio::test]
    async fn listener_intersection_does_not_promote_wildcard_route_priority() {
        let mut wildcard = contract_route("a-older-wildcard", vec!["*.example.com"], "/");
        wildcard.spec.resolved_hostnames = Some(vec!["api.example.com".into()]);
        let mut exact = contract_route("z-exact", vec!["api.example.com"], "/");
        exact.spec.resolved_hostnames = Some(vec!["api.example.com".into()]);
        let table = contract_table(vec![wildcard, exact]);
        let index = contract_index(&[("shared", Some("api.example.com"))]);
        assert_eq!(
            match_contract_route(&table, "api.example.com", "/", &index).await,
            Some("hostname-contract/z-exact".into())
        );
        assert_eq!(
            match_contract_route(&table, "other.example.com", "/", &index).await,
            None
        );
    }

    #[tokio::test]
    async fn empty_exact_listener_in_other_gateway_blocks_wider_route() {
        let table = contract_table(vec![contract_route("wide", vec!["*.example.com"], "/")]);
        let index = contract_index(&[
            ("shared", Some("*.example.com")),
            ("empty-exact", Some("api.example.com")),
        ]);
        assert_eq!(match_contract_route(&table, "api.example.com", "/", &index).await, None);
        assert_eq!(
            match_contract_route(&table, "other.example.com", "/", &index).await,
            Some("hostname-contract/wide".into())
        );
    }

    #[tokio::test]
    async fn listener_specificity_wins_before_the_routes_declared_hostname_priority() {
        let exact_route = contract_route("exact-route-on-wildcard", vec!["api.example.com"], "/");
        let mut wildcard_route = contract_route("wildcard-route-on-exact", vec!["*.example.com"], "/");
        wildcard_route.spec.parent_refs.as_mut().unwrap()[0].name = "exact-owner".into();
        wildcard_route.spec.current_status.as_mut().unwrap().parents[0]
            .parent_ref
            .name = "exact-owner".into();
        wildcard_route.spec.resolved_hostnames = Some(vec!["api.example.com".into()]);
        let table = contract_table(vec![exact_route, wildcard_route]);
        let broad_only = contract_index(&[("shared", Some("*.example.com"))]);
        assert_eq!(
            match_contract(&table, "api.example.com", "/", &broad_only).await,
            Some((
                "hostname-contract/exact-route-on-wildcard".into(),
                "hostname-contract/shared".into()
            ))
        );
        let both = contract_index(&[
            ("shared", Some("*.example.com")),
            ("exact-owner", Some("api.example.com")),
        ]);
        assert_eq!(
            match_contract(&table, "api.example.com", "/", &both).await,
            Some((
                "hostname-contract/wildcard-route-on-exact".into(),
                "hostname-contract/exact-owner".into()
            ))
        );
    }

    #[test]
    fn full_set_and_gateway_rebuild_reuse_unchanged_hostname_tree() {
        let managers = GlobalHttpRouteManagers::new();
        let manager = managers.get_global_routes();
        let first = HashMap::from([
            (
                "hostname-contract/exact".into(),
                Arc::new(contract_route("exact", vec!["api.example.com"], "/old")),
            ),
            (
                "hostname-contract/wildcard".into(),
                Arc::new(contract_route("wildcard", vec!["*.example.com"], "/wide")),
            ),
        ]);
        managers.full_set(&first);
        let before = Arc::clone(&manager.load_route_table());
        let replacement = HashMap::from([
            (
                "hostname-contract/exact".into(),
                Arc::new(contract_route("exact", vec!["api.example.com"], "/new")),
            ),
            (
                "hostname-contract/wildcard".into(),
                Arc::new(contract_route("wildcard", vec!["*.example.com"], "/wide")),
            ),
        ]);
        managers.full_set(&replacement);
        let replaced = Arc::clone(&manager.load_route_table());
        assert!(!Arc::ptr_eq(&before, &replaced));
        assert!(before.hosts.shares_hosts(&replaced.hosts));
        let exact = RadixHostnamePattern::parse("api.example.com").unwrap();
        assert!(!Arc::ptr_eq(
            before.hosts.get(&exact).unwrap(),
            replaced.hosts.get(&exact).unwrap()
        ));
        managers.rebuild_all_listener_managers();
        let rebuilt = Arc::clone(&manager.load_route_table());
        assert!(!Arc::ptr_eq(&replaced, &rebuilt));
        assert!(before.hosts.shares_hosts(&rebuilt.hosts));
        assert_eq!(rebuilt.hosts.len(), 2);
    }

    #[tokio::test]
    async fn hostname_delta_preserves_untouched_paths_units_and_old_snapshots() {
        let managers = GlobalHttpRouteManagers::new();
        let manager = managers.get_global_routes();
        let first = contract_route("updated", vec!["api.example.com"], "/old");
        let sibling = contract_route("sibling", vec!["api.example.com"], "/sibling");
        let cold = contract_route("cold", vec!["cold.example.com"], "/cold");
        managers.full_set(&HashMap::from([
            ("hostname-contract/updated".into(), Arc::new(first)),
            ("hostname-contract/sibling".into(), Arc::new(sibling)),
            ("hostname-contract/cold".into(), Arc::new(cold)),
        ]));
        let old = Arc::clone(&manager.load_route_table());
        let api = RadixHostnamePattern::parse("api.example.com").unwrap();
        let cold = RadixHostnamePattern::parse("cold.example.com").unwrap();
        let old_sibling = old
            .hosts
            .get(&api)
            .unwrap()
            .route_rules_list
            .iter()
            .find(|unit| unit.resource_key == "hostname-contract/sibling")
            .unwrap()
            .clone();
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(
                "hostname-contract/updated".into(),
                Arc::new(contract_route("updated", vec!["api.example.com"], "/new")),
            )]),
            HashSet::new(),
        );
        let changed = Arc::clone(&manager.load_route_table());
        assert!(old.hosts.shares_hosts(&changed.hosts));
        assert!(Arc::ptr_eq(
            old.hosts.get(&cold).unwrap(),
            changed.hosts.get(&cold).unwrap()
        ));
        assert!(Arc::ptr_eq(
            old.hosts.get(&cold).unwrap().match_engine.as_ref().unwrap(),
            changed.hosts.get(&cold).unwrap().match_engine.as_ref().unwrap()
        ));
        let sibling = changed
            .hosts
            .get(&api)
            .unwrap()
            .route_rules_list
            .iter()
            .find(|unit| unit.resource_key == "hostname-contract/sibling")
            .unwrap();
        assert!(Arc::ptr_eq(&old_sibling, sibling));
        assert!(Arc::ptr_eq(&old_sibling.backend_finder, &sibling.backend_finder));
        let index = contract_index(&[("shared", None)]);
        assert_eq!(
            match_contract_route(&old, "api.example.com", "/old", &index).await,
            Some("hostname-contract/updated".into())
        );
        assert_eq!(
            match_contract_route(&changed, "api.example.com", "/old", &index).await,
            None
        );
        assert_eq!(
            match_contract_route(&changed, "api.example.com", "/new", &index).await,
            Some("hostname-contract/updated".into())
        );

        managers.partial_update(
            HashMap::new(),
            HashMap::from([(
                "hostname-contract/updated".into(),
                Arc::new(contract_route("updated", vec!["new.example.com"], "/new")),
            )]),
            HashSet::new(),
        );
        let moved = Arc::clone(&manager.load_route_table());
        assert!(!changed.hosts.shares_hosts(&moved.hosts));
        assert!(Arc::ptr_eq(
            changed.hosts.get(&cold).unwrap(),
            moved.hosts.get(&cold).unwrap()
        ));
        assert_eq!(
            match_contract_route(&moved, "api.example.com", "/new", &index).await,
            None
        );
        assert_eq!(
            match_contract_route(&moved, "new.example.com", "/new", &index).await,
            Some("hostname-contract/updated".into())
        );
        managers.partial_update(
            HashMap::new(),
            HashMap::from([(
                "hostname-contract/updated".into(),
                Arc::new(contract_route("updated", vec!["new.example.com", "*"], "/new")),
            )]),
            HashSet::new(),
        );
        let rejected = manager.load_route_table();
        assert!(rejected
            .hosts
            .get(&RadixHostnamePattern::parse("new.example.com").unwrap())
            .is_none());
        assert!(rejected.catch_all_routes.is_none());
        assert_eq!(
            match_contract_route(&moved, "new.example.com", "/new", &index).await,
            Some("hostname-contract/updated".into())
        );
    }

    /// Helper: wrap HTTPRoute values in Arc for ConfHandler API
    fn wrap_arc(data: HashMap<String, HTTPRoute>) -> HashMap<String, Arc<HTTPRoute>> {
        data.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
    }

    /// Helper function to create a test Gateway from JSON
    fn create_test_gateway(namespace: &str, name: &str, hostnames: Vec<&str>) -> Gateway {
        let listeners_json: Vec<serde_json::Value> = hostnames
            .iter()
            .map(|h| {
                serde_json::json!({
                    "name": format!("listener-{}", h.replace(".", "-")),
                    "hostname": h,
                    "port": 80,
                    "protocol": "HTTP"
                })
            })
            .collect();

        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "Gateway",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "gatewayClassName": "test-class",
                "listeners": listeners_json
            }
        });

        crate::test_current_status::from_value(json).expect("Failed to create Gateway")
    }

    /// Helper function to create a test HTTPRoute with path matches
    fn create_test_httproute_with_paths(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>, // (namespace, name)
        paths: Vec<(&str, &str)>,        // (match_type, path_value)
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs
            .iter()
            .map(|(ns, n)| {
                serde_json::json!({
                    "group": "gateway.networking.k8s.io",
                    "kind": "Gateway",
                    "namespace": ns,
                    "name": n
                })
            })
            .collect();

        let matches_json: Vec<serde_json::Value> = paths
            .iter()
            .map(|(match_type, path_value)| {
                serde_json::json!({
                    "path": {
                        "type": match_type,
                        "value": path_value
                    }
                })
            })
            .collect();

        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": parent_refs_json,
                "hostnames": hostnames,
                "resolvedListeners": ["0.0.0.0:80/HTTP"],
                "rules": [{
                    "matches": matches_json,
                    "backendRefs": [{
                        "name": format!("{}-lb", name),
                        "port": 8080
                    }]
                }]
            }
        });

        crate::test_current_status::from_value(json).expect("Failed to create HTTPRoute")
    }

    /// Helper function to create a test HTTPRoute with regex path matches
    fn create_test_httproute_with_regex(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>, // (namespace, name)
        regex_paths: Vec<&str>,          // regex pattern strings
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs
            .iter()
            .map(|(ns, n)| {
                serde_json::json!({
                    "group": "gateway.networking.k8s.io",
                    "kind": "Gateway",
                    "namespace": ns,
                    "name": n
                })
            })
            .collect();

        let matches_json: Vec<serde_json::Value> = regex_paths
            .iter()
            .map(|regex_pattern| {
                serde_json::json!({
                    "path": {
                        "type": "RegularExpression",
                        "value": regex_pattern
                    }
                })
            })
            .collect();

        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": parent_refs_json,
                "hostnames": hostnames,
                "resolvedListeners": ["0.0.0.0:80/HTTP"],
                "rules": [{
                    "matches": matches_json,
                    "backendRefs": [{
                        "name": format!("{}-lb", name),
                        "port": 8080
                    }]
                }]
            }
        });

        crate::test_current_status::from_value(json).expect("Failed to create HTTPRoute")
    }

    /// Helper function to verify route matching without Session
    /// Instead, we'll test the internal state of RouteRules
    fn verify_route_exists(listener_manager: &Arc<HttpPortRouteManager>, hostname: &str, should_exist: bool) {
        let domain_routes = listener_manager.load_route_table();
        let hostname = hostname.to_ascii_lowercase();
        let exists = RadixHostnameQuery::new(&hostname)
            .is_ok_and(|query| domain_routes.hosts.matching_values(query).next().is_some())
            || domain_routes.catch_all_routes.is_some();
        assert_eq!(
            exists,
            should_exist,
            "RouteRules for hostname '{}' should {}exist",
            hostname,
            if should_exist { "" } else { "not " }
        );
    }

    /// Helper function to verify route count
    fn verify_route_count(
        listener_manager: &Arc<HttpPortRouteManager>,
        hostname: &str,
        expected_normal_routes: usize,
        expected_regex_routes: usize,
    ) {
        let domain_routes = listener_manager.load_route_table();
        let hostname = hostname.to_ascii_lowercase();
        let route_rules = RadixHostnameQuery::new(&hostname)
            .ok()
            .and_then(|query| domain_routes.hosts.matching_values(query).next())
            .or(domain_routes.catch_all_routes.as_deref());

        if let Some(route_rules) = route_rules {
            let normal_count = route_rules.route_rules_list.len();
            let regex_count = route_rules.regex_routes.len();
            assert_eq!(
                normal_count, expected_normal_routes,
                "Expected {} normal routes for hostname '{}', got {}",
                expected_normal_routes, hostname, normal_count
            );
            assert_eq!(
                regex_count, expected_regex_routes,
                "Expected {} regex routes for hostname '{}', got {}",
                expected_regex_routes, hostname, regex_count
            );

            // Verify match_engine and regex_routes_engine are set correctly
            if expected_normal_routes > 0 {
                assert!(
                    route_rules.match_engine.is_some(),
                    "match_engine should exist when there are normal routes"
                );
            } else {
                assert!(
                    route_rules.match_engine.is_none(),
                    "match_engine should be None when there are no normal routes"
                );
            }

            if expected_regex_routes > 0 {
                assert!(
                    route_rules.regex_routes_engine.is_some(),
                    "regex_routes_engine should exist when there are regex routes"
                );
            } else {
                assert!(
                    route_rules.regex_routes_engine.is_none(),
                    "regex_routes_engine should be None when there are no regex routes"
                );
            }
        } else {
            panic!("RouteRules not found for hostname '{}'", hostname);
        }
    }

    fn make_listener_key(port: u16) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, "HTTP")
    }

    fn seed_nonempty_manager(
        managers: &GlobalHttpRouteManagers,
        listener_key: &ListenerKey,
    ) -> Arc<HttpPortRouteManager> {
        let manager = managers.get_or_create_listener_manager(listener_key);
        let mut route = create_test_httproute_with_paths(
            "retained-test",
            "prior",
            vec!["retained.example.com"],
            vec![("retained-test", "gateway")],
            vec![("Exact", "/prior")],
        );
        route.spec.resolved_listeners = Some(vec![listener_key.clone()]);
        let routes = HashMap::from([("retained-test/prior".to_string(), Arc::new(route))]);
        manager.store_route_table(
            crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(
                &routes,
                &DomainRouteRules::new(),
            ),
        );
        assert!(manager
            .load_route_table()
            .hosts
            .get(&RadixHostnamePattern::parse("retained.example.com").unwrap())
            .is_some());
        manager
    }

    #[test]
    fn test_full_set_with_normal_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with normal paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_with_regex_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        data.insert("default/route1".to_string(), route1);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_full_set_with_mixed_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with both normal and regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }

    #[test]
    fn test_partial_update_add_route() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Initially no routes
        verify_route_exists(&domain_routes, "api.example.com", false);

        // Add a route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);

        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_remove_route() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add a route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_exists(&domain_routes, "api.example.com", true);

        // Remove the route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify route was removed and hostname cleaned up
        verify_route_exists(&domain_routes, "api.example.com", false);
    }

    #[test]
    fn test_partial_update_change_hostname() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com", "api1.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add a route with hostname api.example.com
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_exists(&domain_routes, "api1.example.com", false);

        // Update route to use api1.example.com instead
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api1.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Verify old hostname was cleaned up and new hostname has route
        verify_route_exists(&domain_routes, "api.example.com", false);
        verify_route_exists(&domain_routes, "api1.example.com", true);
    }

    #[test]
    fn test_partial_update_remove_hostname_from_route() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add a route with hostname api.example.com
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_exists(&domain_routes, "api.example.com", true);

        // Update route to remove explicit hostname (empty hostnames).
        // The controller resolves effective hostnames into resolved_hostnames;
        // simulate that behavior here.
        let mut add_or_update = HashMap::new();
        let mut route1_updated = create_test_httproute_with_paths(
            "default",
            "route1",
            vec![], // Empty hostnames
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        // Controller sets resolved_hostnames to the listener's hostname
        route1_updated.spec.resolved_hostnames = Some(vec!["api.example.com".to_string()]);
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Route should still exist at api.example.com (resolved by controller)
        verify_route_exists(&domain_routes, "api.example.com", true);
    }

    #[test]
    fn test_partial_update_multiple_routes_same_hostname() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add first route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);

        // Add second route to same hostname
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 2, 0);

        // Remove first route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_regex_only_to_normal_only() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 0, 1);

        // Update to normal route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_remove_last_route_cleans_hostname() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_exists(&domain_routes, "api.example.com", true);

        // Remove route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname was cleaned up
        verify_route_exists(&domain_routes, "api.example.com", false);
    }

    #[test]
    fn test_partial_update_regex_and_normal_both_empty_removes_hostname() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);

        // Remove both routes
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        remove.insert("default/route2".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname was cleaned up (both routes removed)
        verify_route_exists(&domain_routes, "api.example.com", false);
    }

    #[test]
    fn test_partial_update_keep_regex_remove_normal() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);

        // Remove only normal route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname still exists (regex route remains)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_full_set_only_exact_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with only Exact paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_only_prefix_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with only PathPrefix paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_mixed_exact_and_prefix_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with both Exact and PathPrefix paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/v1")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist (route1 has 2 matches, route2 has 1 match)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 3, 0);
    }

    #[test]
    fn test_full_set_only_regex_routes_multiple() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with multiple regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$", r"^/api/v\d+/posts$"],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/\d+/items$"],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist (route1 has 2 regex matches, route2 has 1)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 3);
    }

    #[test]
    fn test_full_set_all_types_mixed() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Create test routes with all types: Exact, PathPrefix, and Regex
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        let route3 = create_test_httproute_with_paths(
            "default",
            "route3",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/v1")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        data.insert("default/route3".to_string(), route3);

        // Execute full_set
        let data = wrap_arc(data);
        mgr.full_set(&data);

        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 3, 1);
    }

    #[test]
    fn test_partial_update_add_prefix_route() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add a prefix route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route1".to_string(), route1);

        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_add_regex_route() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add a regex route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);

        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_partial_update_add_mixed_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add both normal and regex routes via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);

        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        // Verify routes were added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 1);
    }

    #[test]
    fn test_partial_update_remove_normal_keep_regex() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);

        // Remove only normal route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname still exists (regex route remains)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_partial_update_remove_regex_keep_normal() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);

        // Remove only regex route
        let mut remove = HashSet::new();
        remove.insert("default/route2".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname still exists (normal route remains)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_change_from_prefix_to_exact() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add prefix route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);

        // Update to exact route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_change_from_normal_to_regex() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add normal route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);

        // Update to regex route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_partial_update_add_multiple_regex_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add first regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 0, 1);

        // Add second regex route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 0, 2);
    }

    #[test]
    fn test_partial_update_add_multiple_prefix_routes() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add first prefix route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);

        // Add second prefix route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_partial_update_remove_all_routes_cleans_hostname() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // Add multiple routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        let route3 = create_test_httproute_with_regex(
            "default",
            "route3",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        add_or_update.insert("default/route3".to_string(), route3);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 2, 1);

        // Remove all routes
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        remove.insert("default/route2".to_string());
        remove.insert("default/route3".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify hostname was cleaned up
        verify_route_exists(&domain_routes, "api.example.com", false);
    }

    #[test]
    fn test_partial_update_add_regex_then_add_normal() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 0, 1);

        // Then add normal route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }

    #[test]
    fn test_partial_update_add_normal_then_add_regex() {
        let mgr = GlobalHttpRouteManagers::new();

        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_global_routes();
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write();
            let _ = store_guard.add_gateway(gateway);
        }

        // First add normal route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 0);

        // Then add regex route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_regex(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(wrap_arc(add_or_update), HashMap::new(), HashSet::new());

        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }

    #[test]
    fn rebuild_all_clears_a_nonempty_retained_manager_before_drop() {
        let managers = GlobalHttpRouteManagers::new();
        let stale = make_listener_key(9810);
        let retained = seed_nonempty_manager(&managers, &stale);

        managers.rebuild_all_listener_managers_with_active(Vec::new());

        assert!(!managers.by_listener.contains_key(&stale));
        assert!(retained.load_route_table().hosts.is_empty());
        let recreated = managers.get_or_create_listener_manager(&stale);
        assert!(!Arc::ptr_eq(&retained, &recreated));
    }

    #[test]
    fn affected_orphan_sweep_clears_a_nonempty_retained_manager_before_drop() {
        let managers = GlobalHttpRouteManagers::new();
        let affected = make_listener_key(9820);
        let orphan = make_listener_key(9821);
        let retained = seed_nonempty_manager(&managers, &orphan);
        let affected_set = HashSet::from([affected.clone()]);

        managers.rebuild_affected_listener_managers_with_active(
            &affected_set,
            vec![affected],
            &HttpRouteChanges::default(),
        );

        assert!(!managers.by_listener.contains_key(&orphan));
        assert!(retained.load_route_table().hosts.is_empty());
        let recreated = managers.get_or_create_listener_manager(&orphan);
        assert!(!Arc::ptr_eq(&retained, &recreated));
    }
