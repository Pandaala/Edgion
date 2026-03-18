use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::common::matcher::host_match::radix_match::{RadixHost, RadixHostMatchEngine};
use crate::core::gateway::routes::http::lb_policy_sync::{cleanup_lb_policies_for_routes, sync_lb_policies_for_routes};
use crate::core::gateway::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::gateway::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::gateway::routes::http::routes_mgr::{
    resolved_ports_for_route, DomainRouteRules, GlobalHttpRouteManagers, RouteRules,
};
use crate::core::gateway::routes::http::{get_global_http_route_managers, HttpRouteRuleUnit};
use crate::types::resources::common::ParentReference;
use crate::types::resources::http_route::RouteParentStatus;
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

/// Check if a specific parentRef has Accepted=True in the route status.
///
/// Returns true when no matching status entry is found (optimistic: include
/// until the controller explicitly rejects it, e.g. during the first sync
/// before status is populated).
fn is_parent_ref_accepted(pr: &ParentReference, status_parents: &[RouteParentStatus], route_ns: Option<&str>) -> bool {
    let pr_ns = pr.namespace.as_deref().or(route_ns).unwrap_or("default");

    for sp in status_parents {
        let sp_ns = sp.parent_ref.namespace.as_deref().or(route_ns).unwrap_or("default");
        if sp_ns == pr_ns
            && sp.parent_ref.name == pr.name
            && sp.parent_ref.section_name == pr.section_name
            && sp.parent_ref.port == pr.port
        {
            return sp
                .conditions
                .iter()
                .any(|c| c.type_ == "Accepted" && c.status == "True");
        }
    }
    true
}

/// Filter parentRefs to only those with Accepted=True status.
///
/// Routes whose parentRef is not accepted by any Gateway/Listener are excluded
/// from compilation, keeping the route table clean and simplifying debugging.
///
/// Returns None if no parentRefs survive (route should be skipped entirely).
/// When status is not yet available, all parentRefs are included optimistically.
pub(crate) fn filter_accepted_parent_refs(
    parent_refs: Option<&Vec<ParentReference>>,
    status_parents: Option<&[RouteParentStatus]>,
    route_ns: Option<&str>,
) -> Option<Vec<ParentReference>> {
    let parent_refs = parent_refs?;
    if parent_refs.is_empty() {
        return None;
    }

    let status_parents = match status_parents {
        Some(sp) if !sp.is_empty() => sp,
        _ => return Some(parent_refs.clone()),
    };

    let accepted: Vec<ParentReference> = parent_refs
        .iter()
        .filter(|pr| is_parent_ref_accepted(pr, status_parents, route_ns))
        .cloned()
        .collect();

    if accepted.is_empty() {
        None
    } else {
        Some(accepted)
    }
}

/// Return the effective hostnames for a route.
///
/// Priority:
/// 1. `spec.resolved_hostnames` — pre-computed by the controller (intersection with listener)
/// 2. `spec.hostnames` — route's own hostname list (fallback for legacy/bootstrap)
/// 3. `CATCH_ALL_HOSTNAME` — wildcard when no hostnames specified
fn get_effective_hostnames(route: &HTTPRoute) -> Vec<String> {
    if let Some(resolved) = &route.spec.resolved_hostnames {
        if !resolved.is_empty() {
            return resolved.clone();
        }
    }
    if let Some(hostnames) = &route.spec.hostnames {
        if !hostnames.is_empty() {
            return hostnames.clone();
        }
    }
    vec![CATCH_ALL_HOSTNAME.to_string()]
}

/// Implement ConfHandler for &'static GlobalHttpRouteManagers
impl ConfHandler<HTTPRoute> for &'static GlobalHttpRouteManagers {
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        (**self).full_set(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, HTTPRoute>,
        update: HashMap<String, HTTPRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove);
    }
}

