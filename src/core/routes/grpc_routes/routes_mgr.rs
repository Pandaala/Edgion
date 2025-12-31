use std::sync::{Arc, RwLock, Mutex};
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use arc_swap::ArcSwap;
use std::sync::LazyLock;

use crate::core::routes::grpc_routes::{GrpcRouteRuleUnit, GrpcMatchEngine};
use crate::types::{GRPCRoute, GRPCRouteRule, GRPCBackendRef};
use crate::types::err::EdError;
use crate::core::lb::{ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

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
        listener_name: &str,
        hostname: &str,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        if let Some(ref engine) = self.match_engine {
            engine.match_route(session, listener_name, hostname)
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
        let mut backend_ref = rule.backend_finder.select().map_err(|err_code| {
            match err_code {
                ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
                ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
                _ => EdError::BackendNotFound(),
            }
        })?;
        
        // Query BackendTLSPolicy for the selected backend
        let service_group = backend_ref.group.as_deref().unwrap_or("");
        let service_kind = backend_ref.kind.as_deref().unwrap_or("Service");
        let service_name = &backend_ref.name;
        let service_namespace = backend_ref.namespace.as_deref();
        
        backend_ref.backend_tls_policy = crate::core::backends::query_backend_tls_policy_for_service(
            service_group,
            service_kind,
            service_name,
            service_namespace,
        );
        
        if let Some(ref policy) = backend_ref.backend_tls_policy {
            tracing::debug!(
                policy = %format!("{}/{}", 
                    policy.namespace().unwrap_or(""), 
                    policy.name()
                ),
                service = %format!("{}/{}", 
                    service_namespace.unwrap_or(""), 
                    service_name
                ),
                sni = %policy.spec.validation.hostname,
                "BackendTLSPolicy found for selected gRPC backend"
            );
        }
        
        Ok(backend_ref)
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

/// gRPC route rules for a gateway (no hostname-based separation)
pub struct DomainGrpcRouteRules {
    pub grpc_routes: ArcSwap<GrpcRouteRules>,
}

impl DomainGrpcRouteRules {
    pub fn new() -> Self {
        Self {
            grpc_routes: ArcSwap::from_pointee(GrpcRouteRules::new()),
        }
    }

    /// Match a route based on service/method
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        listener_name: &str,
        hostname: &str,
    ) -> Result<Arc<GrpcRouteRuleUnit>, EdError> {
        let grpc_routes = self.grpc_routes.load();
        grpc_routes.match_route(session, listener_name, hostname)
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
static GLOBAL_GRPC_ROUTE_MANAGER: LazyLock<Arc<GrpcRouteManager>> =
    LazyLock::new(|| Arc::new(GrpcRouteManager::new()));

/// Get the global GrpcRouteManager instance
pub fn get_global_grpc_route_manager() -> Arc<GrpcRouteManager> {
    GLOBAL_GRPC_ROUTE_MANAGER.clone()
}

