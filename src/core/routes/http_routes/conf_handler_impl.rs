use crate::core::conf_sync::traits::ConfHandler;
use crate::core::gateway::gateway::get_global_gateway_store;
use crate::core::matcher::host_match::radix_match::{RadixHost, RadixHostMatchEngine};
use crate::core::routes::http_routes::lb_policy_sync::{cleanup_lb_policies_for_routes, sync_lb_policies_for_routes};
use crate::core::routes::http_routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::http_routes::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::routes::http_routes::routes_mgr::RouteRules;
use crate::core::routes::http_routes::{get_global_route_manager, HttpRouteRuleUnit, RouteManager};
use crate::types::resources::common::ParentReference;
use crate::types::{HTTPPathMatch, HTTPRoute, HTTPRouteMatch, HTTPRouteRule, MatchInfo, ResourceMeta};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;

type DomainStr = String;
/// (normal_routes, regex_routes) tuple for a domain
type RouteRulesPair = (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>);
/// Domain -> (normal_routes, regex_routes) mapping
type DomainRouteRulesMap = HashMap<DomainStr, RouteRulesPair>;

/// Validated HTTPRoute data extracted from an HTTPRoute resource
struct ValidatedHttpRoute<'a> {
    rules: &'a Vec<crate::types::HTTPRouteRule>,
    namespace: String,
    name: String,
}

/// Sentinel hostname used when an HTTPRoute has no spec.hostnames.
/// Per Gateway API spec, omitting hostnames means "match all hosts".
const CATCH_ALL_HOSTNAME: &str = "*";

/// Resolve the effective hostnames for a route attached to a specific gateway via a given parent_ref.
///
/// Per Gateway API spec, if an HTTPRoute has no `spec.hostnames`, it inherits the hostname
/// from the listener it is attached to.
fn resolve_effective_hostnames_for_route(
    route: &HTTPRoute,
    gateway_key: &str,
    parent_ref: &ParentReference,
) -> Vec<String> {
    if let Some(hostnames) = &route.spec.hostnames {
        if !hostnames.is_empty() {
            return hostnames.clone();
        }
    }

    let store_arc = get_global_gateway_store();
    if let Ok(store) = store_arc.read() {
        if let Ok(gw) = store.get_gateway(gateway_key) {
            if let Some(listeners) = &gw.spec.listeners {
                let listener = match parent_ref.section_name.as_deref() {
                    Some(section_name) => listeners.iter().find(|l| l.name == section_name),
                    None => listeners.first(),
                };
                if let Some(listener) = listener {
                    if let Some(hostname) = &listener.hostname {
                        if !hostname.is_empty() {
                            return vec![hostname.clone()];
                        }
                    }
                }
            }
        }
    }

    vec![CATCH_ALL_HOSTNAME.to_string()]
}

