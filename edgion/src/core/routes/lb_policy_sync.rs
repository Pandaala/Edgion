//! Load balancing policy synchronization for HTTPRoute
//! 
//! This module handles extracting LB policies from HTTPRoute resources
//! and synchronizing them with the global policy store.

use std::collections::{HashMap, HashSet};
use crate::core::lb::optional_lb::{get_global_policy_store, LbPolicy};
use crate::types::{HTTPRoute, HTTPRouteFilterType};

/// Extract load balancing policies from HTTPRoute and update the global policy store
/// 
/// This function iterates through all rules and filters in the HTTPRoute,
/// extracts LB policy configurations from extension_ref, and updates the global policy store.
/// 
/// The extension_ref.name contains the algorithms in format:
/// "algorithm1,algorithm2,..."
/// For example: "ketama" or "ketama,fnvhash,leastconn"
/// 
/// # Arguments
/// * `routes` - HashMap of HTTPRoute resources (key: resource_key, value: HTTPRoute)
/// 
/// # Example
/// ```
/// use std::collections::HashMap;
/// use edgion::core::routes::lb_policy_sync::sync_lb_policies_for_routes;
/// 
/// let routes = HashMap::new();
/// // ... populate routes ...
/// sync_lb_policies_for_routes(&routes);
/// ```
pub fn sync_lb_policies_for_routes(routes: &HashMap<String, HTTPRoute>) {
    let policy_store = get_global_policy_store();
    
    // Track which routes reference which services
    // Format: HashMap<route_key, HashMap<service_key, Vec<LbPolicy>>>
    let mut route_service_policies: HashMap<String, HashMap<String, Vec<LbPolicy>>> = HashMap::new();
    
    for (route_key, route) in routes {
        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        
        // Get rules from spec
        let Some(rules) = route.spec.rules.as_ref() else {
            continue;
        };
        
        for rule in rules {
            // Get backend_refs to know which services this rule routes to
            let Some(backend_refs) = rule.backend_refs.as_ref() else {
                continue;
            };
            
            // Get filters from rule
            let Some(filters) = rule.filters.as_ref() else {
                continue;
            };
            
            for filter in filters {
                // Only process ExtensionRef filters
                if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
                    continue;
                }
                
                // Get extension_ref
                let Some(ext_ref) = filter.extension_ref.as_ref() else {
                    tracing::warn!(
                        route = %route_key,
                        "ExtensionRef filter missing extension_ref field"
                    );
                    continue;
                };
                
                // Parse algorithms from extension_ref.name
                // Format: "algorithms" (applies to all backend_refs in this rule)
                let policies = LbPolicy::parse_from_string(&ext_ref.name);
                
                if policies.is_empty() {
                    tracing::warn!(
                        route = %route_key,
                        policy_name = %ext_ref.name,
                        "Failed to parse LB algorithms from extensionRef.name"
                    );
                    continue;
                }
                
                // Apply policies to all backend services in this rule
                for backend_ref in backend_refs {
                    let service_namespace = backend_ref.namespace.as_deref()
                        .unwrap_or(route_namespace);
                    let service_key = format!("{}/{}", service_namespace, backend_ref.name);
                    
                    // Add to route_service_policies map
                    route_service_policies
                        .entry(route_key.clone())
                        .or_insert_with(HashMap::new)
                        .entry(service_key.clone())
                        .or_insert_with(Vec::new)
                        .extend(policies.clone());
                    
                    tracing::debug!(
                        route = %route_key,
                        service = %service_key,
                        policies = ?policies,
                        "Extracted LB policies from HTTPRoute"
                    );
                }
            }
        }
    }
    
    // Update the global policy store with all extracted policies
    for (route_key, service_policies) in route_service_policies {
        policy_store.batch_add(route_key, service_policies);
    }
    
    tracing::info!(
        routes_processed = routes.len(),
        "LB policy extraction completed"
    );
}

