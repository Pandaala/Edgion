use std::collections::HashMap;
use std::sync::Arc;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::types::{HTTPRoute, HTTPRouteRule, ResourceMeta};

type DomainStr = String;

#[derive(Clone)]
pub struct RouteRules {
    route_rules_list: Vec<HTTPRouteRule>,
}

pub struct DomainRoutesMap {
    domain_routes_map: HashMap<DomainStr, Arc<RouteRules>>,
    need_rebuild: bool,
}

pub struct RouteManager {
    gateway_listener_routes_map: HashMap<String, DomainRoutesMap>,
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
            self.add_http_route_single(route);
        }
    }

    pub fn add_http_route(&mut self, route: HTTPRoute) {
        self.add_http_route_single(route);


    fn add_http_route_single(&mut self, route: HTTPRoute) {
        let parent_refs = match &route.spec.parent_refs {
            Some(refs) => refs,
            None => {
                tracing::warn!(
                    "HTTPRoute '{}' has no parent_refs, skipping",
                    route.key_name()
                );
                return;
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
                    let domain_routes_map = self
                        .gateway_listener_routes_map
                        .entry(map_key)
                        .or_insert_with(|| DomainRoutesMap {
                            domain_routes_map: HashMap::new(),
                            need_rebuild: true,
                        });

                    // Add route rules to domain routes map
                    if let Some(rules) = &route.spec.rules {
                        for rule in rules {
                            if let Some(hostnames) = &route.spec.hostnames {
                                for hostname in hostnames {
                                    let route_rules = domain_routes_map
                                        .domain_routes_map
                                        .entry(hostname.clone())
                                        .or_insert_with(|| Arc::new(RouteRules {
                                            route_rules_list: Vec::new(),
                                        }));
                                    
                                    // Clone Arc to get mutable access
                                    let route_rules = Arc::make_mut(route_rules);
                                    route_rules.route_rules_list.push(rule.clone());
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
                                route_rules.route_rules_list.push(rule.clone());
                            }
                        }
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
    }
}