/// Resolve ALL effective hostnames for a route across all its parentRefs.
fn resolve_all_effective_hostnames(route: &HTTPRoute, route_namespace: &str) -> Vec<String> {
    if let Some(hostnames) = &route.spec.hostnames {
        if !hostnames.is_empty() {
            return hostnames.clone();
        }
    }

    let mut all_hostnames: Vec<String> = Vec::new();
    if let Some(parent_refs) = &route.spec.parent_refs {
        for pr in parent_refs {
            let gw_key = pr.build_parent_key(Some(route_namespace));
            let resolved = resolve_effective_hostnames_for_route(route, &gw_key, pr);
            for h in resolved {
                if !all_hostnames.contains(&h) {
                    all_hostnames.push(h);
                }
            }
        }
    }

    if all_hostnames.is_empty() {
        vec![CATCH_ALL_HOSTNAME.to_string()]
    } else {
        all_hostnames
    }
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
    /// Collect the set of hostnames affected by the given add/update/remove changes.
    /// Includes both old and new hostnames for updates to ensure stale entries are cleaned.
    fn build_affected_hostnames(
        &self,
        add_or_update: &HashMap<String, HTTPRoute>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let mut affected: HashSet<String> = HashSet::new();

        let http_routes = self.http_routes.lock().unwrap();

        for (resource_key, route) in add_or_update.iter() {
            let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
            for h in resolve_all_effective_hostnames(route, route_ns) {
                affected.insert(h);
            }
            // For updates, also include old hostnames
            let old_route = http_routes.get(resource_key);
            if let Some(old_route) = old_route {
                let old_ns = old_route.metadata.namespace.as_deref().unwrap_or("default");
                for h in resolve_all_effective_hostnames(old_route, old_ns) {
                    affected.insert(h);
                }
            }
        }

        for resource_key in remove.iter() {
            if let Some(route) = http_routes.get(resource_key) {
                let ns = route.metadata.namespace.as_deref().unwrap_or("default");
                for h in resolve_all_effective_hostnames(route, ns) {
                    affected.insert(h);
                }
            }
        }

        affected
    }

    /// Rebuild RouteRules for a single exact hostname from ALL stored routes.
    /// Returns new RouteRules or None if no routes remain for this hostname.
    fn rebuild_exact_hostname(&self, hostname: &str, remove: &HashSet<String>) -> Option<Arc<RouteRules>> {
        let http_routes = self.http_routes.lock().unwrap();

        let mut route_rules_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        let mut regex_routes_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
        let mut resource_keys: HashSet<String> = HashSet::new();

        for (resource_key, route) in http_routes.iter() {
            if remove.contains(resource_key) {
                continue;
            }

            // Check if this route applies to this hostname (explicit or listener-inherited)
            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            let applies_to_hostname =
                resolve_all_effective_hostnames(route, route_namespace).contains(&hostname.to_string());

            if !applies_to_hostname {
                continue;
            }

            let route_name = route.metadata.name.as_deref().unwrap_or("");

            if let Some(rules) = &route.spec.rules {
                for (rule_id, rule) in rules.iter().enumerate() {
                    let rule_arc = Arc::new(rule.clone());

                    // Default match if none provided (prefix "/")
                    let default_matches = vec![HTTPRouteMatch {
                        path: Some(HTTPPathMatch {
                            match_type: Some("PathPrefix".to_string()),
                            value: Some("/".to_string()),
                        }),
                        headers: None,
                        query_params: None,
                        method: None,
                    }];

                    let matches = match &rule.matches {
                        Some(m) if !m.is_empty() => m,
                        _ => &default_matches,
                    };

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

            resource_keys.insert(resource_key.clone());
        }

        if route_rules_list.is_empty() && regex_routes_list.is_empty() {
            return None;
        }

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

        let regex_routes_engine =
            (!regex_routes_list.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));

        Some(Arc::new(RouteRules {
            resource_keys: RwLock::new(resource_keys),
            route_rules_list: RwLock::new(route_rules_list),
            match_engine,
            regex_routes: RwLock::new(regex_routes_list),
            regex_routes_engine,
        }))
    }

    /// Rebuild the wildcard engine from all stored routes.
    /// Reuses Arc<RouteRules> from the existing engine for unchanged wildcard hostnames.
    fn rebuild_wildcard_engine(
        &self,
        affected_hostnames: &HashSet<String>,
        remove: &HashSet<String>,
        current_engine: Option<&RadixHostMatchEngine<RouteRules>>,
    ) -> Result<Option<RadixHostMatchEngine<RouteRules>>, String> {
        let http_routes = self.http_routes.lock().unwrap();

        let mut existing_hosts_map: HashMap<String, RadixHost<RouteRules>> = HashMap::new();
        if let Some(engine) = current_engine {
            for host in engine.export_hosts() {
                existing_hosts_map.insert(host.original.to_lowercase(), host);
            }
        }

        // Collect ALL wildcard hostnames across all routes
        let mut wildcard_hostnames: HashSet<String> = HashSet::new();
        for (resource_key, route) in http_routes.iter() {
            if remove.contains(resource_key) {
                continue;
            }

            let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
            for h in resolve_all_effective_hostnames(route, route_ns) {
                if h.starts_with("*.") {
                    wildcard_hostnames.insert(h);
                }
            }
        }

        let mut radix_hosts: Vec<RadixHost<RouteRules>> = Vec::new();

        for hostname in wildcard_hostnames.iter() {
            // Reuse unchanged wildcard hostnames
            if !affected_hostnames.contains(hostname) {
                if let Some(existing_host) = existing_hosts_map.get(&hostname.to_lowercase()) {
                    radix_hosts.push(existing_host.clone());
                    continue;
                }
            }

            // Rebuild affected hostname
            let mut route_rules_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
            let mut regex_routes_list: Vec<Arc<HttpRouteRuleUnit>> = Vec::new();
            let mut resource_keys: HashSet<String> = HashSet::new();

            for (resource_key, route) in http_routes.iter() {
                if remove.contains(resource_key) {
                    continue;
                }

                // Check if this route applies to this hostname (explicit or inherited)
                let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                let applies = resolve_all_effective_hostnames(route, route_namespace).contains(&hostname.to_string());

                if !applies {
                    continue;
                }

                let route_name = route.metadata.name.as_deref().unwrap_or("");

                if let Some(rules) = &route.spec.rules {
                    for (rule_id, rule) in rules.iter().enumerate() {
                        let rule_arc = Arc::new(rule.clone());

                        // Default match if none provided (prefix "/")
                        let default_matches = vec![HTTPRouteMatch {
                            path: Some(HTTPPathMatch {
                                match_type: Some("PathPrefix".to_string()),
                                value: Some("/".to_string()),
                            }),
                            headers: None,
                            query_params: None,
                            method: None,
                        }];

                        let matches = match &rule.matches {
                            Some(m) if !m.is_empty() => m,
                            _ => &default_matches,
                        };

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

                resource_keys.insert(resource_key.clone());
            }

            // Skip if no routes for this hostname
            if route_rules_list.is_empty() && regex_routes_list.is_empty() {
                continue;
            }

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

            let regex_routes_engine =
                (!regex_routes_list.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(regex_routes_list.clone())));

            let route_rules = Arc::new(RouteRules {
                resource_keys: RwLock::new(resource_keys),
                route_rules_list: RwLock::new(route_rules_list),
                match_engine,
                regex_routes: RwLock::new(regex_routes_list),
                regex_routes_engine,
            });

            radix_hosts.push(RadixHost::new(hostname, route_rules));
        }

        if radix_hosts.is_empty() {
            Ok(None)
        } else {
            let mut new_engine = RadixHostMatchEngine::new();
            new_engine.initialize(radix_hosts)?;
            Ok(Some(new_engine))
        }
    }
}

/// Parse all HTTPRoutes and collect rules into a global domain->rules structure.
///
/// Routes from all gateways are merged into a single table keyed by hostname.
/// Each route unit carries its parentRefs so gateway validation happens at match time.
fn parse_http_routes_to_domain_rules(data: &HashMap<String, HTTPRoute>) -> DomainRouteRulesMap {
    let mut domain_rules: DomainRouteRulesMap = HashMap::new();

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

        // Resolve effective hostnames across ALL parent_refs.
        // A route with explicit hostnames uses those directly.
        // A route with no hostnames inherits from each listener it is attached to.
        let effective_hostnames = resolve_all_effective_hostnames(route, &validated.namespace);
        for hostname in &effective_hostnames {
            for (rule_id, rule) in validated.rules.iter().enumerate() {
                let rule_arc = Arc::new(rule.clone());

                let default_matches = vec![HTTPRouteMatch {
                    path: Some(HTTPPathMatch {
                        match_type: Some("PathPrefix".to_string()),
                        value: Some("/".to_string()),
                    }),
                    headers: None,
                    query_params: None,
                    method: None,
                }];

                let matches = match &rule.matches {
                    Some(m) if !m.is_empty() => m,
                    _ => &default_matches,
                };

                for (match_id, match_item) in matches.iter().enumerate() {
                    let split = domain_rules
                        .entry(hostname.clone())
                        .or_insert_with(|| (Vec::new(), Vec::new()));

                    if RouteManager::is_regex_path(match_item) {
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
            }
        }
        processed_routes += 1;
    }

    tracing::debug!(
        component = "route_manager",
        proc = processed_routes,
        skip = skipped_routes,
        domains = domain_rules.len(),
        "parsed"
    );

    domain_rules
}

/// Validate HTTPRoute and extract required fields
/// Returns Some(ValidatedHttpRoute) if valid, None otherwise
fn validate_http_route(route: &HTTPRoute) -> Option<ValidatedHttpRoute<'_>> {
    // Check parent_refs
    match &route.spec.parent_refs {
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
        rules,
        namespace: route_namespace,
        name: route_name,
    })
}

impl RouteManager {
    /// Rebuild routing tables from currently stored HTTPRoutes.
    ///
    /// Called when gateway listener hostnames change so hostname inheritance
    /// for routes without explicit `spec.hostnames` is refreshed.
    pub fn rebuild_from_stored_routes(&self) {
        let routes = self.http_routes.lock().unwrap().clone();
        if routes.is_empty() {
            return;
        }
        tracing::info!(
            component = "route_manager",
            cnt = routes.len(),
            "rebuilding routes from stored data"
        );
        self.full_set(&routes);
    }
}

impl ConfHandler<HTTPRoute> for RouteManager {
    /// Full set with a complete set of HTTPRoutes.
    /// Builds a single global route table shared by all gateways/listeners.
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        let start_time = Instant::now();
        tracing::info!(component = "route_manager", cnt = data.len(), "full set start");

        // Step 0: Store all HTTPRoute resources
        *self.http_routes.lock().unwrap() = data.clone();
        // Step 1: Parse all HTTPRoutes into global domain->rules structure
        let domain_rules_map = parse_http_routes_to_domain_rules(data);
        // Step 2: Build the single global DomainRouteRules
        let mut exact_domain_map: HashMap<DomainStr, Arc<RouteRules>> = HashMap::new();
        let mut wildcard_hosts: Vec<RadixHost<RouteRules>> = Vec::new();
        for (domain, split) in domain_rules_map.into_iter() {
            if split.0.is_empty() && split.1.is_empty() {
                continue;
            }

            let match_engine = if split.0.is_empty() {
                None
            } else {
                match RadixRouteMatchEngine::build(split.0.clone()) {
                    Ok(engine) => Some(Arc::new(engine)),
                    Err(e) => {
                        tracing::error!(component="route_manager",domain=%domain,err=?e,"build failed");
                        continue;
                    }
                }
            };

            let regex_routes_engine =
                (!split.1.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));

            let mut resource_keys: HashSet<String> = split.0.iter().map(|u| u.resource_key.clone()).collect();
            resource_keys.extend(split.1.iter().map(|u| u.resource_key.clone()));

            let route_rules = Arc::new(RouteRules {
                resource_keys: RwLock::new(resource_keys),
                route_rules_list: RwLock::new(split.0),
                match_engine,
                regex_routes: RwLock::new(split.1),
                regex_routes_engine,
            });

            if domain.starts_with("*.") {
                wildcard_hosts.push(RadixHost::new(&domain, route_rules));
            } else {
                exact_domain_map.insert(domain.to_lowercase(), route_rules);
            }
        }

        let wildcard_engine = if !wildcard_hosts.is_empty() {
            let mut engine = RadixHostMatchEngine::new();
            if let Err(e) = engine.initialize(wildcard_hosts) {
                tracing::error!(component="route_manager",err=%e,"failed to build RadixHostMatchEngine");
                None
            } else {
                Some(engine)
            }
        } else {
            None
        };

        let elapsed = start_time.elapsed();
        let global = self.global_routes.load();
        global.exact_domain_map.store(Arc::new(exact_domain_map));
        global.wildcard_engine.store(Arc::new(wildcard_engine));
        tracing::info!(component = "route_manager", ms = elapsed.as_millis(), "full set done");
    }

    /// Handle partial configuration updates.
    /// Processes additions, updates, and removals of HTTPRoutes against the global route table.
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

        let mut add_or_update = add;
        add_or_update.extend(update);

        // Step 0: Build affected hostnames BEFORE updating storage
        let affected_hostnames = self.build_affected_hostnames(&add_or_update, &remove);

        // Step 1: Update http_routes storage
        {
            let mut routes = self.http_routes.lock().unwrap();
            for (key, route) in add_or_update.iter() {
                routes.insert(key.clone(), route.clone());
            }
        }

        // Step 2: Separate exact and wildcard hostnames
        let mut exact_hostnames: HashSet<String> = HashSet::new();
        let mut wildcard_hostnames: HashSet<String> = HashSet::new();
        for hostname in &affected_hostnames {
            if hostname.starts_with("*.") {
                wildcard_hostnames.insert(hostname.clone());
            } else {
                exact_hostnames.insert(hostname.clone());
            }
        }

        // Step 3: Load current global routes snapshot
        let current_global = self.global_routes.load();

        // Update exact domains (RCU pattern)
        if !exact_hostnames.is_empty() {
            let current_map = current_global.exact_domain_map.load();
            let mut new_map = (**current_map).clone();

            for hostname in exact_hostnames.iter() {
                match self.rebuild_exact_hostname(hostname, &remove) {
                    Some(new_route_rules) => {
                        new_map.insert(hostname.to_lowercase(), new_route_rules);
                    }
                    None => {
                        new_map.remove(&hostname.to_lowercase());
                    }
                }
            }

            current_global.exact_domain_map.store(Arc::new(new_map));
            tracing::info!(
                component = "route_manager",
                cnt = exact_hostnames.len(),
                "exact domains updated"
            );
        }

        // Update wildcard domains (rebuild engine with Arc reuse)
        if !wildcard_hostnames.is_empty() {
            let current_engine_opt = current_global.wildcard_engine.load();
            let current_engine = current_engine_opt.as_ref().as_ref();

            match self.rebuild_wildcard_engine(&wildcard_hostnames, &remove, current_engine) {
                Ok(new_engine_opt) => {
                    current_global.wildcard_engine.store(Arc::new(new_engine_opt));
                    tracing::info!(
                        component = "route_manager",
                        cnt = wildcard_hostnames.len(),
                        "wildcard engine rebuilt"
                    );
                }
                Err(e) => {
                    tracing::error!(component="route_manager",err=%e,"failed to rebuild wildcard engine");
                }
            }
        }

        // Step 4: Remove deleted routes from storage (after rebuilding)
        {
            let mut routes = self.http_routes.lock().unwrap();
            for key in remove.iter() {
                routes.remove(key);
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
    fn test_build_affected_hostnames_with_add_routes() {
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

        let result = mgr.build_affected_hostnames(&add_or_update, &remove);
        assert_eq!(result.len(), 2);
        assert!(result.contains("api.example.com"));
        assert!(result.contains("web.example.com"));
    }

    #[test]
    fn test_build_affected_hostnames_with_remove_routes() {
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

        let result = mgr.build_affected_hostnames(&add_or_update, &remove);
        assert_eq!(result.len(), 1);
        assert!(result.contains("api.example.com"));
    }

    #[test]
    fn test_build_affected_hostnames_with_multiple_gateways() {
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

        let result = mgr.build_affected_hostnames(&add_or_update, &remove);
        assert_eq!(result.len(), 2);
        assert!(result.contains("api.example.com"));
        assert!(result.contains("web.example.com"));
    }

    #[test]
    fn test_build_affected_hostnames_deduplicates() {
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

        let result = mgr.build_affected_hostnames(&add_or_update, &remove);
        assert_eq!(result.len(), 1);
        assert!(result.contains("api.example.com"));
    }

    #[test]
    fn test_partial_update_add_routes() {
        let mgr = RouteManager::new();

        // Setup: Create a gateway and add it to the gateway store
        let gateway = create_test_gateway("default", "gateway1", vec!["api.example.com"]);

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
