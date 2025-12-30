use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::http_routes::{RouteManager, HttpRouteRuleUnit, get_global_route_manager};
use crate::core::routes::http_routes::routes_mgr::RouteRules;
use crate::core::routes::http_routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::http_routes::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::routes::http_routes::lb_policy_sync::{sync_lb_policies_for_routes, cleanup_lb_policies_for_routes};
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::types::{HTTPRoute, ResourceMeta, HTTPRouteMatch, HTTPRouteRule};
use regex::Regex;
use crate::core::matcher::host_match::radix_match::{RadixHostMatchEngine, RadixHost};

type GatewayKey = String;
type DomainStr = String;

/// Implement ConfHandler for Arc<RouteManager> to allow using the global instance
impl ConfHandler<HTTPRoute> for Arc<RouteManager> {
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        // Extract and update LB policies from all routes
        sync_lb_policies_for_routes(data);
        
        (**self).full_set(data)
    }

    fn partial_update(&self, add: HashMap<String, HTTPRoute>, update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        // Merge add and update for policy extraction
        let mut add_or_update = add.clone();
        add_or_update.extend(update.clone());
        
        // Extract and update LB policies from add/update routes
        sync_lb_policies_for_routes(&add_or_update);
        
        // Clean up LB policies for removed routes
        cleanup_lb_policies_for_routes(&remove);
        
        (**self).partial_update(add, update, remove)
    }
}

/// Create a RouteManager handler for registration with ConfigClient
/// Returns the global RouteManager instance
pub fn create_route_manager_handler() -> Box<dyn ConfHandler<HTTPRoute> + Send + Sync> {
    Box::new(get_global_route_manager())
}

/// Private helper methods for RouteManager
impl RouteManager {
    /// Check if the path match is a regex type
    fn is_regex_path(match_item: &HTTPRouteMatch) -> bool {
        if let Some(ref path_match) = match_item.path {
            if let Some(ref match_type) = path_match.match_type {
                return match_type == "RegularExpression";
            }
        }
        false
    }
    
    /// Create a regex route unit from match_item
    fn create_regex_route_unit(
        namespace: &str,
        name: &str,
        rule_id: usize,
        match_id: usize,
        resource_key: &str,
        match_item: &HTTPRouteMatch,
        rule: Arc<HTTPRouteRule>,
        parent_refs: Option<Vec<crate::types::resources::common::ParentReference>>,
    ) -> Result<HttpRouteRuleUnit, String> {
        let path_value = match_item.path.as_ref()
            .and_then(|p| p.value.as_deref())
            .ok_or_else(|| "Regex path must have value".to_string())?;
        
        let regex = Regex::new(path_value)
            .map_err(|e| format!("Invalid regex '{}': {}", path_value, e))?;
        
        Ok(HttpRouteRuleUnit::new(
            namespace.to_string(),
            name.to_string(),
            rule_id,
            match_id,
            resource_key.to_string(),
            match_item.clone(),
            rule,
            Some(regex),
            parent_refs,
        ))
    }
}

