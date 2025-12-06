use std::sync::{Arc, RwLock, Mutex};
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use crate::core::routes::{HttpRouteRuleUnit, HttpRouteRuleRegexUnit};
use crate::types::{HTTPRouteRule, MatchInfo, HTTPBackendRef};
use crate::types::err::EdError;
use crate::core::routes::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::routes::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::types::HTTPRoute;
use crate::core::lb::{ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

type DomainStr = String;

pub struct RouteRules {
    /// All resource keys (HTTPRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub(crate) resource_keys: RwLock<HashSet<String>>,
    
    /// Exact and prefix match routes (handled by radix tree)
    pub(crate) route_rules_list: RwLock<Vec<HttpRouteRuleUnit>>,
    /// Match engine for exact/prefix routes. None if there are no normal routes (only regex routes)
    pub(crate) match_engine: Option<Arc<RadixRouteMatchEngine>>,
    
    /// Regex match routes (handled separately)
    /// Kept for backward compatibility and debugging, but matching uses regex_routes_engine
    pub(crate) regex_routes: RwLock<Vec<HttpRouteRuleRegexUnit>>,
    /// Regex routes engine for lock-free matching. None if there are no regex routes
    pub(crate) regex_routes_engine: Option<Arc<RegexRoutesEngine>>,
}

impl Clone for RouteRules {
    fn clone(&self) -> Self {
        Self {
            resource_keys: RwLock::new(self.resource_keys.read().unwrap().clone()),
            route_rules_list: RwLock::new(self.route_rules_list.read().unwrap().clone()),
            match_engine: self.match_engine.clone(),
            regex_routes: RwLock::new(self.regex_routes.read().unwrap().clone()),
            regex_routes_engine: self.regex_routes_engine.clone(),
        }
    }
}

impl RouteRules {
    /// Select backend from the matched route rule
    fn select_backend(rule: &Arc<HTTPRouteRule>) -> Result<HTTPBackendRef, EdError> {
        // Initialize selector if not yet initialized
        if !rule.backend_finder.is_initialized() {
            let (items, weights) = match &rule.backend_refs {
                Some(refs) if !refs.is_empty() => {
                    let items: Vec<HTTPBackendRef> = refs.clone();
                    // Default weight to 1 if not specified
                    let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
                    (items, weights)
                }
                _ => (vec![], vec![]),
            };
            rule.backend_finder.init(items, weights);
        }

        // Select backend
        rule.backend_finder.select().map_err(|err_code| {
            match err_code {
                ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
                ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
                _ => EdError::BackendNotFound(),
            }
        })
    }

    /// Match a route using the match_engine engine
    /// Try match in order: exact → regex → prefix
    /// Returns (Arc<MatchInfo>, HTTPBackendRef) on success
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
    ) -> Result<(Arc<MatchInfo>, HTTPBackendRef), EdError> {
        // Step 1: Try exact match first (highest priority) - only if match_engine exists
        if let Some(ref match_engine) = self.match_engine {
            if let Some(route_entry) = match_engine.exact_match(session)? {
                tracing::debug!(path=%session.req_header().uri.path(),"exact match ok");
                // Convert RouteEntry back to HttpRouteRuleUnit
                let route_entry_id = route_entry.identifier();
                let route_rules = self.route_rules_list.read().unwrap();
                if let Some(unit) = route_rules.iter().find(|u| format!("{}/{}", u.matched_info.rns, u.matched_info.rn) == route_entry_id) {
                    let backend = Self::select_backend(&unit.rule)?;
                    return Ok((unit.matched_info.clone(), backend));
                }
            }
        }
        
        // Step 2: Try regex match - use engine if available
        if let Some(ref regex_engine) = self.regex_routes_engine {
            if let Some(result) = regex_engine.match_route(session)? {
                tracing::debug!(path=%session.req_header().uri.path(),"regex match ok");
                return Ok(result);
            }
        }
        
        // Step 3: Fall back to prefix match - only if match_engine exists
        if let Some(ref match_engine) = self.match_engine {
            let route_entry = match_engine.prefix_match(session)?;
            tracing::debug!(path=%session.req_header().uri.path(),"prefix match ok");
            
            // Convert RouteEntry back to HttpRouteRuleUnit
            let route_entry_id = route_entry.identifier();
            let route_rules = self.route_rules_list.read().unwrap();
            if let Some(unit) = route_rules.iter().find(|u| format!("{}/{}", u.matched_info.rns, u.matched_info.rn) == route_entry_id) {
                let backend = Self::select_backend(&unit.rule)?;
                return Ok((unit.matched_info.clone(), backend));
            }
        }
        
        // No route matched
        Err(EdError::RouteNotFound())
    }
}

pub struct DomainRouteRules {
    pub(crate) domain_routes_map: ArcSwap<Arc<HashMap<DomainStr, Arc<RouteRules>>>>,
}

impl DomainRouteRules {
    /// Match a route for the given hostname and session
    /// Returns (Arc<MatchInfo>, HTTPBackendRef) if found, or an error if no route matches
    pub fn match_route(
        &self,
        hostname: &str,
        session: &mut pingora_proxy::Session,
    ) -> Result<(Arc<MatchInfo>, HTTPBackendRef), EdError> {
        let domain_routes_map = self.domain_routes_map.load();
        
        // Try to find RouteRules for the hostname (exact match only)
        let route_rules = domain_routes_map
            .get(hostname)
            .cloned();

        if let Some(route_rules) = route_rules {
            route_rules.match_route(session)
        } else {
            Err(EdError::RouteNotFound())
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
            tracing::info!(gateway_key = %gateway_key, "Created new domain routes for gateway");
        }
        
        domain_routes
    }
}