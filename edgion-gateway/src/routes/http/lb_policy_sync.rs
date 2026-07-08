//! Load balancing policy synchronization for HTTPRoute
//!
//! This module handles extracting LB policies from HTTPRoute resources
//! and synchronizing them with the global policy store.

use crate::lb::lb_policy::{get_global_policy_store, LbPolicy};
use edgion_resources::{HTTPRoute, ParsedLBPolicy};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Extract LB policies from HTTPRoute backend refs and update the global policy store.
///
/// Reads `extension_info.lb_policy` (pre-parsed during `pre_parse()`) from each
/// `HTTPBackendRef` and batch-inserts the results into the policy store.
pub fn sync_lb_policies_for_routes(routes: &HashMap<String, Arc<HTTPRoute>>) {
    let policy_store = get_global_policy_store();

    // route_key -> service_key -> Vec<LbPolicy>
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

            for backend_ref in backend_refs {
                let Some(parsed_policy) = &backend_ref.extension_info.lb_policy else {
                    continue;
                };

                let lb_policy = match parsed_policy {
                    ParsedLBPolicy::ConsistentHash(_) => LbPolicy::Consistent,
                    ParsedLBPolicy::LeastConn => LbPolicy::LeastConnection,
                    ParsedLBPolicy::Ewma => LbPolicy::Ewma,
                };

                let ns = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
                let service_key = format!("{}/{}", ns, backend_ref.name);

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

    for (route_key, service_policies) in route_service_policies {
        policy_store.batch_add(route_key, service_policies);
    }

    tracing::info!(routes_processed = routes.len(), "LB policy extraction completed");
}

/// Remove all LB policy references for the given HTTPRoute resource keys from the global store.
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
    use edgion_resources::ResourceMeta;
    use std::collections::HashMap;

    /// Build a test HTTPRoute with an LB policy extensionRef on the backend.
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
        routes.insert("test-sync/route1".to_string(), Arc::new(route));

        sync_lb_policies_for_routes(&routes);

        let policies = policy_store.get("test-sync/svc-sync");
        assert!(!policies.is_empty());
        assert!(policies.contains(&LbPolicy::Consistent));

        policy_store.remove_by_route("test-sync/route1");
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
        routes.insert("test-cleanup/route1".to_string(), Arc::new(route));
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
        routes.insert("test-multi/route1".to_string(), Arc::new(route1));
        routes.insert("test-multi/route2".to_string(), Arc::new(route2));

        sync_lb_policies_for_routes(&routes);

        let policies1 = policy_store.get("test-multi/svc-multi-1");
        let policies2 = policy_store.get("test-multi/svc-multi-2");
        assert!(policies1.contains(&LbPolicy::Consistent));
        assert!(policies2.contains(&LbPolicy::LeastConnection));

        policy_store.remove_by_route("test-multi/route1");
        policy_store.remove_by_route("test-multi/route2");
    }
}