impl RouteManager {
    /// Build gateway_hostnames map from add_or_update and remove sets
    /// Returns a map of gateway_key -> set of affected hostnames
    /// 
    /// For updated routes, this includes both old and new hostnames to ensure
    /// old hostnames are properly cleaned up when they're removed.
    fn build_gateway_hostnames_map(
        &self,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) -> HashMap<String, HashSet<String>> {
        let mut gateway_hostnames: HashMap<String, HashSet<String>> = HashMap::new();
        
        // Get http_routes lock once for efficiency
        let http_routes = self.http_routes.lock().unwrap();
        
        // Process add_or_update routes
        // For updates, we need to include both old and new hostnames
        for (resource_key, route) in add_or_update.iter() {
            // Check if this is an update (route already exists)
            let old_route = http_routes.get(resource_key);
            
            // Collect hostnames from both old and new routes
            let mut all_hostnames = HashSet::new();
            
            // Add new hostnames
            if let Some(hostnames) = &route.spec.hostnames {
                for hostname in hostnames {
                    all_hostnames.insert(hostname.clone());
                }
            }
            
            // Add old hostnames (if this is an update)
            if let Some(old_route) = old_route {
                if let Some(old_hostnames) = &old_route.spec.hostnames {
                    for hostname in old_hostnames {
                        all_hostnames.insert(hostname.clone());
                    }
                }
            }
            
            // Process parent_refs and add all hostnames to gateway_hostnames
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
                    for hostname in &all_hostnames {
                        hostname_set.insert(hostname.clone());
                    }
                }
            }
        }
        
        drop(http_routes); // Release lock before processing remove routes
        
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

    /// Rebuild RouteRules for a single exact hostname (fine-grained update)
    /// Returns new RouteRules for the hostname, or None if no routes remain
    fn rebuild_exact_hostname(
        &self,
        hostname: &str,
        gateway_key: &str,
        remove: &HashSet<String>,
    ) -> Option<Arc<RouteRules>> {
        let http_routes = self.http_routes.lock().unwrap();
        
        let mut route_rules_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        let mut regex_routes_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        let mut resource_keys: HashSet<String> = HashSet::new();
        
        // Collect all routes that apply to this hostname and gateway
        for (resource_key, route) in http_routes.iter() {
            // Skip routes that are being removed
            if remove.contains(resource_key) {
                continue;
            }
            
            // Check if this route applies to this hostname
            let applies_to_hostname = route.spec.hostnames
                .as_ref()
                .map(|hostnames| hostnames.contains(&hostname.to_string()))
                .unwrap_or(false);
            
            if !applies_to_hostname {
                continue;
            }
            
            // Check if this route applies to this gateway
            let applies_to_gateway = route.spec.parent_refs.as_ref().map(|parent_refs| {
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                parent_refs.iter().any(|parent_ref| {
                    parent_ref.build_parent_key(Some(route_namespace)) == gateway_key
                })
            }).unwrap_or(false);
            
            if !applies_to_gateway {
                continue;
            }
            
            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            let route_name = route.metadata.name.as_deref().unwrap_or("");
            
            if let Some(rules) = &route.spec.rules {
                for (rule_id, rule) in rules.iter().enumerate() {
                    let rule_arc = Arc::new(rule.clone());
                    
                    if let Some(matches) = &rule.matches {
                        for (match_id, match_item) in matches.iter().enumerate() {
                            if Self::is_regex_path(match_item) {
                                if let Ok(regex_unit) = Self::create_regex_route_unit(
                                    route_namespace,
                                    route_name,
                                    rule_id,
                                    match_id,
                                    resource_key,
                                    match_item,
                                    rule_arc.clone(),
                                    route.spec.parent_refs.clone(),
                                ) {
                                    regex_routes_list.push(Arc::new(regex_unit));
                                }
                            } else {
                                let rule_unit = HttpRouteRuleUnit::new(
                                    route_namespace.to_string(),
                                    route_name.to_string(),
                                    rule_id,
                                    match_id,
                                    resource_key.clone(),
                                    match_item.clone(),
                                    rule_arc.clone(),
                                    None,
                                    route.spec.parent_refs.clone(),
                                );
                                route_rules_list.push(Arc::new(rule_unit));
                            }
                        }
                    }
                }
            }
            
            resource_keys.insert(resource_key.clone());
        }
        
        // If no routes remain, return None
        if route_rules_list.is_empty() && regex_routes_list.is_empty() {
            return None;
        }
        
        // Build match engine for exact/prefix routes
        let match_engine = if route_rules_list.is_empty() {
            None
        } else {
            match RadixRouteMatchEngine::build(route_rules_list.clone()) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(e) => {
                    tracing::error!(component="route_manager",hostname=%hostname,err=%e,"rebuild match_engine failed");
                    return None;
                }
            }
        };
        
        // Build regex routes engine
        let regex_routes_engine = (!regex_routes_list.is_empty())
            .then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));
        
        // Create RouteRules
        Some(Arc::new(RouteRules {
            resource_keys: RwLock::new(resource_keys),
            route_rules_list: RwLock::new(route_rules_list),
            match_engine,
            regex_routes: RwLock::new(regex_routes_list),
            regex_routes_engine,
        }))
    }

    /// Rebuild RadixHostMatchEngine for wildcard domains of a gateway (with Arc reuse optimization)
    /// Returns a new RadixHostMatchEngine with ALL wildcard hostnames for this gateway
    /// Excludes routes in the `remove` set
    /// Returns None if no wildcard domains remain
    /// 
    /// Optimization: Reuses Arc<RouteRules> from existing engine for unchanged wildcard hostnames
    fn rebuild_gateway_wildcard_engine(
        &self,
        gateway_key: &str,
        affected_hostnames: &HashSet<String>,
        remove: &HashSet<String>,
        current_engine: Option<&RadixHostMatchEngine<RouteRules>>,
    ) -> Result<Option<RadixHostMatchEngine<RouteRules>>, String> {
        let http_routes = self.http_routes.lock().unwrap();
        
        // Step 1: Export existing RadixHosts (shallow copy of Arc pointers)
        let mut existing_hosts_map: HashMap<String, RadixHost<RouteRules>> = HashMap::new();
        if let Some(engine) = current_engine {
            let existing_hosts = engine.export_hosts();
            for host in existing_hosts {
                existing_hosts_map.insert(host.original.to_lowercase(), host);
            }
        }
        
        // Step 2: Collect all wildcard hostnames for this gateway from http_routes
        let mut gateway_wildcard_hostnames: HashSet<String> = HashSet::new();
        for (resource_key, route) in http_routes.iter() {
            // Skip routes being removed
            if remove.contains(resource_key) {
                continue;
            }
            
            // Check if route belongs to this gateway
            if let Some(parent_refs) = &route.spec.parent_refs {
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                for parent_ref in parent_refs {
                    let parent_key = parent_ref.build_parent_key(Some(route_namespace));
                    if parent_key == gateway_key {
                        // Collect wildcard hostnames from this route
                        if let Some(hostnames) = &route.spec.hostnames {
                            for hostname in hostnames {
                                if hostname.starts_with("*.") {
                                    gateway_wildcard_hostnames.insert(hostname.clone());
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }
        
        // Step 3: Build final RadixHost list (reuse or rebuild)
        let mut radix_hosts: Vec<RadixHost<RouteRules>> = Vec::new();
        
        for hostname in gateway_wildcard_hostnames.iter() {
            // If hostname is not affected, reuse existing RadixHost (Arc reuse!)
            if !affected_hostnames.contains(hostname) {
                if let Some(existing_host) = existing_hosts_map.get(&hostname.to_lowercase()) {
                    radix_hosts.push(existing_host.clone()); // Shallow copy of Arc
                    tracing::trace!(component="route_manager",hostname=%hostname,"reused existing RadixHost");
                    continue;
                }
            }
            
            // Hostname is affected or new, need to rebuild
            // Collect all routes that apply to this hostname
            let mut route_rules_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
            let mut regex_routes_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
            let mut resource_keys: HashSet<String> = HashSet::new();
            
            for (resource_key, route) in http_routes.iter() {
                // Skip routes that are being removed
                if remove.contains(resource_key) {
                    continue;
                }
                
                // Check if this route applies to this hostname
                let applies = route.spec.hostnames
                    .as_ref()
                    .map(|hostnames| hostnames.contains(&hostname.to_string()))
                    .unwrap_or(false);
                
                if !applies {
                    continue;
                }
                
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                let route_name = route.metadata.name.as_deref().unwrap_or("");
                
                if let Some(rules) = &route.spec.rules {
                    for (rule_id, rule) in rules.iter().enumerate() {
                        let rule_arc = Arc::new(rule.clone());
                        
                        if let Some(matches) = &rule.matches {
                            for (match_id, match_item) in matches.iter().enumerate() {
                                if Self::is_regex_path(match_item) {
                                    if let Ok(regex_unit) = Self::create_regex_route_unit(
                                        route_namespace,
                                        route_name,
                                        rule_id,
                                        match_id,
                                        resource_key,
                                        match_item,
                                        rule_arc.clone(),
                                        route.spec.parent_refs.clone(),
                                    ) {
                                        regex_routes_list.push(Arc::new(regex_unit));
                                    }
                                } else {
                                    let rule_unit = HttpRouteRuleUnit::new(
                                        route_namespace.to_string(),
                                        route_name.to_string(),
                                        rule_id,
                                        match_id,
                                        resource_key.clone(),
                                        match_item.clone(),
                                        rule_arc.clone(),
                                        None,
                                        route.spec.parent_refs.clone(),
                                    );
                                    route_rules_list.push(Arc::new(rule_unit));
                                }
                            }
                        }
                    }
                }
                
                resource_keys.insert(resource_key.clone());
            }
            
            // Skip if no routes for this hostname
            if route_rules_list.is_empty() && regex_routes_list.is_empty() {
                continue;
            }
            
            // Build match engine for exact/prefix routes
            let match_engine = if route_rules_list.is_empty() {
                None
            } else {
                match RadixRouteMatchEngine::build(route_rules_list.clone()) {
                    Ok(engine) => Some(Arc::new(engine)),
                    Err(e) => {
                        tracing::error!(component="route_manager",hostname=%hostname,err=%e,"rebuild match_engine failed");
                        continue;
                    }
                }
            };
            
            // Build regex routes engine
            let regex_routes_engine = (!regex_routes_list.is_empty())
                .then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));
            
            // Create RouteRules
            let route_rules = Arc::new(RouteRules {
                resource_keys: RwLock::new(resource_keys),
                route_rules_list: RwLock::new(route_rules_list),
                match_engine,
                regex_routes: RwLock::new(regex_routes_list),
                regex_routes_engine,
            });
            
            // Add to radix hosts
            radix_hosts.push(RadixHost::new(hostname, route_rules));
        }
        
        // Build RadixHostMatchEngine only if there are wildcard domains
        if radix_hosts.is_empty() {
            Ok(None)
        } else {
            let mut new_engine = RadixHostMatchEngine::new();
            new_engine.initialize(radix_hosts)?;
            Ok(Some(new_engine))
        }
    }

    /// Update a single hostname's RouteRules in the given HashMap (legacy, kept for reference)
    /// This modifies the HashMap in place and does NOT do RCU
    #[allow(dead_code)]
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
        
        // Step 2: Add/update/remove resource keys from add_or_update
        // For updates, if a route no longer applies to this hostname, remove it
        for (resource_key, route) in add_or_update.iter() {
            // Check if this route applies to this hostname
            let applies = route.spec.hostnames
                .as_ref()
                .map(|hostnames| hostnames.contains(&hostname.to_string()))
                .unwrap_or(false);
            
            if applies {
                resource_keys.insert(resource_key.clone());
                tracing::debug!(component="route_manager",hostname=%hostname,key=%resource_key,"add/update key");
            } else {
                // Route no longer applies to this hostname (e.g., hostname was removed from route)
                // Remove it from resource_keys if it exists
                if resource_keys.remove(resource_key) {
                    tracing::debug!(component="route_manager",hostname=%hostname,key=%resource_key,"rm key (no longer applies)");
                }
            }
        }
        
        // Step 3: Rebuild route_rules_list and match_engine from resource_keys
        // Rebuild from http_routes storage
        let mut route_rules_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        let mut regex_routes_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        
        let http_routes = self.http_routes.lock().unwrap();
        for resource_key in resource_keys.iter() {
            if let Some(route) = http_routes.get(resource_key) {
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                let route_name = route.metadata.name.as_deref().unwrap_or("");
                
                if let Some(rules) = &route.spec.rules {
                    for (rule_id, rule) in rules.iter().enumerate() {
                        let rule_arc = Arc::new(rule.clone());
                        
                        // Each rule may have multiple matches
                        if let Some(matches) = &rule.matches {
                            for (match_id, match_item) in matches.iter().enumerate() {
                                // Check if this is a regex path
                                if Self::is_regex_path(&match_item) {
                                    // Create regex route
                                    if let Ok(regex_unit) = Self::create_regex_route_unit(
                                        route_namespace,
                                        route_name,
                                        rule_id,
                                        match_id,
                                        resource_key,
                                        match_item,
                                        rule_arc.clone(),
                                        route.spec.parent_refs.clone(),
                                    ) {
                                        regex_routes_list.push(Arc::new(regex_unit));
                                    }
                                } else {
                                    // Create normal route (exact/prefix)
                                    let rule_unit = HttpRouteRuleUnit::new(
                                        route_namespace.to_string(),
                                        route_name.to_string(),
                                        rule_id,
                                        match_id,
                                        resource_key.clone(),
                                        match_item.clone(),
                                        rule_arc.clone(),
                                        None,
                                        route.spec.parent_refs.clone(),
                                    );
                                    route_rules_list.push(Arc::new(rule_unit));
                                }
                            }
                        } else {
                            tracing::warn!(route_name=%route_name, route_namespace=%route_namespace, "route missing match");
                        }
                    }
                }
            } else {
                tracing::warn!(component="route_manager",key=%resource_key,"key not found in storage");
            }
        }
        drop(http_routes);
        
        // Only remove hostname if both normal routes and regex routes are empty
        if route_rules_list.is_empty() && regex_routes_list.is_empty() {
            // No more routes for this hostname, remove it
            domain_hashmap.remove(hostname);
            tracing::info!(component="route_manager",hostname=%hostname,"rm hostname (no routes)");
            return;
        }
        
        // Build match engine for exact/prefix routes (only if there are normal routes)
        let match_engine = if route_rules_list.is_empty() {
            // Only regex routes, no need for match_engine
            None
        } else {
            // Directly pass HttpRouteRuleUnit, no type conversion needed!
            match RadixRouteMatchEngine::build(route_rules_list.clone()) {
                Ok(new_engine) => {
                    Some(Arc::new(new_engine))
                }
                Err(e) => {
                    tracing::error!(component="route_manager",hostname=%hostname,err=%e,"rebuild match_engine failed");
                    return;
                }
            }
        };
        
        // Build regex routes engine (only if there are regex routes)
        let regex_routes_engine = (!regex_routes_list.is_empty())
            .then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));
        
        let normal_routes_count = route_rules_list.len();
        let regex_routes_count = regex_routes_list.len();
        
        let new_route_rules = Arc::new(RouteRules {
            resource_keys: RwLock::new(resource_keys),
            route_rules_list: RwLock::new(route_rules_list),
            match_engine,
            regex_routes: RwLock::new(regex_routes_list),
            regex_routes_engine,
        });
        
        domain_hashmap.insert(hostname.to_string(), new_route_rules);
        tracing::debug!(
            component="route_manager",
            hostname=%hostname,
            normal_routes=normal_routes_count,
            regex_routes=regex_routes_count,
            "updated"
        );
    }
}

