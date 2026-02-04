use crate::core::conf_sync::traits::ConfHandler;
use crate::core::matcher::host_match::radix_match::{RadixHost, RadixHostMatchEngine};
use crate::core::routes::http_routes::lb_policy_sync::{cleanup_lb_policies_for_routes, sync_lb_policies_for_routes};
use crate::core::routes::http_routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::http_routes::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::routes::http_routes::routes_mgr::RouteRules;
use crate::core::routes::http_routes::{get_global_route_manager, HttpRouteRuleUnit, RouteManager};
use crate::types::{HTTPRoute, HTTPRouteMatch, HTTPRouteRule, MatchInfo, ResourceMeta};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;

type GatewayKey = String;
type DomainStr = String;
/// (normal_routes, regex_routes) tuple for a domain
type RouteRulesPair = (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>);
/// Domain -> (normal_routes, regex_routes) mapping
type DomainRouteRulesMap = HashMap<DomainStr, RouteRulesPair>;
/// Gateway -> Domain -> Routes mapping
type GatewayDomainRulesMap = HashMap<GatewayKey, DomainRouteRulesMap>;

/// Validated HTTPRoute data extracted from an HTTPRoute resource
struct ValidatedHttpRoute<'a> {
    parent_refs: &'a Vec<crate::types::resources::common::ParentReference>,
    rules: &'a Vec<crate::types::HTTPRouteRule>,
    hostnames: &'a Vec<String>,
    namespace: String,
    name: String,
}

/// Implement ConfHandler for Arc<RouteManager> to allow using the global instance
impl ConfHandler<HTTPRoute> for Arc<RouteManager> {
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        // Extract and update LB policies from all routes
        sync_lb_policies_for_routes(data);

        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, HTTPRoute>,
        update: HashMap<String, HTTPRoute>,
        remove: HashSet<String>,
    ) {
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
    #[allow(clippy::too_many_arguments)]
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
        let path_value = match_item
            .path
            .as_ref()
            .and_then(|p| p.value.as_deref())
            .ok_or_else(|| "Regex path must have value".to_string())?;

        let regex = Regex::new(path_value).map_err(|e| format!("Invalid regex '{}': {}", path_value, e))?;

        Ok(HttpRouteRuleUnit {
            resource_key: resource_key.to_string(),
            matched_info: MatchInfo::new(
                namespace.to_string(),
                name.to_string(),
                rule_id,
                match_id,
                match_item.clone(),
            ),
            rule,
            path_regex: Some(regex),
            parent_refs,
        })
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

                    let hostname_set = gateway_hostnames.entry(gateway_key).or_default();
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

                            let hostname_set = gateway_hostnames.entry(gateway_key).or_default();
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
            let applies_to_hostname = route
                .spec
                .hostnames
                .as_ref()
                .map(|hostnames| hostnames.contains(&hostname.to_string()))
                .unwrap_or(false);

            if !applies_to_hostname {
                continue;
            }

            // Check if this route applies to this gateway
            let applies_to_gateway = route
                .spec
                .parent_refs
                .as_ref()
                .map(|parent_refs| {
                    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                    parent_refs
                        .iter()
                        .any(|parent_ref| parent_ref.build_parent_key(Some(route_namespace)) == gateway_key)
                })
                .unwrap_or(false);

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
                                let rule_unit = HttpRouteRuleUnit {
                                    resource_key: resource_key.clone(),
                                    matched_info: MatchInfo::new(
                                        route_namespace.to_string(),
                                        route_name.to_string(),
                                        rule_id,
                                        match_id,
                                        match_item.clone(),
                                    ),
                                    rule: rule_arc.clone(),
                                    path_regex: None,
                                    parent_refs: route.spec.parent_refs.clone(),
                                };
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
        let regex_routes_engine =
            (!regex_routes_list.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));

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
                let applies = route
                    .spec
                    .hostnames
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
                                    let rule_unit = HttpRouteRuleUnit {
                                        resource_key: resource_key.clone(),
                                        matched_info: MatchInfo::new(
                                            route_namespace.to_string(),
                                            route_name.to_string(),
                                            rule_id,
                                            match_id,
                                            match_item.clone(),
                                        ),
                                        rule: rule_arc.clone(),
                                        path_regex: None,
                                        parent_refs: route.spec.parent_refs.clone(),
                                    };
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
            let regex_routes_engine =
                (!regex_routes_list.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));

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
}

