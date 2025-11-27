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

/// Implement ConfHandler for Arc<RouteManager> to allow using the global instance
impl ConfHandler<HTTPRoute> for Arc<RouteManager> {
    fn full_build(&self, data: &HashMap<String, HTTPRoute>) {
        (**self).full_build(data)
    }

    fn conf_change(&self, add_or_update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        (**self).conf_change(add_or_update, remove)
    }

    fn update_rebuild(&self) {
        (**self).update_rebuild()
    }
}

/// Create a RouteManager handler for registration with ConfigClient
/// Returns the global RouteManager instance
pub fn create_route_manager_handler() -> Box<dyn crate::core::conf_sync::traits::ConfHandler<HTTPRoute> + Send + Sync> {
    Box::new(crate::core::routes::get_global_route_manager())
}

/// Private helper methods for RouteManager
impl RouteManager {
    /// Collect all hostnames affected by add/update/remove operations
    fn collect_affected_hostnames(
        &self,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let mut affected_hostnames = HashSet::new();
        
        // Collect hostnames from additions/updates
        for (key, route) in add_or_update.iter() {
            if let Some(hostnames) = &route.spec.hostnames {
                for hostname in hostnames {
                    affected_hostnames.insert(hostname.clone());
                }
                tracing::debug!(
                    component = "route_manager",
                    route_key = %key,
                    hostnames = ?hostnames,
                    "HTTPRoute add/update affects hostnames"
                );
            }
        }

        // Collect hostnames from removals (lookup from storage)
        for key in remove.iter() {
            let old_hostnames = {
                let routes = self.http_routes.lock().unwrap();
                routes.get(key).and_then(|route| route.spec.hostnames.clone())
            };
            
            if let Some(hostnames) = old_hostnames {
                for hostname in hostnames.iter() {
                    affected_hostnames.insert(hostname.clone());
                }
                tracing::debug!(
                    component = "route_manager",
                    route_key = %key,
                    hostnames = ?hostnames,
                    "HTTPRoute removal affects hostnames"
                );
            } else {
                tracing::warn!(
                    component = "route_manager",
                    route_key = %key,
                    "Old HTTPRoute not found in storage, cannot determine affected hostnames"
                );
            }
        }

        tracing::info!(
            component = "route_manager",
            total_affected_hostnames = affected_hostnames.len(),
            hostnames = ?affected_hostnames,
            "Collected all affected hostnames"
        );
        
        affected_hostnames
    }

