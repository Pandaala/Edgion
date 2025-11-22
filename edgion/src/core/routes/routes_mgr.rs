use std::collections::HashMap;
use std::sync::Arc;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::routes::{HttpRouteRuleUnit, r#match::RouteRuntime};
use crate::types::{HTTPRoute, ResourceMeta};

type DomainStr = String;

#[derive(Clone)]
pub struct RouteRules {
    route_rules_list: Vec<HttpRouteRuleUnit>,
}

#[derive(Clone)]
pub struct DomainRouteRules {
    domain_routes_map: HashMap<DomainStr, Arc<RouteRules>>,
}

pub struct RouteManager {
    gateway_listener_routes_map: HashMap<String, Arc<DomainRouteRules>>,
    // gateway_route_matcher: HashMap<DomainStr, >,
}

impl RouteManager {
    pub fn new() -> Self {
        Self {
            gateway_listener_routes_map: HashMap::new(),
        }
    }

    pub fn add_http_routes(&mut self, http_routes: Vec<HTTPRoute>) {
        for route in http_routes {
            let changed_domain_routes = self.add_http_route_single(route);
            // Traverse changed domain routes and output need_rebuild
            for domain_routes in changed_domain_routes {
                for (_, route_rules) in &domain_routes.domain_routes_map {
                    for rule_unit in &route_rules.route_rules_list {
                        if rule_unit.need_rebuild {
                            tracing::info!(
                                "HttpRouteRuleUnit needs rebuild: need_rebuild={}, identifier={}",
                                rule_unit.need_rebuild,
                                rule_unit.identifier()
                            );
                        }
                    }
                }
            }
        }
    }

    pub fn add_http_route(&mut self, route: HTTPRoute) {
        let changed_domain_routes = self.add_http_route_single(route);
        // Traverse changed domain routes and output need_rebuild
        for domain_routes in changed_domain_routes {
            for (_, route_rules) in &domain_routes.domain_routes_map {
                for rule_unit in &route_rules.route_rules_list {
                    if rule_unit.need_rebuild {
                        tracing::info!(
                            "HttpRouteRuleUnit needs rebuild: need_rebuild={}, identifier={}",
                            rule_unit.need_rebuild,
                            rule_unit.identifier()
                        );
                    }
                }
            }
        }
    }

    fn add_http_route_single(&mut self, route: HTTPRoute) -> Vec<Arc<DomainRouteRules>> {
        let mut changed_domain_routes = Vec::new();
        let parent_refs = match &route.spec.parent_refs {
            Some(refs) => refs,
            None => {
                tracing::warn!(
                    "HTTPRoute '{}' has no parent_refs, skipping",
                    route.key_name()
                );
                return changed_domain_routes;
            }
        };

        let gateway_store = get_global_gateway_store();
        let gateway_store_guard = gateway_store.read().unwrap();

        for parent_ref in parent_refs {
            // Build gateway key from parent_ref
            let gateway_key = if let Some(namespace) = parent_ref.namespace.as_ref() {
                format!("{}/{}", namespace, parent_ref.name)
            } else {
                // If namespace is not specified, use the route's namespace
                if let Some(route_namespace) = route.metadata.namespace.as_ref() {
                    format!("{}/{}", route_namespace, parent_ref.name)
                } else {
                    parent_ref.name.clone()
                }
            };

            // Check if gateway exists in global store
            match gateway_store_guard.get_gateway(&gateway_key) {
                Ok(_gateway) => {
                    // Gateway exists, add to gateway_listener_routes_map
                    // Key format: gateway_key/listener_name
                    let listener_name = parent_ref
                        .section_name
                        .as_ref()
                        .map(|s| s.clone())
                        .unwrap_or_else(|| "default".to_string());
                    
                    let map_key = format!("{}/{}", gateway_key, listener_name);
                    
                    // Get or create DomainRoutesMap for this gateway/listener combination
                    let domain_routes_map_arc = self
                        .gateway_listener_routes_map
                        .entry(map_key.clone())
                        .or_insert_with(|| Arc::new(DomainRouteRules {
                            domain_routes_map: HashMap::new(),
                        }));

                    // Clone Arc to get mutable access (Arc::make_mut ensures we have unique ownership)
                    let domain_routes_map = Arc::make_mut(domain_routes_map_arc);

                    // Add route rules to domain routes map
                    if let Some(rules) = &route.spec.rules {
                        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
                        let route_name = route.metadata.name.as_deref().unwrap_or("").to_string();
                        
                        for rule in rules {
                            if let Some(hostnames) = &route.spec.hostnames {
                                let hostname_vec = hostnames.clone();
                                for hostname in hostnames {
                                    let route_rules = domain_routes_map
                                        .domain_routes_map
                                        .entry(hostname.clone())
                                        .or_insert_with(|| Arc::new(RouteRules {
                                            route_rules_list: Vec::new(),
                                        }));
                                    
                                    // Clone Arc to get mutable access
                                    let route_rules = Arc::make_mut(route_rules);
                                    // Create HttpRouteRuleUnit from HTTPRouteRule
                                    let rule_unit = HttpRouteRuleUnit::new(
                                        route_namespace.clone(),
                                        route_name.clone(),
                                        rule.clone(),
                                    );
                                    route_rules.route_rules_list.push(rule_unit);
                                }
                            } else {
                                // If no hostnames specified, use "*" as default
                                let route_rules = domain_routes_map
                                    .domain_routes_map
                                    .entry("*".to_string())
                                    .or_insert_with(|| Arc::new(RouteRules {
                                        route_rules_list: Vec::new(),
                                    }));
                                
                                // Clone Arc to get mutable access
                                let route_rules = Arc::make_mut(route_rules);
                                // Create HttpRouteRuleUnit from HTTPRouteRule
                                let rule_unit = HttpRouteRuleUnit::new(
                                    route_namespace.clone(),
                                    route_name.clone(),
                                    rule.clone(),
                                );
                                route_rules.route_rules_list.push(rule_unit);
                            }
                        }
                    }

                    // Add to changed_domain_routes list (get the updated Arc from the map)
                    if let Some(domain_routes) = self.gateway_listener_routes_map.get(&map_key) {
                        changed_domain_routes.push(domain_routes.clone());
                    }

                    tracing::info!(
                        "Added HTTPRoute '{}' to gateway '{}' listener '{}'",
                        route.key_name(),
                        gateway_key,
                        listener_name
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
        
        changed_domain_routes
    }
}