/// Parse all HTTPRoutes and collect rules into gateway->domain->rules structure
/// Returns HashMap<GatewayKey, HashMap<DomainStr, (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>)>>
/// The tuple contains (normal_routes, regex_routes), both using Arc<HttpRouteRuleUnit>
fn parse_http_routes_to_gateway_domain_rules(data: &HashMap<String, HTTPRoute>) -> GatewayDomainRulesMap {
    let mut gateway_domain_rules: GatewayDomainRulesMap = HashMap::new();

    let mut processed_routes = 0;
    let mut skipped_routes = 0;

    // Iterate through all HTTPRoutes and collect rules
    for (_key, route) in data.iter() {
        // Validate HTTPRoute and extract required fields
        let validated = match validate_http_route(route) {
            Some(v) => v,
            None => {
                skipped_routes += 1;
                continue;
            }
        };

        // Process each parent gateway reference
        for parent_ref in validated.parent_refs {
            // Build gateway key
            let gateway_key = parent_ref.build_parent_key(Some(&validated.namespace));

            // Get or create the domain map for this gateway
            let domain_map = gateway_domain_rules.entry(gateway_key.clone()).or_default();

            // Process each hostname and rule combination
            for hostname in validated.hostnames {
                for (rule_id, rule) in validated.rules.iter().enumerate() {
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
                                    &validated.namespace,
                                    &validated.name,
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
                                let rule_unit = HttpRouteRuleUnit {
                                    resource_key: route.key_name(),
                                    matched_info: MatchInfo::new(
                                        validated.namespace.clone(),
                                        validated.name.clone(),
                                        rule_id,
                                        match_id,
                                        match_item.clone(),
                                    ),
                                    rule: rule_arc.clone(),
                                    path_regex: None,
                                    parent_refs: route.spec.parent_refs.clone(),
                                };
                                split.0.push(Arc::new(rule_unit));
                            }
                        }
                    } else {
                        tracing::warn!(route_name=%validated.name, route_namespace=%validated.namespace, "route missing match");
                    }
                }
            }

            processed_routes += 1;
        }
    }

    tracing::debug!(
        component = "route_manager",
        proc = processed_routes,
        skip = skipped_routes,
        gws = gateway_domain_rules.len(),
        "parsed"
    );

    gateway_domain_rules
}

/// Validate HTTPRoute and extract required fields
/// Returns Some(ValidatedHttpRoute) if valid, None otherwise
fn validate_http_route(route: &HTTPRoute) -> Option<ValidatedHttpRoute<'_>> {
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

    Some(ValidatedHttpRoute {
        parent_refs,
        rules,
        hostnames,
        namespace: route_namespace,
        name: route_name,
    })
}

impl ConfHandler<HTTPRoute> for RouteManager {
    /// Full set with a complete set of HTTPRoutes
    /// This is typically called during initial sync or re-list
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        let start_time = Instant::now();
        tracing::info!(component = "route_manager", cnt = data.len(), "full set start");

        // Step 0: Store all HTTPRoute resources for future lookups (e.g., during deletions)
        *self.http_routes.lock().unwrap() = data.clone();
        tracing::debug!(component = "route_manager", cnt = data.len(), "stored http_routes");

        // Step 1: Parse all HTTPRoutes into temporary gateway->domain->rules structure
        let gateway_domain_rules_new = parse_http_routes_to_gateway_domain_rules(data);

        // Step 1.5: Collect new gateway keys for cleanup later
        let new_gateway_keys: HashSet<String> = gateway_domain_rules_new.keys().cloned().collect();

        // Step 2: Build RouteRules with RadixRouteMatchEngine and update gateway_routes_map
        // Note: We no longer check GatewayStore - routes are built based on HTTPRoute's parentRef
        // and will be available when the Gateway arrives later
        let mut processed_gateways = 0;

        for (gateway_key, domain_rules_map) in gateway_domain_rules_new.into_iter() {
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
                let regex_routes_engine =
                    (!split.1.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));

                // Collect resource keys for this domain (from both normal and regex routes)
                let mut resource_keys: HashSet<String> = split.0.iter().map(|unit| unit.resource_key.clone()).collect();
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

            // Get or create DomainRouteRules for this gateway
            // This allows HTTPRoutes to be processed even before Gateway arrives
            let (namespace, name) = gateway_key.split_once('/').unwrap_or(("", gateway_key.as_str()));
            let domain_route_rules = self.get_or_create_domain_routes(namespace, name);

            // Build and store exact domain map
            domain_route_rules.exact_domain_map.store(Arc::new(exact_domain_map));

            // Build and store wildcard engine (only if wildcard domains exist)
            let wildcard_engine = if !wildcard_hosts.is_empty() {
                let mut engine = RadixHostMatchEngine::new();
                if let Err(e) = engine.initialize(wildcard_hosts) {
                    tracing::error!(component="route_manager",gw=%gateway_key,err=%e,"failed to build RadixHostMatchEngine");
                    continue;
                }
                Some(engine)
            } else {
                None
            };
            domain_route_rules.wildcard_engine.store(Arc::new(wildcard_engine));

