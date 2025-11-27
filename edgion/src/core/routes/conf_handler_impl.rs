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
    /// Build gateway_hostnames map from add_or_update and remove sets
    /// Returns a map of gateway_key -> set of affected hostnames
    fn build_gateway_hostnames_map(
        &self,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) -> HashMap<String, HashSet<String>> {
        let mut gateway_hostnames: HashMap<String, HashSet<String>> = HashMap::new();
        
        // Process add_or_update routes
        for route in add_or_update.values() {
            if let Some(hostnames) = &route.spec.hostnames {
                if let Some(parent_refs) = &route.spec.parent_refs {
                    for parent_ref in parent_refs {
                        let gateway_key = if let Some(ns) = &parent_ref.namespace {
                            format!("{}/{}", ns, parent_ref.name)
                        } else if let Some(ns) = &route.metadata.namespace {
                            format!("{}/{}", ns, parent_ref.name)
                        } else {
                            parent_ref.name.clone()
                        };
                        
                        let hostname_set = gateway_hostnames
                            .entry(gateway_key)
                            .or_insert_with(HashSet::new);
                        for hostname in hostnames {
                            hostname_set.insert(hostname.clone());
                        }
                    }
                }
            }
        }
        
        // Process remove routes - find which gateways/hostnames they affect
        let http_routes = self.http_routes.lock().unwrap();
        for resource_key in remove.iter() {
            if let Some(route) = http_routes.get(resource_key) {
                if let Some(hostnames) = &route.spec.hostnames {
                    if let Some(parent_refs) = &route.spec.parent_refs {
                        for parent_ref in parent_refs {
                            let gateway_key = if let Some(ns) = &parent_ref.namespace {
                                format!("{}/{}", ns, parent_ref.name)
                            } else if let Some(ns) = &route.metadata.namespace {
                                format!("{}/{}", ns, parent_ref.name)
                            } else {
                                parent_ref.name.clone()
                            };
                            
                            let hostname_set = gateway_hostnames
                                .entry(gateway_key)
                                .or_insert_with(HashSet::new);
                            for hostname in hostnames {
                                hostname_set.insert(hostname.clone());
                            }
                        }
                    }
                }
            }
        }
        
        gateway_hostnames
    }

    /// Collect all hostnames affected by add/update/remove operations

    /// Update a single hostname's RouteRules in the given HashMap
    /// This modifies the HashMap in place and does NOT do RCU
    fn update_single_hostname(
        &self,
        domain_hashmap: &mut HashMap<DomainStr, Arc<RouteRules>>,
        hostname: &str,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) {
        // Get existing resource_keys or create empty set
        let mut resource_keys = domain_hashmap
            .get(hostname)
            .map(|rr| rr.resource_keys.read().unwrap().clone())
            .unwrap_or_else(HashSet::new);
        
        // Step 1: Remove resource keys
        for key in remove.iter() {
            if resource_keys.remove(key) {
                tracing::debug!(component="route_manager",hostname=%hostname,key=%key,"rm key");
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
                tracing::debug!(component="route_manager",hostname=%hostname,key=%resource_key,"add key");
            }
        }
        
        // Step 3: Rebuild route_rules_list and match_engine from resource_keys
        if resource_keys.is_empty() {
            // No more routes for this hostname, remove it
            domain_hashmap.remove(hostname);
            tracing::info!(component="route_manager",hostname=%hostname,"rm hostname");
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
                    tracing::warn!(component="route_manager",key=%resource_key,"key not found in storage");
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
                    
                    domain_hashmap.insert(hostname.to_string(), new_route_rules);
                    tracing::debug!(component="route_manager",hostname=%hostname,cnt=route_entries.len(),"updated");
                }
                Err(e) => {
                    tracing::error!(component="route_manager",hostname=%hostname,err=%e,"rebuild failed");
                }
            }
        }
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

    tracing::debug!(component="route_manager",proc=processed_routes,skip=skipped_routes,gws=gateway_domain_rules.len(),"parsed");

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
            tracing::warn!(route=%route.key_name(),"no parent_refs");
            return None;
        }
    };

    // Check rules
    let rules = match &route.spec.rules {
        Some(rules) if !rules.is_empty() => rules,
        _ => {
            tracing::warn!(route=%route.key_name(),"no rules");
            return None;
        }
    };

    // Check hostnames
    let hostnames = match &route.spec.hostnames {
        Some(hostnames) if !hostnames.is_empty() => hostnames,
        _ => {
            tracing::warn!(route=%route.key_name(),"no hostnames");
            return None;
        }
    };

    // Check and extract route namespace
    let route_namespace = match &route.metadata.namespace {
        Some(ns) if !ns.is_empty() => ns.clone(),
        _ => {
            tracing::warn!(route=%route.key_name(),"no namespace");
            return None;
        }
    };

    // Check and extract route name
    let route_name = match &route.metadata.name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            tracing::warn!(route=%route.key_name(),"no name");
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
        tracing::info!(component="route_manager",cnt=data.len(),"full build start");

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
                tracing::debug!(component="route_manager",gw=%gateway_key,"gw not in store");
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
                        tracing::error!(component="route_manager",gw=%gateway_key,domain=%domain,err=?e,"build failed");
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
                tracing::debug!(component="route_manager",gw=%gateway_key,"gw not in routes map");
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
        tracing::info!(component="route_manager",total=processed_gateways+skipped_gateways,proc=processed_gateways,skip=skipped_gateways,ms=elapsed.as_millis(),"full build done");
    }

    /// Handle incremental configuration changes
    /// Processes additions, updates, and removals of HTTPRoutes
    fn conf_change(&self, add_or_update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        tracing::info!(component = "route_manager",au = add_or_update.len(),rm = remove.len(),"Processing HTTPRoute changes");

        // Step 0: First update http_routes storage (before rebuilding, so we have the latest data)
        {
            let mut routes = self.http_routes.lock().unwrap();
            for (key, route) in add_or_update.iter() {
                routes.insert(key.clone(), route.clone());
                tracing::debug!(component = "route_manager",route_key = %key,"add/update HTTPRoute");
            }
        }
        
        // Step 1: Build gateway_hostnames map
        let gateway_hostnames = self.build_gateway_hostnames_map(&add_or_update, &remove);

        // Step 2: For each gateway, update all affected hostnames in one RCU operation
        for (gateway_key, hostnames) in gateway_hostnames.iter() {
            tracing::debug!(component="route_manager",gw=%gateway_key,cnt=hostnames.len(),"updating gw");
            
            if let Some(domain_routes_ref) = self.gateway_routes_map.get(gateway_key) {
                // Clone current domain_routes_map
                let current_map = domain_routes_ref.domain_routes_map.load();
                let current_hashmap: &HashMap<DomainStr, Arc<RouteRules>> = current_map.as_ref();
                let mut new_hashmap: HashMap<String, Arc<RouteRules>> = current_hashmap.clone();
                
                // Update all affected hostnames
                for hostname in hostnames.iter() {
                    self.update_single_hostname(
                        &mut new_hashmap,
                        hostname,
                        &add_or_update,
                        &remove,
                    );
                }
                
                // Replace the entire domain_routes_map in one atomic operation
                // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
                domain_routes_ref.domain_routes_map.store(Arc::new(Arc::new(new_hashmap)));
                tracing::info!(component="route_manager",gw=%gateway_key,cnt=hostnames.len(),"updated gw");
            }
        }
        
        // Step 3: Remove deleted routes from http_routes storage (after rebuilding)
        {
            let mut routes = self.http_routes.lock().unwrap();
            for key in remove.iter() {
                if routes.remove(key).is_some() {
                    tracing::debug!(component="route_manager",key=%key,"rm route");
                }
            }
        }
        
        tracing::info!( component = "route_manager","HTTPRoute changes processed successfully");
    }

    /// Trigger a rebuild/refresh of the route configuration
    /// This could be used to optimize internal data structures or refresh caches
    fn update_rebuild(&self) {
        tracing::debug!(component="route_manager","update_rebuild(no-op)");
        // Currently no-op as RouteManager doesn't need periodic rebuilds
        // If needed in the future, we could:
        // - Rebuild internal match engines
        // - Optimize route lookup structures
        // - Refresh cached data
    }
}