/// Clean up LB policies for removed HTTPRoutes
/// 
/// This function removes all policy references for the specified routes
/// from the global policy store.
/// 
/// # Arguments
/// * `removed_routes` - Set of HTTPRoute resource keys that were removed
/// 
/// # Example
/// ```
/// use std::collections::HashSet;
/// use edgion::core::routes::lb_policy_sync::cleanup_lb_policies_for_routes;
/// 
/// let mut removed = HashSet::new();
/// removed.insert("default/my-route".to_string());
/// cleanup_lb_policies_for_routes(&removed);
/// ```
pub fn cleanup_lb_policies_for_routes(removed_routes: &HashSet<String>) {
    if removed_routes.is_empty() {
        return;
    }
    
    let policy_store = get_global_policy_store();
    let removed_keys: Vec<String> = removed_routes.iter().cloned().collect();
    policy_store.batch_remove_routes(&removed_keys);
    
    tracing::debug!(
        removed_count = removed_routes.len(),
        "Cleaned up LB policies for removed routes"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Helper function to create a test HTTPRoute with LB policy
    fn create_test_route_with_lb_policy(
        namespace: &str,
        name: &str,
        service: &str,
        algorithm: &str,
    ) -> HTTPRoute {
        let json = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "namespace": namespace,
                "name": name,
                "kind": "HTTPRoute"
            },
            "spec": {
                "parentRefs": [{
                    "name": "test-gateway",
                    "kind": "Gateway"
                }],
                "hostnames": ["test.example.com"],
                "rules": [{
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "name": algorithm,
                            "kind": "LBPolicy"
                        }
                    }],
                    "backendRefs": [{
                        "name": service,
                        "port": 8080,
                        "kind": "Service"
                    }]
                }]
            }
        });
        
        serde_json::from_value(json).expect("Failed to create test HTTPRoute")
    }

    #[test]
    fn test_sync_lb_policies_for_routes() {
        let policy_store = get_global_policy_store();
        policy_store.clear();
        
        let mut routes = HashMap::new();
        let route = create_test_route_with_lb_policy("default", "route1", "service1", "ketama");
        routes.insert("default/route1".to_string(), route);
        
        sync_lb_policies_for_routes(&routes);
        
        let policies = policy_store.get("default/service1");
        assert!(!policies.is_empty());
        assert!(policies.contains(&LbPolicy::Ketama));
        
        policy_store.clear();
    }

    #[test]
    fn test_cleanup_lb_policies_for_routes() {
        let policy_store = get_global_policy_store();
        policy_store.clear();
        
        // First add some policies
        let mut routes = HashMap::new();
        let route = create_test_route_with_lb_policy("default", "route1", "service1", "ketama");
        routes.insert("default/route1".to_string(), route);
        sync_lb_policies_for_routes(&routes);
        
        // Verify policies exist
        assert!(!policy_store.get("default/service1").is_empty());
        
        // Clean up
        let mut removed = HashSet::new();
        removed.insert("default/route1".to_string());
        cleanup_lb_policies_for_routes(&removed);
        
        // Verify policies are removed
        assert!(policy_store.get("default/service1").is_empty());
        
        policy_store.clear();
    }

    #[test]
    fn test_sync_multiple_routes() {
        let policy_store = get_global_policy_store();
        policy_store.clear();
        
        let mut routes = HashMap::new();
        let route1 = create_test_route_with_lb_policy("default", "route1", "service1", "ketama");
        let route2 = create_test_route_with_lb_policy("default", "route2", "service2", "fnvhash");
        routes.insert("default/route1".to_string(), route1);
        routes.insert("default/route2".to_string(), route2);
        
        sync_lb_policies_for_routes(&routes);
        
        let policies1 = policy_store.get("default/service1");
        let policies2 = policy_store.get("default/service2");
        assert!(policies1.contains(&LbPolicy::Ketama));
        assert!(policies2.contains(&LbPolicy::FnvHash));
        
        policy_store.clear();
    }
}

