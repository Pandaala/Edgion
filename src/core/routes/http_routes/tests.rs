#[cfg(test)]
mod route_matching_tests {
    use crate::core::routes::http_routes::routes_mgr::RouteManager;
    use crate::core::routes::http_routes::routes_mgr::DomainRouteRules;
    use crate::core::gateway::gateway_store::get_global_gateway_store;
    use crate::core::conf_sync::traits::ConfHandler;
    use crate::types::{Gateway, HTTPRoute};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    /// Helper function to create a test Gateway from JSON
    fn create_test_gateway(namespace: &str, name: &str, hostnames: Vec<&str>) -> Gateway {
        let listeners_json: Vec<serde_json::Value> = hostnames.iter().map(|h| {
            serde_json::json!({
                "name": format!("listener-{}", h.replace(".", "-")),
                "hostname": h,
                "port": 80,
                "protocol": "HTTP"
            })
        }).collect();
        
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
        
        serde_json::from_value(json).expect("Failed to create Gateway")
    }
    
    /// Helper function to create a test HTTPRoute with path matches
    fn create_test_httproute_with_paths(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>, // (namespace, name)
        paths: Vec<(&str, &str)>, // (match_type, path_value)
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs.iter().map(|(ns, n)| {
            serde_json::json!({
                "group": "gateway.networking.k8s.io",
                "kind": "Gateway",
                "namespace": ns,
                "name": n
            })
        }).collect();
        
        let matches_json: Vec<serde_json::Value> = paths.iter().map(|(match_type, path_value)| {
            serde_json::json!({
                "path": {
                    "type": match_type,
                    "value": path_value
                }
            })
        }).collect();
        
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "parentRefs": parent_refs_json,
                "hostnames": hostnames,
                "rules": [{
                    "matches": matches_json,
                    "backendRefs": [{
                        "name": format!("{}-lb", name),
                        "port": 8080
                    }]
                }]
            }
        });
        
        serde_json::from_value(json).expect("Failed to create HTTPRoute")
    }

    /// Helper function to create a test HTTPRoute with regex path matches
    fn create_test_httproute_with_regex(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>, // (namespace, name)
        regex_paths: Vec<&str>, // regex pattern strings
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs.iter().map(|(ns, n)| {
            serde_json::json!({
                "group": "gateway.networking.k8s.io",
                "kind": "Gateway",
                "namespace": ns,
                "name": n
            })
        }).collect();
        
        let matches_json: Vec<serde_json::Value> = regex_paths.iter().map(|regex_pattern| {
            serde_json::json!({
                "path": {
                    "type": "RegularExpression",
                    "value": regex_pattern
                }
            })
        }).collect();
        
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "parentRefs": parent_refs_json,
                "hostnames": hostnames,
                "rules": [{
                    "matches": matches_json,
                    "backendRefs": [{
                        "name": format!("{}-lb", name),
                        "port": 8080
                    }]
                }]
            }
        });
        
        serde_json::from_value(json).expect("Failed to create HTTPRoute")
    }


    /// Helper function to verify route matching without Session
    /// Instead, we'll test the internal state of RouteRules
    fn verify_route_exists(
        domain_routes: &Arc<DomainRouteRules>,
        hostname: &str,
        should_exist: bool,
    ) {
        let domain_routes_map = domain_routes.domain_routes_map.load();
        let exists = domain_routes_map.contains_key(hostname);
        assert_eq!(exists, should_exist, "RouteRules for hostname '{}' should {}exist", hostname, if should_exist { "" } else { "not " });
    }

    /// Helper function to verify route count
    fn verify_route_count(
        domain_routes: &Arc<DomainRouteRules>,
        hostname: &str,
        expected_normal_routes: usize,
        expected_regex_routes: usize,
    ) {
        let domain_routes_map = domain_routes.domain_routes_map.load();
        if let Some(route_rules) = domain_routes_map.get(hostname) {
            let normal_count = route_rules.route_rules_list.read().unwrap().len();
            let regex_count = route_rules.regex_routes.read().unwrap().len();
            assert_eq!(normal_count, expected_normal_routes, "Expected {} normal routes for hostname '{}', got {}", expected_normal_routes, hostname, normal_count);
            assert_eq!(regex_count, expected_regex_routes, "Expected {} regex routes for hostname '{}', got {}", expected_regex_routes, hostname, regex_count);
            
            // Verify match_engine and regex_routes_engine are set correctly
            if expected_normal_routes > 0 {
                assert!(route_rules.match_engine.is_some(), "match_engine should exist when there are normal routes");
            } else {
                assert!(route_rules.match_engine.is_none(), "match_engine should be None when there are no normal routes");
            }
            
            if expected_regex_routes > 0 {
                assert!(route_rules.regex_routes_engine.is_some(), "regex_routes_engine should exist when there are regex routes");
            } else {
                assert!(route_rules.regex_routes_engine.is_none(), "regex_routes_engine should be None when there are no regex routes");
            }
        } else {
            panic!("RouteRules not found for hostname '{}'", hostname);
        }
    }

    #[test]
    fn test_full_set_with_normal_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with normal paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_with_regex_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        data.insert("default/route1".to_string(), route1);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_full_set_with_mixed_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with both normal and regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }

    #[test]
    fn test_partial_update_add_route() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Initially no routes
        verify_route_exists(&domain_routes, "api.example.com", false);
        
        // Add a route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_remove_route() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add a route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com", "api1.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add a route with hostname api.example.com
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_exists(&domain_routes, "api1.example.com", false);
        
        // Update route to use api1.example.com instead
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api1.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify old hostname was cleaned up and new hostname has route
        verify_route_exists(&domain_routes, "api.example.com", false);
        verify_route_exists(&domain_routes, "api1.example.com", true);
    }

    #[test]
    fn test_partial_update_remove_hostname_from_route() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add a route with hostname api.example.com
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_exists(&domain_routes, "api.example.com", true);
        
        // Update route to remove hostname (empty hostnames)
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default", "route1",
            vec![], // Empty hostnames
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify hostname was cleaned up (route no longer applies to this hostname)
        verify_route_exists(&domain_routes, "api.example.com", false);
    }

    #[test]
    fn test_partial_update_multiple_routes_same_hostname() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add first route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
        
        // Add second route to same hostname
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
        
        // Remove first route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_regex_only_to_normal_only() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
        
        // Update to normal route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_remove_last_route_cleans_hostname() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with only Exact paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_only_prefix_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with only PathPrefix paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_full_set_mixed_exact_and_prefix_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with both Exact and PathPrefix paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/v1")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist (route1 has 2 matches, route2 has 1 match)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 3, 0);
    }

    #[test]
    fn test_full_set_only_regex_routes_multiple() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with multiple regex paths
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$", r"^/api/v\d+/posts$"],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/\d+/items$"],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist (route1 has 2 regex matches, route2 has 1)
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 3);
    }

    #[test]
    fn test_full_set_all_types_mixed() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes with all types: Exact, PathPrefix, and Regex
        let mut data = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        let route3 = create_test_httproute_with_paths(
            "default", "route3",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/v1")],
        );
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        data.insert("default/route3".to_string(), route3);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes exist
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 3, 1);
    }

    #[test]
    fn test_partial_update_add_prefix_route() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add a prefix route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_add_regex_route() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add a regex route via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify route was added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_partial_update_add_mixed_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add both normal and regex routes via partial_update
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users"), ("PathPrefix", "/api/posts")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        // Verify routes were added
        verify_route_exists(&domain_routes, "api.example.com", true);
        verify_route_count(&domain_routes, "api.example.com", 2, 1);
    }

    #[test]
    fn test_partial_update_remove_normal_keep_regex() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add both normal and regex routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add prefix route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
        
        // Update to exact route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
    }

    #[test]
    fn test_partial_update_change_from_normal_to_regex() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add normal route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
        
        // Update to regex route
        let mut add_or_update = HashMap::new();
        let route1_updated = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1_updated);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
    }

    #[test]
    fn test_partial_update_add_multiple_regex_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add first regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
        
        // Add second regex route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 0, 2);
    }

    #[test]
    fn test_partial_update_add_multiple_prefix_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add first prefix route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
        
        // Add second prefix route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 2, 0);
    }

    #[test]
    fn test_partial_update_remove_all_routes_cleans_hostname() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Add multiple routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("PathPrefix", "/api/posts")],
        );
        let route3 = create_test_httproute_with_regex(
            "default", "route3",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/items$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        add_or_update.insert("default/route3".to_string(), route3);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
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
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add regex route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_regex(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/users$"],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 0, 1);
        
        // Then add normal route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_paths(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/posts")],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }

    #[test]
    fn test_partial_update_add_normal_then_add_regex() {
        let mgr = RouteManager::new();
        
        // Setup: Create gateway and add to store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // First add normal route
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute_with_paths(
            "default", "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![("Exact", "/api/users")],
        );
        add_or_update.insert("default/route1".to_string(), route1);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 0);
        
        // Then add regex route
        let mut add_or_update = HashMap::new();
        let route2 = create_test_httproute_with_regex(
            "default", "route2",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
            vec![r"^/api/v\d+/posts$"],
        );
        add_or_update.insert("default/route2".to_string(), route2);
        mgr.partial_update(add_or_update, HashMap::new(), HashSet::new());
        
        verify_route_count(&domain_routes, "api.example.com", 1, 1);
    }
}

