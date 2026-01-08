//! Load balancing policy synchronization for HTTPRoute
//!
//! This module handles extracting LB policies from HTTPRoute resources
//! and synchronizing them with the global policy store.

use crate::core::lb::lb_policy::{get_global_policy_store, LbPolicy};
use crate::types::{HTTPRoute, ParsedLBPolicy};
use std::collections::{HashMap, HashSet};

/// Extract load balancing policies from HTTPRoute and update the global policy store
///
/// This function iterates through all rules and backend_refs in the HTTPRoute,
/// extracts pre-parsed LB policy from extension_info, and updates the global policy store.
///
/// The extension_info.lb_policy is already parsed during pre_parse() stage,
/// supporting formats:
/// - kind: LBPolicyConsistentHash, name: header.xxx / cookie.xxx / arg.xxx
/// - kind: LBPolicyLeastConn, name: default
///
/// # Arguments
/// * `routes` - HashMap of HTTPRoute resources (key: resource_key, value: HTTPRoute)
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

            // Iterate through backend_refs and check pre-parsed extension_info
            for backend_ref in backend_refs {
                // Check if this backend_ref has LB policy configured
                let Some(parsed_policy) = &backend_ref.extension_info.lb_policy else {
                    continue;
                };

                // Convert ParsedLBPolicy to LbPolicy
                let lb_policy = match parsed_policy {
                    ParsedLBPolicy::ConsistentHash(_) => LbPolicy::Consistent,
                    ParsedLBPolicy::LeastConn => LbPolicy::LeastConnection,
                    ParsedLBPolicy::Ewma => LbPolicy::Ewma,
                };

                let service_namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
                let service_key = format!("{}/{}", service_namespace, backend_ref.name);

                // Add to route_service_policies map
                route_service_policies
                    .entry(route_key.clone())
                    .or_default()
                    .entry(service_key.clone())
                    .or_default()
                    .push(lb_policy);

                tracing::debug!(
                    route = %route_key,
                    service = %service_key,
                    policy = ?parsed_policy,
                    "Extracted LB policy from HTTPBackendRef.extension_info"
                );
            }
        }
    }

    // Update the global policy store with all extracted policies
    for (route_key, service_policies) in route_service_policies {
        policy_store.batch_add(route_key, service_policies);
    }

    tracing::info!(routes_processed = routes.len(), "LB policy extraction completed");
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
/// use edgion::core::routes::http_routes::lb_policy_sync::cleanup_lb_policies_for_routes;
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
    use crate::types::ResourceMeta;
    use std::collections::HashMap;

    /// Helper function to create a test HTTPRoute with LB policy
    /// Uses new extensionRef format: kind=LBPolicyConsistentHash/LBPolicyLeastConn
    fn create_test_route_with_lb_policy(
        namespace: &str,
        name: &str,
        service: &str,
        kind: &str,
        policy_name: &str,
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
                    "backendRefs": [{
                        "name": service,
                        "port": 8080,
                        "kind": "Service",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": {
                                "group": "edgion.io",
                                "kind": kind,
                                "name": policy_name
                            }
                        }]
                    }]
                }]
            }
        });

        let mut route: HTTPRoute = serde_json::from_value(json).expect("Failed to create test HTTPRoute");
        // Call pre_parse to populate extension_info
        route.pre_parse();
        route
    }

    #[test]
    fn test_sync_lb_policies_for_routes() {
        // Use unique namespace to avoid parallel test conflicts
        let policy_store = get_global_policy_store();

        let mut routes = HashMap::new();
        let route = create_test_route_with_lb_policy(
            "test-sync",
            "route1",
            "svc-sync",
            "LBPolicyConsistentHash",
            "header.x-user-id",
        );
        routes.insert("test-sync/route1".to_string(), route);

        sync_lb_policies_for_routes(&routes);

        let policies = policy_store.get("test-sync/svc-sync");
        assert!(!policies.is_empty());
        assert!(policies.contains(&LbPolicy::Consistent));

        // Cleanup only our test data
        policy_store.delete_lb_policies_by_resource_key("test-sync/route1");
    }

    #[test]
    fn test_cleanup_lb_policies_for_routes() {
        // Use unique namespace to avoid parallel test conflicts
        let policy_store = get_global_policy_store();

        // First add some policies
        let mut routes = HashMap::new();
        let route = create_test_route_with_lb_policy(
            "test-cleanup",
            "route1",
            "svc-cleanup",
            "LBPolicyConsistentHash",
            "cookie.session-id",
        );
        routes.insert("test-cleanup/route1".to_string(), route);
        sync_lb_policies_for_routes(&routes);

        // Verify policies exist
        assert!(!policy_store.get("test-cleanup/svc-cleanup").is_empty());

        // Clean up
        let mut removed = HashSet::new();
        removed.insert("test-cleanup/route1".to_string());
        cleanup_lb_policies_for_routes(&removed);

        // Verify policies are removed
        assert!(policy_store.get("test-cleanup/svc-cleanup").is_empty());
    }

    #[test]
    fn test_sync_multiple_routes() {
        // Use unique namespace to avoid parallel test conflicts
        let policy_store = get_global_policy_store();

        let mut routes = HashMap::new();
        let route1 = create_test_route_with_lb_policy(
            "test-multi",
            "route1",
            "svc-multi-1",
            "LBPolicyConsistentHash",
            "header.x-tenant-id",
        );
        let route2 =
            create_test_route_with_lb_policy("test-multi", "route2", "svc-multi-2", "LBPolicyLeastConn", "default");
        routes.insert("test-multi/route1".to_string(), route1);
        routes.insert("test-multi/route2".to_string(), route2);

        sync_lb_policies_for_routes(&routes);

        let policies1 = policy_store.get("test-multi/svc-multi-1");
        let policies2 = policy_store.get("test-multi/svc-multi-2");
        assert!(policies1.contains(&LbPolicy::Consistent));
        assert!(policies2.contains(&LbPolicy::LeastConnection));

        // Cleanup only our test data
        policy_store.delete_lb_policies_by_resource_key("test-multi/route1");
        policy_store.delete_lb_policies_by_resource_key("test-multi/route2");
    }
}