impl ConfHandler<HTTPRoute> for GlobalHttpRouteManagers {
    fn full_set(&self, data: &HashMap<String, HTTPRoute>) {
        sync_lb_policies_for_routes(data);

        let start_time = Instant::now();
        tracing::info!(component = "http_route_manager", cnt = data.len(), "full set start");

        // Step 1: Replace route cache
        self.route_cache.clear();
        for (key, route) in data {
            self.route_cache.insert(key.clone(), route.clone());
        }

        // Step 2: Rebuild all per-port managers
        self.rebuild_all_port_managers();

        let elapsed = start_time.elapsed();
        tracing::info!(
            component = "http_route_manager",
            ms = elapsed.as_millis(),
            "full set done"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, HTTPRoute>,
        update: HashMap<String, HTTPRoute>,
        remove: HashSet<String>,
    ) {
        let mut add_or_update = add;
        add_or_update.extend(update);

        sync_lb_policies_for_routes(&add_or_update);
        cleanup_lb_policies_for_routes(&remove);

        tracing::info!(
            component = "http_route_manager",
            add = add_or_update.len(),
            rm = remove.len(),
            "partial update start"
        );

        // Compute affected ports BEFORE updating cache.
        // When a route lacks resolved_ports, extract from parentRef.port
        // or mark all existing ports as affected.
        let mut affected_ports = HashSet::new();
        let mut needs_all_ports = false;

        for (key, route) in &add_or_update {
            let ports = resolved_ports_for_route(route);
            if ports.is_empty() {
                if let Some(parent_refs) = &route.spec.parent_refs {
                    let any_port = parent_refs.iter().any(|pr| pr.port.is_some());
                    if any_port {
                        for pr in parent_refs {
                            if let Some(p) = pr.port {
                                affected_ports.insert(p as u16);
                            }
                        }
                    } else {
                        needs_all_ports = true;
                    }
                } else {
                    needs_all_ports = true;
                }
            } else {
                for &port in ports {
                    affected_ports.insert(port);
                }
            }
            if let Some(old) = self.route_cache.get(key) {
                for &port in resolved_ports_for_route(old.value()) {
                    affected_ports.insert(port);
                }
            }
        }
        for key in &remove {
            if let Some(old) = self.route_cache.get(key) {
                let ports = resolved_ports_for_route(old.value());
                if ports.is_empty() {
                    needs_all_ports = true;
                } else {
                    for &port in ports {
                        affected_ports.insert(port);
                    }
                }
            }
        }
        if needs_all_ports {
            for entry in self.by_port.iter() {
                affected_ports.insert(*entry.key());
            }
        }

        // Update route cache
        for (key, route) in add_or_update {
            self.route_cache.insert(key, route);
        }
        for key in &remove {
            self.route_cache.remove(key);
        }

        // Rebuild affected port managers
        self.rebuild_affected_port_managers(&affected_ports);

        tracing::info!(component = "http_route_manager", "partial update done");
    }
}

/// Create a handler for registration with ConfigClient
pub fn create_route_manager_handler() -> Box<dyn ConfHandler<HTTPRoute> + Send + Sync> {
    Box::new(get_global_http_route_managers())
}

// ============================================================================
// Route parsing and DomainRouteRules building
// ============================================================================

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
    let (compiled_header_regexes, compiled_query_regexes) = HttpRouteRuleUnit::compile_match_regexes(match_item);

    Ok(HttpRouteRuleUnit {
        resource_key: resource_key.to_string(),
        matched_info: MatchInfo::new(
            namespace.to_string(),
            name.to_string(),
            rule_id,
            match_id,
            match_item.clone(),
            0,
        ),
        rule,
        path_regex: Some(regex),
        parent_refs,
        compiled_header_regexes,
        compiled_query_regexes,
    })
}

