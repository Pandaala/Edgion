use std::sync::{Arc, RwLock, Mutex};
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::routes::{HttpRouteRuleUnit, HttpRouteRuleRegexUnit, MatchedRoute};
use crate::core::routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::types::{HTTPRoute, ResourceMeta};

type DomainStr = String;

pub struct RouteRules {
    /// All resource keys (HTTPRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub(crate) resource_keys: RwLock<HashSet<String>>,
    
    /// Exact and prefix match routes (handled by radix tree)
    pub(crate) route_rules_list: RwLock<Vec<HttpRouteRuleUnit>>,
    pub(crate) match_engine: Arc<RadixRouteMatchEngine>,
    
    /// Regex match routes (handled separately)
    pub(crate) regex_routes: RwLock<Vec<HttpRouteRuleRegexUnit>>,
}

impl Clone for RouteRules {
    fn clone(&self) -> Self {
        Self {
            resource_keys: RwLock::new(self.resource_keys.read().unwrap().clone()),
            route_rules_list: RwLock::new(self.route_rules_list.read().unwrap().clone()),
            match_engine: self.match_engine.clone(),
            regex_routes: RwLock::new(self.regex_routes.read().unwrap().clone()),
        }
    }
}

impl RouteRules {
    /// Match a route using the match_engine engine
    /// Try match in order: exact → regex → prefix
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
    ) -> Result<MatchedRoute, crate::types::err::EdError> {
        let path = session.req_header().uri.path().to_string();
        
        // Step 1: Try exact match first (highest priority)
        if let Some(route_entry) = self.match_engine.exact_match(session)? {
            tracing::debug!(path=%path,"exact match ok");
            // Convert RouteEntry back to HttpRouteRuleUnit
            let route_entry_id = route_entry.identifier();
            let route_rules = self.route_rules_list.read().unwrap();
            if let Some(unit) = route_rules.iter().find(|u| format!("{}/{}", u.namespace, u.name) == route_entry_id) {
                return Ok(MatchedRoute::Normal(Arc::new(unit.clone())));
            }
        }
        
        // Step 2: Try regex match
        let regex_routes = self.regex_routes.read().unwrap();
        for regex_route in regex_routes.iter() {
            if regex_route.matches_path(&path) {
                if regex_route.deep_match(session)? {
                    tracing::debug!(path=%path,regex=%regex_route.path_regex.as_str(),"regex match ok");
                    return Ok(MatchedRoute::Regex(regex_route.clone()));
                }
            }
        }
        drop(regex_routes);
        
        // Step 3: Fall back to prefix match
        let route_entry = self.match_engine.prefix_match(session)?;
        tracing::debug!(path=%path,"prefix match ok");
        
        // Convert RouteEntry back to HttpRouteRuleUnit
        let route_entry_id = route_entry.identifier();
        let route_rules = self.route_rules_list.read().unwrap();
        if let Some(unit) = route_rules.iter().find(|u| format!("{}/{}", u.namespace, u.name) == route_entry_id) {
            return Ok(MatchedRoute::Normal(Arc::new(unit.clone())));
        }
        
        // Should not happen
        Err(crate::types::err::EdError::RouteNotFound())
    }
}

pub struct DomainRouteRules {
    pub(crate) domain_routes_map: ArcSwap<Arc<HashMap<DomainStr, Arc<RouteRules>>>>,
}

impl DomainRouteRules {
    /// Match a route for the given hostname and session
    /// Returns the matched route if found, or an error if no route matches
    pub fn match_route(
        &self,
        hostname: &str,
        session: &mut pingora_proxy::Session,
    ) -> Result<MatchedRoute, crate::types::err::EdError> {
        let domain_routes_map = self.domain_routes_map.load();
        
        // Try to find RouteRules for the hostname (exact match only)
        let route_rules = domain_routes_map
            .get(hostname)
            .cloned();

        if let Some(route_rules) = route_rules {
            route_rules.match_route(session)
        } else {
            Err(crate::types::err::EdError::RouteNotFound())
        }
    }
}

type GatewayKey = String;
type RouteKey = String; // Format: "namespace/name"

pub struct RouteManager {
    /// Maps gateway key to domain route rules
    pub(crate) gateway_routes_map: DashMap<GatewayKey, Arc<DomainRouteRules>>,
    
    /// Stores all HTTPRoute resources for lookup during delete events
    /// Key format: "namespace/name"
    /// Uses Mutex since route updates are serialized (no concurrent writes needed)
    pub(crate) http_routes: Mutex<HashMap<RouteKey, HTTPRoute>>,
}

