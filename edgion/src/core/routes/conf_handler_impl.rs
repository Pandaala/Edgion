use std::collections::{HashMap, HashSet};
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::{RouteManager, HttpRouteRuleUnit};
use crate::types::{HTTPRoute, ResourceMeta};

type GatewayKey = String;
type DomainStr = String;

/// Validate HTTPRoute and extract required fields
/// Returns Some((parent_refs, rules, hostnames, namespace, name)) if valid, None otherwise
fn validate_http_route(route: &HTTPRoute) -> Option<(
    &Vec<crate::types::resources::http_route::ParentReference>,
    &Vec<crate::types::HTTPRouteRule>,
    &Vec<String>,
    String,
    String,
)> {
    // Check parent_refs
    let parent_refs = match &route.spec.parent_refs {
        Some(refs) if !refs.is_empty() => refs,
        _ => {
            tracing::warn!(
                route_key = %route.key_name(),
                "HTTPRoute has no parent_refs, skipping"
            );
            return None;
        }
    };

    // Check rules
    let rules = match &route.spec.rules {
        Some(rules) if !rules.is_empty() => rules,
        _ => {
            tracing::warn!(
                route_key = %route.key_name(),
                "HTTPRoute has no rules, skipping"
            );
            return None;
        }
    };

    // Check hostnames
    let hostnames = match &route.spec.hostnames {
        Some(hostnames) if !hostnames.is_empty() => hostnames,
        _ => {
            tracing::warn!(
                route_key = %route.key_name(),
                "HTTPRoute has no hostnames, skipping"
            );
            return None;
        }
    };

    // Check and extract route namespace
    let route_namespace = match &route.metadata.namespace {
        Some(ns) if !ns.is_empty() => ns.clone(),
        _ => {
            tracing::warn!(
                route_key = %route.key_name(),
                "HTTPRoute has no namespace, skipping"
            );
            return None;
        }
    };

    // Check and extract route name
    let route_name = match &route.metadata.name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            tracing::warn!(
                route_key = %route.key_name(),
                "HTTPRoute has no name, skipping"
            );
            return None;
        }
    };

    Some((parent_refs, rules, hostnames, route_namespace, route_name))
}

impl ConfHandler<HTTPRoute> for RouteManager {
    /// Full rebuild with a complete set of HTTPRoutes
    /// This is typically called during initial sync
    fn full_build(&mut self, data: &HashMap<String, HTTPRoute>) {
        tracing::info!(
            component = "route_manager",
            count = data.len(),
            "Full build with HTTPRoutes - starting from scratch"
        );

        // Step 1: Define temporary storage for parsed data
        // Structure: HashMap<GatewayKey, HashMap<DomainStr, Vec<HttpRouteRuleUnit>>>
        let mut gateway_domain_rules: HashMap<GatewayKey, HashMap<DomainStr, Vec<HttpRouteRuleUnit>>> = HashMap::new();

        let mut processed_routes = 0;
        let mut skipped_routes = 0;

        // Step 2: Iterate through all HTTPRoutes and collect rules
        for (_key, route) in data.iter() {
            // Validate HTTPRoute and extract required fields
            let (parent_refs, rules, hostnames, route_namespace, route_name) = match validate_http_route(route) {
                Some(validated) => validated,
                None => {
                    skipped_routes += 1;
                    continue;
                }
            };

            // Process each parent gateway reference
            for parent_ref in parent_refs {
                // Build gateway key
                let gateway_key = if let Some(namespace) = parent_ref.namespace.as_ref() {
                    format!("{}/{}", namespace, parent_ref.name)
                } else {
                    format!("{}/{}", route_namespace, parent_ref.name)
                };

                // Get or create the domain map for this gateway
                let domain_map = gateway_domain_rules
                    .entry(gateway_key.clone())
                    .or_insert_with(HashMap::new);

                // Process each hostname and rule combination
                for hostname in hostnames {
                    for rule in rules {
                        // Create HttpRouteRuleUnit
                        let rule_unit = HttpRouteRuleUnit::new(
                            route_namespace.clone(),
                            route_name.clone(),
                            route.key_name(),
                            rule.clone(),
                        );

                        // Add to the domain's rule list
                        domain_map
                            .entry(hostname.clone())
                            .or_insert_with(Vec::new)
                            .push(rule_unit);
                    }
                }

                processed_routes += 1;
                tracing::debug!(route_key = %route.key_name(), gateway_key = %gateway_key, "Collected HTTPRoute rules");
            }
        }

        tracing::info!(
            component = "route_manager",
            total_routes = data.len(),
            processed = processed_routes,
            skipped = skipped_routes,
            gateways = gateway_domain_rules.len(),
            "Step 1 completed: collected all route rules"
        );

        // TODO: Step 3 - Build RadixRouteMatchEngine and update gateway_routes_map
        // This will be implemented in the next step
    }

    /// Handle incremental configuration changes
    /// Processes additions, updates, and removals of HTTPRoutes
    fn conf_change(&mut self, add_or_update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        tracing::info!(
            component = "route_manager",
            add_or_update_count = add_or_update.len(),
            remove_count = remove.len(),
            "Processing HTTPRoute changes"
        );

        // Process additions and updates
        for (key, route) in add_or_update {
            tracing::debug!(
                component = "route_manager",
                route_key = %key,
                "Adding/updating HTTPRoute"
            );
            self.add_http_route(route);
        }

        // Process removals
        for key in remove {
            tracing::debug!(
                component = "route_manager",
                route_key = %key,
                "Removing HTTPRoute (not yet implemented)"
            );
            // TODO: Implement route removal when RouteManager supports it
            // self.remove_http_route(&key);
        }

        tracing::info!(
            component = "route_manager",
            "HTTPRoute changes processed"
        );
    }

    /// Trigger a rebuild/refresh of the route configuration
    /// This could be used to optimize internal data structures or refresh caches
    fn update_rebuild(&mut self) {
        tracing::debug!(
            component = "route_manager",
            "Update rebuild triggered (no-op for now)"
        );
        
        // Currently no-op as RouteManager doesn't need periodic rebuilds
        // If needed in the future, we could:
        // - Rebuild internal match engines
        // - Optimize route lookup structures
        // - Refresh cached data
    }
}