            processed_gateways += 1;
        }

        // Step 3: Clean up stale gateway entries that no longer have any routes
        // This prevents memory leaks after relist when some gateways lose all their HTTPRoutes
        // First clear the routes data (for any existing Arc references), then remove from map
        let stale_keys: Vec<String> = self
            .gateway_routes_map
            .iter()
            .filter(|entry| !new_gateway_keys.contains(entry.key()))
            .map(|entry| entry.key().clone())
            .collect();

        for key in &stale_keys {
            // Clear routes first (for any existing Arc references)
            if let Some(entry) = self.gateway_routes_map.get(key) {
                entry.value().exact_domain_map.store(Arc::new(HashMap::new()));
                entry.value().wildcard_engine.store(Arc::new(None));
            }
            // Then remove from map
            self.gateway_routes_map.remove(key);
            tracing::debug!(component = "route_manager", gateway_key = %key, "Removed stale gateway entry");
        }

        if !stale_keys.is_empty() {
            tracing::info!(
                component = "route_manager",
                stale = stale_keys.len(),
                "cleaned up stale gateway entries"
            );
        }

        let elapsed = start_time.elapsed();
        tracing::info!(
            component = "route_manager",
            total = processed_gateways,
            ms = elapsed.as_millis(),
            "full set done"
        );
    }

    /// Handle partial configuration updates
    /// Processes additions, updates, and removals of HTTPRoutes
    /// Uses fine-grained updates for exact domains and batch updates for wildcard domains
    fn partial_update(
        &self,
        add: HashMap<String, HTTPRoute>,
        update: HashMap<String, HTTPRoute>,
        remove: HashSet<String>,
    ) {
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

            // Get or create DomainRouteRules - allows HTTPRoutes to be processed before Gateway arrives
            let (namespace, name) = gateway_key.split_once('/').unwrap_or(("", gateway_key.as_str()));
            let domain_routes_ref = self.get_or_create_domain_routes(namespace, name);

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

        // Step 3: Remove deleted routes from http_routes storage (after rebuilding)
        {
            let mut routes = self.http_routes.lock().unwrap();
            for key in remove.iter() {
                if routes.remove(key).is_some() {
                    tracing::debug!(component="route_manager",key=%key,"rm route");
                }
            }
        }

        tracing::info!(component = "route_manager", "HTTPRoute changes processed successfully");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::gateway::get_global_gateway_store;
    use crate::types::{Gateway, HTTPRoute};

    /// Helper function to create a test Gateway from JSON
    fn create_test_gateway(namespace: &str, name: &str, hostnames: Vec<&str>) -> Gateway {
        let listeners_json: Vec<serde_json::Value> = hostnames
            .iter()
            .map(|h| {
                serde_json::json!({
                    "name": format!("listener-{}", h),
                    "hostname": h,
                    "port": 80,
                    "protocol": "HTTP"
                })
            })
            .collect();

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
        let parent_refs_json: Vec<serde_json::Value> = gateway_refs
            .iter()
            .map(|(ns, n)| {
                serde_json::json!({
                    "group": "gateway.networking.k8s.io",
                    "kind": "Gateway",
                    "namespace": ns,
                    "name": n
                })
            })
            .collect();

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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        let route2 = create_test_httproute(
            "default",
            "route2",
            vec!["web.example.com"],
            vec![("default", "gateway1")],
        );
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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        mgr.http_routes
            .lock()
            .unwrap()
            .insert("default/route1".to_string(), route1);

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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        let route2 = create_test_httproute(
            "default",
            "route2",
            vec!["web.example.com"],
            vec![("default", "gateway2")],
        );
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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        let route2 = create_test_httproute(
            "default",
            "route2",
            vec!["api.example.com"],
            vec![("default", "gateway2")],
        );
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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        mgr.http_routes
            .lock()
            .unwrap()
            .insert("default/route1".to_string(), route1);

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
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
        let route2 = create_test_httproute(
            "default",
            "route2",
            vec!["web.example.com"],
            vec![("default", "gateway1")],
        );
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
            let old_route = create_test_httproute(
                "default",
                "old-route",
                vec!["old.example.com"],
                vec![("default", "gateway1")],
            );
            http_routes.insert("default/old-route".to_string(), old_route);
        }

        // Create new test data (without old route)
        let mut data = HashMap::new();
        let route1 = create_test_httproute(
            "default",
            "route1",
            vec!["api.example.com"],
            vec![("default", "gateway1")],
        );
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
