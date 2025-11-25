use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use dashmap::DashMap;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::routes::HttpRouteRuleUnit;
use crate::core::routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::types::{HTTPRoute, ResourceMeta};

type DomainStr = String;

pub struct RouteRules {
    route_rules_list: RwLock<Vec<HttpRouteRuleUnit>>,
    match_engine: Arc<RadixRouteMatchEngine>,
}

impl Clone for RouteRules {
    fn clone(&self) -> Self {
        Self {
            route_rules_list: RwLock::new(self.route_rules_list.read().unwrap().clone()),
            match_engine: self.match_engine.clone(),
        }
    }
}

impl RouteRules {
    /// Match a route using the match_engine engine
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
    ) -> Result<Arc<dyn crate::core::routes::match_engine::RouteEntry>, crate::types::err::EdError> {
        self.match_engine.match_route(session)
    }
}

pub struct DomainRouteRules {
    domain_routes_map: ArcSwap<Arc<HashMap<DomainStr, Arc<RouteRules>>>>,
}

impl DomainRouteRules {
    /// Match a route for the given hostname and session
    /// Returns the matched RouteEntry if found, or an error if no route matches
    pub fn match_route(
        &self,
        hostname: &str,
        session: &mut pingora_proxy::Session,
    ) -> Result<Arc<dyn crate::core::routes::match_engine::RouteEntry>, crate::types::err::EdError> {
        let domain_routes_map = self.domain_routes_map.load();
        
        // Try to find RouteRules for the hostname (exact match_engine only)
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

pub struct RouteManager {
    gateway_routes_map: DashMap<GatewayKey, Arc<DomainRouteRules>>,
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
        }
    }

    /// Get or create DomainRouteRules for a specific gateway by namespace and name
    /// This ensures the gateway has a route map even if no HTTPRoutes exist yet
    pub fn get_or_create_domain_routes(&self, namespace: &str, name: &str) -> Arc<DomainRouteRules> {
        let gateway_key = format!("{}/{}", namespace, name);
        self.gateway_routes_map
            .entry(gateway_key)
            .or_insert_with(|| Arc::new(DomainRouteRules {
                domain_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
            }))
            .value()
            .clone()
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
                    route_rules_list: RwLock::new(Vec::new()),
                    match_engine: Arc::new(RadixRouteMatchEngine::default()),
                }));
            
            // Get mutable access to RouteRules
            let route_rules_mut = Arc::make_mut(route_rules_arc);
            
            // Create HttpRouteRuleUnit from HTTPRouteRule
            let rule_unit = HttpRouteRuleUnit::new(
                route_namespace.to_string(),
                route_name.to_string(),
                rule.clone(),
            );
            
            // Add the rule to the list
            route_rules_mut.route_rules_list.write().unwrap().push(rule_unit);
            
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
}