/// Parse all HTTPRoutes and collect rules into a domain->rules structure.
///
/// Routes are merged into a single table keyed by hostname.
/// Each route unit carries its parentRefs so gateway validation happens at match time.
fn parse_http_routes_to_domain_rules(data: &HashMap<String, HTTPRoute>) -> DomainRouteRulesMap {
    let mut domain_rules: DomainRouteRulesMap = HashMap::new();

    let mut processed_routes = 0;
    let mut skipped_routes = 0;

    for (_key, route) in data.iter() {
        let validated = match validate_http_route(route) {
            Some(v) => v,
            None => {
                skipped_routes += 1;
                continue;
            }
        };

        let accepted_refs = match filter_accepted_parent_refs(
            route.spec.parent_refs.as_ref(),
            route.status.as_ref().map(|s| s.parents.as_slice()),
            Some(&validated.namespace),
        ) {
            Some(refs) => refs,
            None => {
                skipped_routes += 1;
                continue;
            }
        };

        let accepted_refs_opt = Some(accepted_refs.clone());
        let route_sv = route.get_sync_version();

        let effective_hostnames = get_effective_hostnames(route);
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

                    if is_regex_path(match_item) {
                        match create_regex_route_unit(
                            &validated.namespace,
                            &validated.name,
                            rule_id,
                            match_id,
                            &route.key_name(),
                            match_item,
                            rule_arc.clone(),
                            accepted_refs_opt.clone(),
                        ) {
                            Ok(mut regex_unit) => {
                                regex_unit.matched_info.sv = route_sv;
                                split.1.push(Arc::new(regex_unit));
                            }
                            Err(e) => {
                                tracing::warn!(route=%route.key_name(),err=%e,"failed to create regex route");
                            }
                        }
                    } else {
                        let (chr, cqr) = HttpRouteRuleUnit::compile_match_regexes(match_item);
                        let rule_unit = HttpRouteRuleUnit {
                            resource_key: route.key_name(),
                            matched_info: MatchInfo::new(
                                validated.namespace.clone(),
                                validated.name.clone(),
                                rule_id,
                                match_id,
                                match_item.clone(),
                                route_sv,
                            ),
                            rule: rule_arc.clone(),
                            path_regex: None,
                            parent_refs: accepted_refs_opt.clone(),
                            compiled_header_regexes: chr,
                            compiled_query_regexes: cqr,
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
    match &route.spec.parent_refs {
        Some(refs) if !refs.is_empty() => refs,
        _ => {
            tracing::warn!(route=%route.key_name(),"no parent_refs");
            return None;
        }
    };

    let rules = match &route.spec.rules {
        Some(rules) if !rules.is_empty() => rules,
        _ => {
            tracing::warn!(route=%route.key_name(),"no rules");
            return None;
        }
    };

    let route_namespace = match &route.metadata.namespace {
        Some(ns) if !ns.is_empty() => ns.clone(),
        _ => {
            tracing::warn!(route=%route.key_name(),"no namespace");
            return None;
        }
    };

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

/// Build DomainRouteRules from a set of routes (used by per-port rebuild).
pub(crate) fn build_domain_route_rules_from_routes(data: &HashMap<String, HTTPRoute>) -> DomainRouteRules {
    let domain_rules_map = parse_http_routes_to_domain_rules(data);

    let mut exact_domain_map: HashMap<DomainStr, Arc<RouteRules>> = HashMap::new();
    let mut wildcard_hosts: Vec<RadixHost<RouteRules>> = Vec::new();
    let mut catch_all_routes: Option<Arc<RouteRules>> = None;

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

        let regex_routes_engine = (!split.1.is_empty()).then(|| Arc::new(RegexRoutesEngine::build(split.1.clone())));

        let mut resource_keys: HashSet<String> = split.0.iter().map(|u| u.resource_key.clone()).collect();
        resource_keys.extend(split.1.iter().map(|u| u.resource_key.clone()));

        let route_rules = Arc::new(RouteRules {
            resource_keys: RwLock::new(resource_keys),
            route_rules_list: RwLock::new(split.0),
            match_engine,
            regex_routes: RwLock::new(split.1),
            regex_routes_engine,
        });

        if domain == CATCH_ALL_HOSTNAME {
            catch_all_routes = Some(route_rules);
        } else if domain.starts_with("*.") {
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

    DomainRouteRules {
        exact_domain_map: arc_swap::ArcSwap::from_pointee(exact_domain_map),
        wildcard_engine: arc_swap::ArcSwap::from_pointee(wildcard_engine),
        catch_all_routes: arc_swap::ArcSwap::from_pointee(catch_all_routes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HTTPRoute;

    fn create_test_httproute(
        namespace: &str,
        name: &str,
        hostnames: Vec<&str>,
        gateway_refs: Vec<(&str, &str)>,
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
    fn test_get_effective_hostnames_prefers_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["original.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec!["resolved.example.com".to_string()]);

        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["resolved.example.com"]);
    }

    #[test]
    fn test_get_effective_hostnames_falls_back_to_spec_hostnames() {
        let route = create_test_httproute("default", "route1", vec!["spec.example.com"], vec![("default", "gw1")]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["spec.example.com"]);
    }

    #[test]
    fn test_get_effective_hostnames_catch_all_when_empty() {
        let route = create_test_httproute("default", "route1", vec![], vec![("default", "gw1")]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["*"]);
    }

    #[test]
    fn test_get_effective_hostnames_skips_empty_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["fallback.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec![]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["fallback.example.com"]);
    }

    #[test]
    fn test_get_effective_hostnames_multiple_resolved() {
        let mut route = create_test_httproute(
            "default",
            "route1",
            vec!["ignored.example.com"],
            vec![("default", "gw1")],
        );
        route.spec.resolved_hostnames = Some(vec!["a.example.com".to_string(), "b.example.com".to_string()]);
        let result = get_effective_hostnames(&route);
        assert_eq!(result, vec!["a.example.com", "b.example.com"]);
    }
}
