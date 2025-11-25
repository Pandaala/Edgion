use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::{RouteManager, HttpRouteRuleUnit};
use crate::core::routes::routes_mgr::RouteRules;
use crate::core::routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::match_engine::RouteEntry;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::types::{HTTPRoute, ResourceMeta};

type GatewayKey = String;
type DomainStr = String;

/// Parse all HTTPRoutes and collect rules into gateway->domain->rules structure
/// Returns HashMap<GatewayKey, HashMap<DomainStr, Vec<HttpRouteRuleUnit>>>
fn parse_http_routes_to_gateway_domain_rules(
    data: &HashMap<String, HTTPRoute>
) -> HashMap<GatewayKey, HashMap<DomainStr, Vec<HttpRouteRuleUnit>>> {
    let mut gateway_domain_rules: HashMap<GatewayKey, HashMap<DomainStr, Vec<HttpRouteRuleUnit>>> = HashMap::new();
    
    let mut processed_routes = 0;
    let mut skipped_routes = 0;

    // Iterate through all HTTPRoutes and collect rules
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
        }
    }

    tracing::debug!(
        component = "route_manager",
        processed_routes = processed_routes,
        skipped_routes = skipped_routes,
        gateways = gateway_domain_rules.len(),
        "Parsed HTTPRoutes into gateway-domain structure"
    );

    gateway_domain_rules
}

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
        let start_time = Instant::now();
        
        tracing::info!(
            component = "route_manager",
            count = data.len(),
            "Full build with HTTPRoutes - starting from scratch"
        );

        // Step 1: Parse all HTTPRoutes into temporary gateway->domain->rules structure
        let gateway_domain_rules_new = parse_http_routes_to_gateway_domain_rules(data);

        // Step 2: Build RouteRules with RadixRouteMatchEngine and update gateway_routes_map
        let gateway_store = get_global_gateway_store();
        let gateway_store_guard = gateway_store.read().unwrap();

        let mut processed_gateways = 0;
        let mut skipped_gateways = 0;

        for (gateway_key, domain_rules_map) in gateway_domain_rules_new.into_iter() {
            // Check if gateway exists in store
            if gateway_store_guard.get_gateway(&gateway_key).is_err() {
                tracing::warn!(
                    component = "route_manager",
                    gateway_key = %gateway_key,
                    "Gateway not found in store, skipping routes"
                );
                skipped_gateways += 1;
                continue;
            }

            // Build HashMap<DomainStr, Arc<RouteRules>> for this gateway
            let mut new_domain_routes: HashMap<DomainStr, Arc<RouteRules>> = HashMap::new();

            for (domain, rules_vec) in domain_rules_map.into_iter() {
                // Convert Vec<HttpRouteRuleUnit> to Vec<Arc<dyn RouteEntry>>
                let route_entries: Vec<Arc<dyn RouteEntry>> = rules_vec
                    .iter()
                    .map(|unit| Arc::new(unit.clone()) as Arc<dyn RouteEntry>)
                    .collect();

                // Build RadixRouteMatchEngine
                let match_engine = match RadixRouteMatchEngine::build(route_entries) {
                    Ok(engine) => Arc::new(engine),
                    Err(e) => {
                        tracing::error!(
                            component = "route_manager",
                            gateway_key = %gateway_key,
                            domain = %domain,
                            error = ?e,
                            "Failed to build RadixRouteMatchEngine"
                        );
                        continue;
                    }
                };

                // Create RouteRules
                let route_rules = Arc::new(RouteRules {
                    route_rules_list: RwLock::new(rules_vec),
                    match_engine,
                });

                new_domain_routes.insert(domain, route_rules);
            }

            // Get existing DomainRouteRules for this gateway (don't create new)
            let domain_route_rules = if let Some(entry) = self.gateway_routes_map.get(&gateway_key) {
                entry.value().clone()
            } else {
                tracing::debug!(
                    component = "route_manager",
                    gateway_key = %gateway_key,
                    "Gateway not found in routes map, skipping"
                );
                skipped_gateways += 1;
                continue;
            };

            // Replace the domain_routes_map with the new one
            // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
            // This double-Arc is needed for lock-free atomic pointer swapping
            domain_route_rules.domain_routes_map.store(Arc::new(Arc::new(new_domain_routes)));

            processed_gateways += 1;
        }

        let elapsed = start_time.elapsed();
        tracing::info!(
            component = "route_manager",
            total_gateways = processed_gateways + skipped_gateways,
            processed = processed_gateways,
            skipped = skipped_gateways,
            elapsed_ms = elapsed.as_millis(),
            "Full build completed in {:?}",
            elapsed
        );
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

