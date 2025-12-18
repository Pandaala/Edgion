use std::sync::{Arc, RwLock, Mutex};
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;

use crate::core::routes::grpc_routes::{GrpcRouteRuleUnit, GrpcMatchEngine};
use crate::types::{GRPCRoute, GRPCRouteRule, GRPCBackendRef};
use crate::types::err::EdError;
use crate::core::lb::{ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

type DomainStr = String;
type GatewayKey = String;
type RouteKey = String; // Format: "namespace/name"

/// gRPC route rules for a specific domain
pub struct GrpcRouteRules {
    /// All resource keys (GRPCRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub resource_keys: RwLock<HashSet<String>>,
    
    /// All route rule units (stored as Arc to avoid cloning)
    pub route_rules_list: RwLock<Vec<Arc<GrpcRouteRuleUnit>>>,
    
    /// Match engine for service/method routing
    /// None if there are no routes
    pub match_engine: Option<Arc<GrpcMatchEngine>>,
}

impl GrpcRouteRules {
    pub fn new() -> Self {
        Self {
            resource_keys: RwLock::new(HashSet::new()),
            route_rules_list: RwLock::new(Vec::new()),
            match_engine: None,
        }
    }

    /// Match a gRPC route
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        if let Some(ref engine) = self.match_engine {
            engine.match_route(session)
        } else {
            Err(EdError::RouteNotFound())
        }
    }

    /// Select backend from the matched GRPCRouteRule
    pub fn select_backend(rule: &Arc<GRPCRouteRule>) -> Result<GRPCBackendRef, EdError> {
        // Initialize selector if not yet initialized
        if !rule.backend_finder.is_initialized() {
            let (items, weights) = match &rule.backend_refs {
                Some(refs) if !refs.is_empty() => {
                    let items: Vec<GRPCBackendRef> = refs.clone();
                    // Default weight to 1 if not specified
                    let weights: Vec<Option<i32>> = refs.iter()
                        .map(|br| br.weight.or(Some(1)))
                        .collect();
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
}

impl Clone for GrpcRouteRules {
    fn clone(&self) -> Self {
        Self {
            resource_keys: RwLock::new(self.resource_keys.read().unwrap().clone()),
            route_rules_list: RwLock::new(self.route_rules_list.read().unwrap().clone()),
            match_engine: self.match_engine.clone(),
        }
    }
}

/// Domain-based gRPC route rules mapping
pub struct DomainGrpcRouteRules {
    pub domain_routes_map: ArcSwap<Arc<HashMap<DomainStr, Arc<GrpcRouteRules>>>>,
}

impl DomainGrpcRouteRules {
    pub fn new() -> Self {
        Self {
            domain_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }

    /// Match a route for the given hostname and session
    pub fn match_route(
        &self,
        hostname: &str,
        session: &mut pingora_proxy::Session,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        let domain_routes_map = self.domain_routes_map.load();

        // Try to find GrpcRouteRules for the hostname (exact match only)
        let route_rules = domain_routes_map.get(hostname).cloned();

        if let Some(route_rules) = route_rules {
            route_rules.match_route(session)
        } else {
            Err(EdError::RouteNotFound())
        }
    }
}

/// Global gRPC route manager
pub struct GrpcRouteManager {
    /// Maps gateway key to domain gRPC route rules
    pub gateway_routes_map: DashMap<GatewayKey, Arc<DomainGrpcRouteRules>>,

    /// Stores all GRPCRoute resources for lookup during delete events
    /// Key format: "namespace/name"
    /// Uses Mutex since route updates are serialized
    pub grpc_routes: Mutex<HashMap<RouteKey, GRPCRoute>>,
}

impl GrpcRouteManager {
    pub fn new() -> Self {
        Self {
            gateway_routes_map: DashMap::new(),
            grpc_routes: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create DomainGrpcRouteRules for a gateway
    /// Ensures that each gateway has exactly one DomainGrpcRouteRules instance
    pub fn get_or_create_domain_grpc_routes(&self, namespace: &str, name: &str) -> Arc<DomainGrpcRouteRules> {
        let gateway_key = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", namespace, name)
        };
        
        let entry = self.gateway_routes_map.entry(gateway_key.clone());
        let is_new = matches!(entry, dashmap::mapref::entry::Entry::Vacant(_));
        
        let domain_routes = entry
            .or_insert_with(|| Arc::new(DomainGrpcRouteRules::new()))
            .value()
            .clone();
        
        if is_new {
            tracing::info!(gateway_key = %gateway_key, "Created new gRPC domain routes for gateway");
        }
        
        domain_routes
    }
}

// Global GrpcRouteManager instance
static GLOBAL_GRPC_ROUTE_MANAGER: Lazy<Arc<GrpcRouteManager>> =
    Lazy::new(|| Arc::new(GrpcRouteManager::new()));

/// Get the global GrpcRouteManager instance
pub fn get_global_grpc_route_manager() -> Arc<GrpcRouteManager> {
    GLOBAL_GRPC_ROUTE_MANAGER.clone()
}