// Global RouteManager instance
static GLOBAL_ROUTE_MANAGER: Lazy<Arc<RouteManager>> = 
    Lazy::new(|| Arc::new(RouteManager::new()));

/// Get the global RouteManager instance
pub fn get_global_route_manager() -> Arc<RouteManager> {
    GLOBAL_ROUTE_MANAGER.clone()
}

impl RouteManager {
    pub fn new() -> Self {
        Self {
            gateway_routes_map: DashMap::new(),
            http_routes: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create DomainRouteRules for a specific gateway by namespace and name
    /// This ensures the gateway has a route map even if no HTTPRoutes exist yet
    pub fn get_or_create_domain_routes(&self, namespace: &str, name: &str) -> Arc<DomainRouteRules> {
        let gateway_key = format!("{}/{}", namespace, name);
        
        let entry = self.gateway_routes_map.entry(gateway_key.clone());
        let is_new = matches!(entry, dashmap::mapref::entry::Entry::Vacant(_));
        
        let domain_routes = entry
            .or_insert_with(|| Arc::new(DomainRouteRules {
                domain_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
            }))
            .value()
            .clone();
        
        if is_new {
            tracing::info!(
                gateway_key = %gateway_key,
                "Created new domain routes for gateway"
            );
        }
        
        domain_routes
    }

    pub fn add_http_routes(&self, http_routes: Vec<HTTPRoute>) {
        for route in http_routes {
            self.add_http_route_single(route);
        }
    }

    pub fn add_http_route(&self, route: HTTPRoute) {
        self.add_http_route_single(route);
    }

    fn add_http_route_single(&self, route: HTTPRoute) {
        let parent_refs = match &route.spec.parent_refs {
            Some(refs) => refs,
            None => {
                tracing::warn!("HTTPRoute '{}' has no parent_refs, skipping", route.key_name());
                return;
            }
        };

        // Store the HTTPRoute for later lookup (e.g., during delete events)
        {
            let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            let name = route.metadata.name.as_deref().unwrap_or("");
            let key = format!("{}/{}", namespace, name);
            self.http_routes.lock().unwrap().insert(key, route.clone());
        }

        let gateway_store = get_global_gateway_store();
        let gateway_store_guard = gateway_store.read().unwrap();

        for parent_ref in parent_refs {
            let gateway_key = Self::build_gateway_key(parent_ref, &route);
            
            // Check if gateway exists in global store
            match gateway_store_guard.get_gateway(&gateway_key) {
                Ok(_gateway) => {
                    let domain_routes_map = self.get_or_create_domain_routes_map(&gateway_key);
                    Self::add_rules_to_domain_map(&domain_routes_map, &route);
                    
                    tracing::info!(
                        "Added HTTPRoute '{}' to gateway '{}'",
                        route.key_name(),
                        gateway_key
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Gateway '{}' referenced by HTTPRoute '{}' not found in store: {}",
                        gateway_key,
                        route.key_name(),
                        e
                    );
                }
            }
        }
    }

    /// Build gateway key from parent reference
    fn build_gateway_key(parent_ref: &crate::types::resources::http_route::ParentReference, route: &HTTPRoute) -> String {
        if let Some(namespace) = parent_ref.namespace.as_ref() {
            format!("{}/{}", namespace, parent_ref.name)
        } else {
            // If namespace is not specified, use the route's namespace
            if let Some(route_namespace) = route.metadata.namespace.as_ref() {
                format!("{}/{}", route_namespace, parent_ref.name)
            } else {
                parent_ref.name.clone()
            }
        }
    }

    /// Get or create DomainRouteRules for the given gateway key
    fn get_or_create_domain_routes_map(&self, gateway_key: &str) -> Arc<DomainRouteRules> {
        self.gateway_routes_map
            .entry(gateway_key.to_string())
            .or_insert_with(|| Arc::new(DomainRouteRules {
                domain_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
            }))
            .value()
            .clone()
    }

    /// Add all rules from HTTPRoute to the domain routes map
    fn add_rules_to_domain_map(domain_routes_map: &DomainRouteRules, route: &HTTPRoute) {
        if let Some(rules) = &route.spec.rules {
            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
            let route_name = route.metadata.name.as_deref().unwrap_or("").to_string();
            
            for rule in rules {
                if let Some(hostnames) = &route.spec.hostnames {
                    if hostnames.is_empty() {
                        tracing::warn!(
                            "HTTPRoute '{}/{}' has empty hostnames list, skipping rule",
                            route_namespace,
                            route_name
                        );
                        continue;
                    }
                    for hostname in hostnames {
                        Self::add_rule_to_route_rules(
                            domain_routes_map,
                            &hostname,
                            &route_namespace,
                            &route_name,
                            rule,
                        );
                    }
                } else {
                    // Hostnames are required, skip routes without hostnames
                    tracing::warn!(
                        "HTTPRoute '{}/{}' has no hostnames specified, skipping rule",
                        route_namespace,
                        route_name
                    );
                }
            }
        }
    }

    /// Add a single rule to RouteRules for the given hostname
    fn add_rule_to_route_rules(
        domain_routes_map: &DomainRouteRules,
        hostname: &str,
        route_namespace: &str,
        route_name: &str,
        rule: &crate::types::HTTPRouteRule,
    ) {
        let hostname_key = hostname.to_string();
        
        // Use ArcSwap::rcu for lock-free update
        domain_routes_map.domain_routes_map.rcu(|current_map| {
            let current_hashmap: &HashMap<DomainStr, Arc<RouteRules>> = current_map.as_ref();
            let mut new_hashmap = current_hashmap.clone();
            
            // Get or create RouteRules for this hostname
            let route_rules_arc = new_hashmap
                .entry(hostname_key.clone())
                .or_insert_with(|| Arc::new(RouteRules {
                    resource_keys: RwLock::new(HashSet::new()),
                    route_rules_list: RwLock::new(Vec::new()),
                    match_engine: Arc::new(RadixRouteMatchEngine::default()),
                    regex_routes: RwLock::new(Vec::new()),
                }));
            
            // Get mutable access to RouteRules
            let route_rules_mut = Arc::make_mut(route_rules_arc);
            
            // Create HttpRouteRuleUnit from HTTPRouteRule
            // Each rule may have multiple matches
            let resource_key = format!("{}/{}", route_namespace, route_name);
            
            if let Some(matches) = &rule.matches {
                for match_item in matches {
                    let rule_unit = HttpRouteRuleUnit::new(
                        route_namespace.to_string(),
                        route_name.to_string(),
                        resource_key.clone(),
                        match_item.clone(),
                        Arc::new(rule.clone()),
                    );
                    
                    // Add the rule to the list
                    route_rules_mut.route_rules_list.write().unwrap().push(rule_unit);
                }
            } else {
                // If no matches, create a default match (match all)
                let default_match = crate::types::HTTPRouteMatch {
                    path: None,
                    headers: None,
                    query_params: None,
                    method: None,
                };
                let rule_unit = HttpRouteRuleUnit::new(
                    route_namespace.to_string(),
                    route_name.to_string(),
                    resource_key.clone(),
                    default_match,
                    Arc::new(rule.clone()),
                );
                
                // Add the rule to the list
                route_rules_mut.route_rules_list.write().unwrap().push(rule_unit);
            }
            
            // Add the resource key to the set (only once per route)
            route_rules_mut.resource_keys.write().unwrap().insert(resource_key);
            
            // Rebuild match_engine with updated route rules
            // Collect route entries while holding the read lock
            let route_entries: Vec<Arc<dyn crate::core::routes::match_engine::RouteEntry>> = {
                let route_rules_list = route_rules_mut.route_rules_list.read().unwrap();
                route_rules_list
                    .iter()
                    .map(|unit| Arc::new(unit.clone()) as Arc<dyn crate::core::routes::match_engine::RouteEntry>)
                    .collect()
            };
            
            // Build new match_engine engine and directly replace it (no lock needed)
            match RadixRouteMatchEngine::build(route_entries.clone()) {
                Ok(new_engine) => {
                    route_rules_mut.match_engine = Arc::new(new_engine);
                    tracing::info!(
                        "Rebuilt RadixRouteMatchEngine for hostname '{}' with {} routes",
                        hostname,
                        route_entries.len()
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to rebuild RadixRouteMatchEngine for hostname '{}': {}",
                        hostname,
                        e
                    );
                }
            }
            
            Arc::new(new_hashmap)
        });
    }
    
    /// Remove an HTTPRoute by namespace and name
    /// This method uses the stored HTTPRoute to find all affected domains
    pub fn remove_http_route(&self, namespace: &str, name: &str) {
        // Get and remove the stored HTTPRoute to find which domains and gateways it was bound to
        let old_route = {
            let key = format!("{}/{}", namespace, name);
            let mut routes = self.http_routes.lock().unwrap();
            match routes.remove(&key) {
                Some(route) => route,
                None => {
                    tracing::warn!(
                        "HTTPRoute '{}/{}' not found in stored routes, cannot determine affected domains",
                        namespace, name
                    );
                    return;
                }
            }
        };
        
        tracing::info!("Removing HTTPRoute '{}/{}' from all bound gateways", namespace, name);
        
        let parent_refs = match &old_route.spec.parent_refs {
            Some(refs) => refs,
            None => {
                tracing::warn!("HTTPRoute '{}/{}' has no parent_refs", namespace, name);
                return;
            }
        };
        
        let gateway_store = get_global_gateway_store();
        let gateway_store_guard = gateway_store.read().unwrap();
        
        for parent_ref in parent_refs {
            let gateway_key = Self::build_gateway_key(parent_ref, &old_route);
            
            // Check if gateway exists
            if gateway_store_guard.get_gateway(&gateway_key).is_err() {
                tracing::warn!(
                    "Gateway '{}' referenced by HTTPRoute '{}/{}' not found",
                    gateway_key, namespace, name
                );
                continue;
            }
            
            // Get the domain routes map for this gateway
            if let Some(domain_routes_map) = self.gateway_routes_map.get(&gateway_key) {
                Self::remove_rules_from_domain_map(&domain_routes_map, &old_route);
                
                tracing::info!(
                    "Removed HTTPRoute '{}/{}' from gateway '{}'",
                    namespace, name, gateway_key
                );
            }
        }
    }
    
    /// Remove all rules from an HTTPRoute from the domain routes map
    fn remove_rules_from_domain_map(domain_routes_map: &DomainRouteRules, route: &HTTPRoute) {
        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        let resource_key = format!("{}/{}", route_namespace, route_name);
        
        // Get all hostnames from the route
        let hostnames = match &route.spec.hostnames {
            Some(hostnames) if !hostnames.is_empty() => hostnames,
            _ => {
                tracing::debug!("HTTPRoute '{}/{}' has no hostnames, nothing to remove", route_namespace, route_name);
                return;
            }
        };
        
        // Remove rules from each hostname
        for hostname in hostnames {
            Self::remove_route_from_hostname(domain_routes_map, hostname, &resource_key);
        }
    }
    
    /// Remove a specific route from a hostname's RouteRules
    fn remove_route_from_hostname(
        domain_routes_map: &DomainRouteRules,
        hostname: &str,
        resource_key: &str,
    ) {
        domain_routes_map.domain_routes_map.rcu(|current_map| {
            let current_hashmap: &HashMap<DomainStr, Arc<RouteRules>> = current_map.as_ref();
            let mut new_hashmap = current_hashmap.clone();
            
            // Get the RouteRules for this hostname
            if let Some(route_rules_arc) = new_hashmap.get_mut(hostname) {
                let route_rules_mut = Arc::make_mut(route_rules_arc);
                
                // Remove all rules matching this resource_key
                {
                    let mut route_rules_list = route_rules_mut.route_rules_list.write().unwrap();
                    let original_len = route_rules_list.len();
                    route_rules_list.retain(|unit| unit.resource_key != resource_key);
                    let removed_count = original_len - route_rules_list.len();
                    
                    if removed_count > 0 {
                        tracing::debug!(
                            "Removed {} rule(s) for route '{}' from hostname '{}'",
                            removed_count, resource_key, hostname
                        );
                    }
                }
                
                // Rebuild match_engine with remaining rules
                let route_entries: Vec<Arc<dyn crate::core::routes::match_engine::RouteEntry>> = {
                    let route_rules_list = route_rules_mut.route_rules_list.read().unwrap();
                    route_rules_list
                        .iter()
                        .map(|unit| Arc::new(unit.clone()) as Arc<dyn crate::core::routes::match_engine::RouteEntry>)
                        .collect()
                };
                
                if route_entries.is_empty() {
                    // No more routes for this hostname, remove it
                    new_hashmap.remove(hostname);
                    tracing::info!("Removed hostname '{}' (no more routes)", hostname);
                } else {
                    // Rebuild match_engine with remaining routes
                    match RadixRouteMatchEngine::build(route_entries.clone()) {
                        Ok(new_engine) => {
                            route_rules_mut.match_engine = Arc::new(new_engine);
                            tracing::info!(
                                "Rebuilt RadixRouteMatchEngine for hostname '{}' with {} remaining routes",
                                hostname,
                                route_entries.len()
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to rebuild RadixRouteMatchEngine for hostname '{}': {}",
                                hostname, e
                            );
                        }
                    }
                }
            }
            
            Arc::new(new_hashmap)
        });
    }
}