    /// Update route_rules for a specific hostname by modifying its resource_keys HashSet
    /// Then rebuild from http_routes storage
    fn update_hostname_routes(
        &self,
        hostname: &str,
        gateway_key: &str,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) {
        // Get the domain routes map for this gateway
        let domain_routes_map = match self.gateway_routes_map.get(gateway_key) {
            Some(map) => map,
            None => {
                tracing::warn!(
                    component = "route_manager",
                    gateway_key = %gateway_key,
                    hostname = %hostname,
                    "Gateway not found in routes map"
                );
                return;
            }
        };

        // Use RCU to update the route rules for this hostname
        domain_routes_map.domain_routes_map.rcu(|current_map| {
            let current_hashmap: &HashMap<DomainStr, Arc<RouteRules>> = current_map.as_ref();
            let mut new_hashmap = current_hashmap.clone();
            
            // Get existing resource_keys or create empty set
            let mut resource_keys = new_hashmap
                .get(hostname)
                .map(|rr| rr.resource_keys.read().unwrap().clone())
                .unwrap_or_else(std::collections::HashSet::new);
            
            // Step 1: Remove resource keys
            for key in remove.iter() {
                if resource_keys.remove(key) {
                    tracing::debug!(
                        component = "route_manager",
                        hostname = %hostname,
                        resource_key = %key,
                        "Removed resource key from hostname"
                    );
                }
            }
            
            // Step 2: Add/update resource keys from add_or_update
            for (resource_key, route) in add_or_update.iter() {
                // Check if this route applies to this hostname
                let applies = route.spec.hostnames
                    .as_ref()
                    .map(|hostnames| hostnames.contains(&hostname.to_string()))
                    .unwrap_or(false);
                
                if applies {
                    resource_keys.insert(resource_key.clone());
                    tracing::debug!(
                        component = "route_manager",
                        hostname = %hostname,
                        resource_key = %resource_key,
                        "Added resource key to hostname"
                    );
                }
            }
            
            // Step 3: Rebuild route_rules_list and match_engine from resource_keys
            if resource_keys.is_empty() {
                // No more routes for this hostname, remove it
                new_hashmap.remove(hostname);
                tracing::info!(
                    component = "route_manager",
                    hostname = %hostname,
                    "Removed hostname (no more routes)"
                );
            } else {
                // Rebuild from http_routes storage
                let mut route_rules_list = Vec::new();
                
                let http_routes = self.http_routes.lock().unwrap();
                for resource_key in resource_keys.iter() {
                    if let Some(route) = http_routes.get(resource_key) {
                        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                        let route_name = route.metadata.name.as_deref().unwrap_or("");
                        
                        if let Some(rules) = &route.spec.rules {
                            for rule in rules {
                                let rule_unit = HttpRouteRuleUnit::new(
                                    route_namespace.to_string(),
                                    route_name.to_string(),
                                    resource_key.clone(),
                                    rule.clone(),
                                );
                                route_rules_list.push(rule_unit);
                            }
                        }
                    } else {
                        tracing::warn!(
                            component = "route_manager",
                            resource_key = %resource_key,
                            "Resource key in set but not found in http_routes storage"
                        );
                    }
                }
                
                // Build match engine
                let route_entries: Vec<Arc<dyn RouteEntry>> = route_rules_list
                    .iter()
                    .map(|unit| Arc::new(unit.clone()) as Arc<dyn RouteEntry>)
                    .collect();
                
                match RadixRouteMatchEngine::build(route_entries.clone()) {
                    Ok(new_engine) => {
                        let new_route_rules = Arc::new(RouteRules {
                            resource_keys: RwLock::new(resource_keys),
                            route_rules_list: RwLock::new(route_rules_list),
                            match_engine: Arc::new(new_engine),
                        });
                        
                        new_hashmap.insert(hostname.to_string(), new_route_rules);
                        
                        tracing::info!(
                            component = "route_manager",
                            hostname = %hostname,
                            resource_keys_count = route_entries.len(),
                            "Rebuilt match engine for hostname"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            component = "route_manager",
                            hostname = %hostname,
                            error = %e,
                            "Failed to rebuild match engine"
                        );
                    }
                }
            }
            
            Arc::new(new_hashmap)
        });
    }
}

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
    fn full_build(&self, data: &HashMap<String, HTTPRoute>) {
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
                tracing::debug!(
                    component = "route_manager",
                    gateway_key = %gateway_key,
                    "Gateway not found in store, skipping routes (may not be managed by this instance)"
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

                // Collect resource keys for this domain
                let resource_keys: std::collections::HashSet<String> = rules_vec
                    .iter()
                    .map(|unit| unit.resource_key.clone())
                    .collect();
                
                // Create RouteRules
                let route_rules = Arc::new(RouteRules {
                    resource_keys: RwLock::new(resource_keys),
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
    fn conf_change(&self, add_or_update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        tracing::info!(
            component = "route_manager",
            add_or_update_count = add_or_update.len(),
            remove_count = remove.len(),
            "Processing HTTPRoute changes"
        );

        // Step 1: Collect all affected hostnames
        let affected_hostnames = self.collect_affected_hostnames(&add_or_update, &remove);

        // Step 2 & 3: For each affected hostname, update its route_rules_list and rebuild match engine
        // We need to find which gateways each hostname belongs to
        for hostname in affected_hostnames.iter() {
            // Find all gateways that have this hostname
            for gateway_entry in self.gateway_routes_map.iter() {
                let gateway_key = gateway_entry.key();
                
                // Check if this gateway has routes for this hostname
                let has_hostname = {
                    let domain_map = gateway_entry.value().domain_routes_map.load();
                    domain_map.contains_key(hostname)
                };
                
                if has_hostname {
                    tracing::debug!(
                        component = "route_manager",
                        hostname = %hostname,
                        gateway_key = %gateway_key,
                        "Updating routes for hostname in gateway"
                    );
                    
                    self.update_hostname_routes(hostname, gateway_key, &add_or_update, &remove);
                } else {
                    // Even if gateway doesn't have this hostname yet, check if add_or_update contains routes for it
                    let should_add = add_or_update.values().any(|route| {
                        route.spec.hostnames
                            .as_ref()
                            .map(|hostnames| hostnames.contains(&hostname.to_string()))
                            .unwrap_or(false)
                            && route.spec.parent_refs
                                .as_ref()
                                .map(|refs| refs.iter().any(|ref_| {
                                    let key = if let Some(ns) = &ref_.namespace {
                                        format!("{}/{}", ns, ref_.name)
                                    } else if let Some(ns) = &route.metadata.namespace {
                                        format!("{}/{}", ns, ref_.name)
                                    } else {
                                        ref_.name.clone()
                                    };
                                    &key == gateway_key
                                }))
                                .unwrap_or(false)
                    });
                    
                    if should_add {
                        tracing::debug!(
                            component = "route_manager",
                            hostname = %hostname,
                            gateway_key = %gateway_key,
                            "Adding new hostname to gateway"
                        );
                        
                        self.update_hostname_routes(hostname, gateway_key, &add_or_update, &remove);
                    }
                }
            }
        }
        
        // Step 4: Update stored http_routes
        {
            let mut routes = self.http_routes.lock().unwrap();
            
            // Remove deleted routes
            for key in remove.iter() {
                if routes.remove(key).is_some() {
                    tracing::debug!(
                        component = "route_manager",
                        route_key = %key,
                        "Removed HTTPRoute from storage"
                    );
                }
            }
            
            // Add or update routes
            for (key, route) in add_or_update.iter() {
                routes.insert(key.clone(), route.clone());
                tracing::debug!(
                    component = "route_manager",
                    route_key = %key,
                    "Stored/updated HTTPRoute in storage"
                );
            }
        }
        
        tracing::info!(
            component = "route_manager",
            "HTTPRoute changes processed successfully"
        );
    }

    /// Trigger a rebuild/refresh of the route configuration
    /// This could be used to optimize internal data structures or refresh caches
    fn update_rebuild(&self) {
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