/// Parse all HTTPRoutes and collect rules into gateway->domain->rules structure
/// Returns HashMap<GatewayKey, HashMap<DomainStr, (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>)>>
/// The tuple contains (normal_routes, regex_routes), both using Arc<HttpRouteRuleUnit>
fn parse_http_routes_to_gateway_domain_rules(
    data: &HashMap<String, HTTPRoute>
) -> HashMap<GatewayKey, HashMap<DomainStr, (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>)>> {
    let mut gateway_domain_rules: HashMap<GatewayKey, HashMap<DomainStr, (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>)>> = HashMap::new();

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
            let gateway_key = parent_ref.build_parent_key(Some(&route_namespace));

            // Get or create the domain map for this gateway
            let domain_map = gateway_domain_rules
                .entry(gateway_key.clone())
                .or_insert_with(HashMap::new);

            // Process each hostname and rule combination
            for hostname in hostnames {
                for (rule_id, rule) in rules.iter().enumerate() {
                    let rule_arc = Arc::new(rule.clone());
                    
                    // Each rule may have multiple matches
                    if let Some(matches) = &rule.matches {
                        for (match_id, match_item) in matches.iter().enumerate() {
                            let split = domain_map
                                .entry(hostname.clone())
                                .or_insert_with(|| (Vec::new(), Vec::new()));
                            
                            // Check if this is a regex path
                            if RouteManager::is_regex_path(match_item) {
                                // Create regex route
                                match RouteManager::create_regex_route_unit(
                                    &route_namespace,
                                    &route_name,
                                    rule_id,
                                    match_id,
                                    &route.key_name(),
                                    match_item,
                                    rule_arc.clone(),
                                    route.spec.parent_refs.clone(),
                                ) {
                                    Ok(regex_unit) => {
                                        split.1.push(Arc::new(regex_unit));
                                    }
                                    Err(e) => {
                                        tracing::warn!(route=%route.key_name(),err=%e,"failed to create regex route");
                                    }
                                }
                            } else {
                                // Create normal route
                                let rule_unit = HttpRouteRuleUnit::new(
                                    route_namespace.clone(),
                                    route_name.clone(),
                                    rule_id,
                                    match_id,
                                    route.key_name(),
                                    match_item.clone(),
                                    rule_arc.clone(),
                                    None,
                                    route.spec.parent_refs.clone(),
                                );
                                split.0.push(Arc::new(rule_unit));
                            }
                        }
                    } else {
                        tracing::warn!(route_name=%route_name, route_namespace=%route_namespace, "route missing match");
                    }
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
    &Vec<crate::types::resources::common::ParentReference>,
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
    /// Full set with a complete set of HTTPRoutes
    /// This is typically called during initial sync or re-list
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        let start_time = Instant::now();
        tracing::info!(component="route_manager",cnt=data.len(),"full set start");

        // Step 0: Store all HTTPRoute resources for future lookups (e.g., during deletions)
        *self.http_routes.lock().unwrap() = data.clone();
        tracing::debug!(component="route_manager",cnt=data.len(),"stored http_routes");

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

            // Separate exact domains and wildcard domains
            let mut exact_domain_map: HashMap<DomainStr, Arc<RouteRules>> = HashMap::new();
            let mut wildcard_hosts: Vec<RadixHost<RouteRules>> = Vec::new();

            for (domain, split) in domain_rules_map.into_iter() {
                // Skip if both normal routes and regex routes are empty
                if split.0.is_empty() && split.1.is_empty() {
                    tracing::debug!(component="route_manager",gw=%gateway_key,domain=%domain,"skipping domain (no routes)");
                    continue;
                }
                
                // Build RadixRouteMatchEngine for normal routes (only if there are normal routes)
                let match_engine = if split.0.is_empty() {
                    // Only regex routes, no need for match_engine
                    None
                } else {
                    // Directly pass HttpRouteRuleUnit, no type conversion needed!
                    match RadixRouteMatchEngine::build(split.0.clone()) {
                        Ok(engine) => Some(Arc::new(engine)),
                        Err(e) => {
                            tracing::error!(component="route_manager",gw=%gateway_key,domain=%domain,err=?e,"build failed");
                            continue;
                        }
                    }
                };

                // Build regex routes engine (only if there are regex routes)
                let regex_routes_engine = (!split.1.is_empty())
                    .then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));

                // Collect resource keys for this domain (from both normal and regex routes)
                let mut resource_keys: HashSet<String> = split.0
                    .iter()
                    .map(|unit| unit.resource_key.clone())
                    .collect();
                resource_keys.extend(split.1.iter().map(|unit| unit.resource_key.clone()));
                
                // Create RouteRules
                let route_rules = Arc::new(RouteRules {
                    resource_keys: RwLock::new(resource_keys),
                    route_rules_list: RwLock::new(split.0),
                    match_engine,
                    regex_routes: RwLock::new(split.1),
                    regex_routes_engine,
                });

                // Distinguish exact vs wildcard domains
                if domain.starts_with("*.") {
                    // Wildcard domain - add to RadixHostMatchEngine
                    wildcard_hosts.push(RadixHost::new(&domain, route_rules));
                    tracing::trace!(component="route_manager",gw=%gateway_key,domain=%domain,"added to wildcard engine");
                } else {
                    // Exact domain - add to HashMap (lowercase for case-insensitive matching)
                    exact_domain_map.insert(domain.to_lowercase(), route_rules);
                    tracing::trace!(component="route_manager",gw=%gateway_key,domain=%domain,"added to exact map");
                }
            }

            // Get existing DomainRouteRules for this gateway (don't create new)
            let domain_route_rules = if let Some(entry) = self.gateway_routes_map.get(&gateway_key) {
                entry.value().clone()
            } else {
                tracing::debug!(component="route_manager",gw=%gateway_key,"gw not in routes map");
                skipped_gateways += 1;
                continue;
            };

            // Build and store exact domain map
            domain_route_rules.exact_domain_map.store(Arc::new(exact_domain_map));
            
            // Build and store wildcard engine (only if wildcard domains exist)
            let wildcard_engine = if !wildcard_hosts.is_empty() {
                let mut engine = RadixHostMatchEngine::new();
                if let Err(e) = engine.initialize(wildcard_hosts) {
                    tracing::error!(component="route_manager",gw=%gateway_key,err=%e,"failed to build RadixHostMatchEngine");
                    skipped_gateways += 1;
                    continue;
                }
                Some(engine)
            } else {
                None
            };
            domain_route_rules.wildcard_engine.store(Arc::new(wildcard_engine));

            processed_gateways += 1;
        }

        let elapsed = start_time.elapsed();
        tracing::info!(component="route_manager",total=processed_gateways+skipped_gateways,proc=processed_gateways,skip=skipped_gateways,ms=elapsed.as_millis(),"full set done");
    }

    /// Handle partial configuration updates
    /// Processes additions, updates, and removals of HTTPRoutes
    /// Uses fine-grained updates for exact domains and batch updates for wildcard domains
    fn partial_update(&self, add: HashMap<String, HTTPRoute>, update: HashMap<String, HTTPRoute>, remove: HashSet<String>) {
        tracing::info!(
            component = "route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "Processing HTTPRoute changes"
        );

        // Merge add and update for processing
        let mut add_or_update = add;
        add_or_update.extend(update);

        // Step 0: Build gateway_hostnames map BEFORE updating http_routes storage
        // This is important because we need to access old hostnames from existing routes
        // before they are overwritten with new data
        let gateway_hostnames = self.build_gateway_hostnames_map(&add_or_update, &remove);
        
        // Step 1: Update http_routes storage (after building hostnames map, so we have the latest data for rebuilding)
        {
            let mut routes = self.http_routes.lock().unwrap();
            for (key, route) in add_or_update.iter() {
                routes.insert(key.clone(), route.clone());
                tracing::debug!(component = "route_manager",route_key = %key,"add/update HTTPRoute");
            }
        }

        // Step 2: For each gateway, separate exact and wildcard hostnames, then update accordingly
        for (gateway_key, affected_hostnames) in gateway_hostnames.iter() {
            tracing::debug!(component="route_manager",gw=%gateway_key,affected=affected_hostnames.len(),"processing affected hostnames");
            
            if let Some(domain_routes_ref) = self.gateway_routes_map.get(gateway_key) {
                // Separate exact and wildcard hostnames
                let mut exact_hostnames: HashSet<String> = HashSet::new();
                let mut wildcard_hostnames: HashSet<String> = HashSet::new();
                
                for hostname in affected_hostnames {
                    if hostname.starts_with("*.") {
                        wildcard_hostnames.insert(hostname.clone());
                    } else {
                        exact_hostnames.insert(hostname.clone());
                    }
                }
                
                // Update exact domains (fine-grained, RCU pattern)
                if !exact_hostnames.is_empty() {
                    let current_map = domain_routes_ref.exact_domain_map.load();
                    let mut new_map = (**current_map).clone(); // Clone HashMap
                    
                    for hostname in exact_hostnames.iter() {
                        match self.rebuild_exact_hostname(hostname, gateway_key, &remove) {
                            Some(new_route_rules) => {
                                // Store with lowercase key for case-insensitive matching
                                new_map.insert(hostname.to_lowercase(), new_route_rules);
                                tracing::debug!(component="route_manager",gw=%gateway_key,hostname=%hostname,"updated exact domain");
                            }
                            None => {
                                new_map.remove(&hostname.to_lowercase());
                                tracing::debug!(component="route_manager",gw=%gateway_key,hostname=%hostname,"removed exact domain (no routes)");
                            }
                        }
                    }
                    
                    // Atomically replace the exact domain map
                    domain_routes_ref.exact_domain_map.store(Arc::new(new_map));
                    tracing::info!(component="route_manager",gw=%gateway_key,cnt=exact_hostnames.len(),"exact domains updated");
                }
                
                // Update wildcard domains (rebuild engine with Arc reuse)
                if !wildcard_hostnames.is_empty() {
                    let current_engine_opt = domain_routes_ref.wildcard_engine.load();
                    let current_engine = current_engine_opt.as_ref().as_ref();
                    
                    match self.rebuild_gateway_wildcard_engine(gateway_key, &wildcard_hostnames, &remove, current_engine) {
                        Ok(new_engine_opt) => {
                            domain_routes_ref.wildcard_engine.store(Arc::new(new_engine_opt));
                            tracing::info!(component="route_manager",gw=%gateway_key,cnt=wildcard_hostnames.len(),"wildcard engine rebuilt with Arc reuse");
                        }
                        Err(e) => {
                            tracing::error!(component="route_manager",gw=%gateway_key,err=%e,"failed to rebuild wildcard engine");
                        }
                    }
                }
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
        
        tracing::info!( component = "route_manager", "HTTPRoute changes processed successfully");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Gateway, HTTPRoute};
    
    /// Helper function to create a test Gateway from JSON
    fn create_test_gateway(namespace: &str, name: &str, hostnames: Vec<&str>) -> Gateway {
        let listeners_json: Vec<serde_json::Value> = hostnames.iter().map(|h| {
            serde_json::json!({
                "name": format!("listener-{}", h),
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
    
    /// Helper function to create a test HTTPRoute from JSON
    fn create_test_httproute(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>, // (namespace, name)
    ) -> HTTPRoute {
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs.iter().map(|(ns, n)| {
            serde_json::json!({
                "group": "gateway.networking.k8s.io",
                "kind": "Gateway",
                "namespace": ns,
                "name": n
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
                    "matches": []
                }]
            }
        });
        
        serde_json::from_value(json).expect("Failed to create HTTPRoute")
    }
    
    #[test]
    fn test_build_gateway_hostnames_map_with_add_routes() {
        let mgr = RouteManager::new();
        
        // Create test routes
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        let route2 = create_test_httproute("default", "route2", vec!["web.example.com"], vec![("default", "gateway1")]);
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        
        let remove = HashSet::new();
        
        // Build gateway_hostnames map
        let result = mgr.build_gateway_hostnames_map(&add_or_update, &remove);
        
        // Verify
        assert_eq!(result.len(), 1);
        let hostnames = result.get("default/gateway1").unwrap();
        assert_eq!(hostnames.len(), 2);
        assert!(hostnames.contains("api.example.com"));
        assert!(hostnames.contains("web.example.com"));
    }
    
    #[test]
    fn test_build_gateway_hostnames_map_with_remove_routes() {
        let mgr = RouteManager::new();
        
        // First add a route
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        mgr.http_routes.lock().unwrap().insert("default/route1".to_string(), route1);
        
        // Now test remove
        let add_or_update = HashMap::new();
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        
        // Build gateway_hostnames map
        let result = mgr.build_gateway_hostnames_map(&add_or_update, &remove);
        
        // Verify
        assert_eq!(result.len(), 1);
        let hostnames = result.get("default/gateway1").unwrap();
        assert_eq!(hostnames.len(), 1);
        assert!(hostnames.contains("api.example.com"));
    }
    
    #[test]
    fn test_build_gateway_hostnames_map_with_multiple_gateways() {
        let mgr = RouteManager::new();
        
        // Create test routes targeting different gateways
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        let route2 = create_test_httproute("default", "route2", vec!["web.example.com"], vec![("default", "gateway2")]);
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        
        let remove = HashSet::new();
        
        // Build gateway_hostnames map
        let result = mgr.build_gateway_hostnames_map(&add_or_update, &remove);
        
        // Verify
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("default/gateway1"));
        assert!(result.contains_key("default/gateway2"));
    }
    
    #[test]
    fn test_build_gateway_hostnames_map_with_same_hostname_different_gateways() {
        let mgr = RouteManager::new();
        
        // Create test routes with same hostname but different gateways
        let mut add_or_update = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        let route2 = create_test_httproute("default", "route2", vec!["api.example.com"], vec![("default", "gateway2")]);
        add_or_update.insert("default/route1".to_string(), route1);
        add_or_update.insert("default/route2".to_string(), route2);
        
        let remove = HashSet::new();
        
        // Build gateway_hostnames map
        let result = mgr.build_gateway_hostnames_map(&add_or_update, &remove);
        
        // Verify both gateways have the same hostname
        assert_eq!(result.len(), 2);
        let gw1_hostnames = result.get("default/gateway1").unwrap();
        let gw2_hostnames = result.get("default/gateway2").unwrap();
        assert!(gw1_hostnames.contains("api.example.com"));
        assert!(gw2_hostnames.contains("api.example.com"));
    }
    
    #[test]
    fn test_partial_update_add_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Create a gateway and add it to the gateway routes map
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);
        let _domain_routes = mgr.get_or_create_domain_routes("default", "gateway1");
        
        // Add gateway to store
        {
            let store = get_global_gateway_store();
            let mut store_guard = store.write().unwrap();
            let _ = store_guard.add_gateway(gateway);
        }
        
        // Create test routes to add
        let mut add = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        add.insert("default/route1".to_string(), route1);
        
        let remove = HashSet::new();
        
        // Execute partial_update
        mgr.partial_update(add, HashMap::new(), remove);
        
        // Verify the route was stored
        let http_routes = mgr.http_routes.lock().unwrap();
        assert!(http_routes.contains_key("default/route1"));
    }
    
    #[test]
    fn test_partial_update_remove_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Add a route first
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        mgr.http_routes.lock().unwrap().insert("default/route1".to_string(), route1);
        
        // Create remove set
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        
        // Execute partial_update
        mgr.partial_update(HashMap::new(), HashMap::new(), remove);
        
        // Verify the route was removed
        let http_routes = mgr.http_routes.lock().unwrap();
        assert!(!http_routes.contains_key("default/route1"));
    }
    
    #[test]
    fn test_full_set_stores_routes() {
        let mgr = RouteManager::new();
        
        // Create test data
        let mut data = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        let route2 = create_test_httproute("default", "route2", vec!["web.example.com"], vec![("default", "gateway1")]);
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify routes were stored
        let http_routes = mgr.http_routes.lock().unwrap();
        assert_eq!(http_routes.len(), 2);
        assert!(http_routes.contains_key("default/route1"));
        assert!(http_routes.contains_key("default/route2"));
    }
    
    #[test]
    fn test_full_set_replaces_existing_routes() {
        let mgr = RouteManager::new();
        
        // Setup: Add some existing routes
        {
            let mut http_routes = mgr.http_routes.lock().unwrap();
            let old_route = create_test_httproute("default", "old-route", vec!["old.example.com"], vec![("default", "gateway1")]);
            http_routes.insert("default/old-route".to_string(), old_route);
        }
        
        // Create new test data (without old route)
        let mut data = HashMap::new();
        let route1 = create_test_httproute("default", "route1", vec!["api.example.com"], vec![("default", "gateway1")]);
        data.insert("default/route1".to_string(), route1);
        
        // Execute full_set
        mgr.full_set(&data);
        
        // Verify old route was replaced
        let http_routes = mgr.http_routes.lock().unwrap();
        assert_eq!(http_routes.len(), 1);
        assert!(!http_routes.contains_key("default/old-route"));
        assert!(http_routes.contains_key("default/route1"));
    }
    
}
