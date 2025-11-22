use std::collections::HashMap;
use std::sync::Arc;
use arc_swap::ArcSwap;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::routes::HttpRouteRuleUnit;
use crate::core::routes::r#match::radix_route_match::RadixRouteMatchEngine;
use crate::types::{HTTPRoute, ResourceMeta};

type DomainStr = String;

#[derive(Clone)]
pub struct RouteRules {
    route_rules_list: Vec<HttpRouteRuleUnit>,
    match_engine: Arc<RadixRouteMatchEngine>,
    need_rebuild: bool,
}

#[derive(Clone)]
pub struct DomainRouteRules {
    domain_routes_map: HashMap<DomainStr, Arc<RouteRules>>,
}

pub struct RouteManager {
    gateway_routes_map: HashMap<String, Arc<DomainRouteRules>>,
    gateway_routes_matcher: ArcSwap<HashMap<String, HashMap<String, ArcSwap<RadixRouteMatchEngine>>>>,
}

impl RouteManager {
    pub fn new() -> Self {
        Self {
            gateway_routes_map: HashMap::new(),
            gateway_routes_matcher: ArcSwap::new(Arc::new(HashMap::new())),
        }
    }

    pub fn add_http_routes(&mut self, http_routes: Vec<HTTPRoute>) {
        for route in http_routes {
            let changed_route_rules = self.add_http_route_single(route);
            // Traverse changed route rules and output need_rebuild
            for route_rules in changed_route_rules {
                if route_rules.need_rebuild {
                    tracing::info!(
                        "RouteRules needs rebuild: need_rebuild={}, route_count={}",
                        route_rules.need_rebuild,
                        route_rules.route_rules_list.len()
                    );
                }
            }
        }
    }

    pub fn add_http_route(&mut self, route: HTTPRoute) {
        let changed_route_rules = self.add_http_route_single(route);
        // Traverse changed route rules and output need_rebuild
        for route_rules in changed_route_rules {
            if route_rules.need_rebuild {
                tracing::info!(
                    "RouteRules needs rebuild: need_rebuild={}, route_count={}",
                    route_rules.need_rebuild,
                    route_rules.route_rules_list.len()
                );
            }
        }
    }

    fn add_http_route_single(&mut self, route: HTTPRoute) -> Vec<Arc<RouteRules>> {
        let mut changed_route_rules = Vec::new();
        let parent_refs = match &route.spec.parent_refs {
            Some(refs) => refs,
            None => {
                tracing::warn!("HTTPRoute '{}' has no parent_refs, skipping", route.key_name());
                return changed_route_rules;
            }
        };

        let gateway_store = get_global_gateway_store();
        let gateway_store_guard = gateway_store.read().unwrap();

        for parent_ref in parent_refs {
            let gateway_key = Self::build_gateway_key(parent_ref, &route);
            
            // Check if gateway exists in global store
            match gateway_store_guard.get_gateway(&gateway_key) {
                Ok(_gateway) => {
                    let domain_routes_map = self.get_or_create_domain_routes_map(&gateway_key);
                    let route_changes = Self::add_rules_to_domain_map(domain_routes_map, &route);
                    changed_route_rules.extend(route_changes);
                    
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
        
        changed_route_rules
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
    fn get_or_create_domain_routes_map(&mut self, gateway_key: &str) -> &mut DomainRouteRules {
        let domain_routes_map_arc = self
            .gateway_routes_map
            .entry(gateway_key.to_string())
            .or_insert_with(|| Arc::new(DomainRouteRules {
                domain_routes_map: HashMap::new(),
            }));

        // Clone Arc to get mutable access (Arc::make_mut ensures we have unique ownership)
        Arc::make_mut(domain_routes_map_arc)
    }

    /// Add all rules from HTTPRoute to the domain routes map
    fn add_rules_to_domain_map(domain_routes_map: &mut DomainRouteRules, route: &HTTPRoute) -> Vec<Arc<RouteRules>> {
        let mut changed_route_rules = Vec::new();
        
        if let Some(rules) = &route.spec.rules {
            let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
            let route_name = route.metadata.name.as_deref().unwrap_or("").to_string();
            
            for rule in rules {
                if let Some(hostnames) = &route.spec.hostnames {
                    for hostname in hostnames {
                        let route_rules_arc = Self::add_rule_to_route_rules(
                            domain_routes_map,
                            &hostname,
                            &route_namespace,
                            &route_name,
                            rule,
                        );
                        changed_route_rules.push(route_rules_arc);
                    }
                } else {
                    // If no hostnames specified, use "*" as default
                    let route_rules_arc = Self::add_rule_to_route_rules(
                        domain_routes_map,
                        "*",
                        &route_namespace,
                        &route_name,
                        rule,
                    );
                    changed_route_rules.push(route_rules_arc);
                }
            }
        }
        
        changed_route_rules
    }

    /// Add a single rule to RouteRules for the given hostname
    fn add_rule_to_route_rules(
        domain_routes_map: &mut DomainRouteRules,
        hostname: &str,
        route_namespace: &str,
        route_name: &str,
        rule: &crate::types::HTTPRouteRule,
    ) -> Arc<RouteRules> {
        let hostname_key = hostname.to_string();
        let route_rules = domain_routes_map
            .domain_routes_map
            .entry(hostname_key.clone())
            .or_insert_with(|| Arc::new(RouteRules {
                route_rules_list: Vec::new(),
                match_engine: Arc::new(RadixRouteMatchEngine::default()),
                need_rebuild: true,
            }));
        
        // Clone Arc to get mutable access
        let route_rules_mut = Arc::make_mut(route_rules);
        // Mark as needing rebuild since we're adding routes
        route_rules_mut.need_rebuild = true;
        // Create HttpRouteRuleUnit from HTTPRouteRule
        let rule_unit = HttpRouteRuleUnit::new(
            route_namespace.to_string(),
            route_name.to_string(),
            rule.clone(),
        );
        route_rules_mut.route_rules_list.push(rule_unit);
        
        // Return the updated Arc from the map
        domain_routes_map
            .domain_routes_map
            .get(&hostname_key)
            .unwrap()
            .clone()
    }
}