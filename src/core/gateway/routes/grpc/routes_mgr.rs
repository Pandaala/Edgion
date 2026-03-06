use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex, RwLock};

use crate::core::gateway::runtime::GatewayInfo;
use crate::core::gateway::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::core::gateway::routes::grpc::{GrpcMatchEngine, GrpcRouteRuleUnit};
use crate::types::err::EdError;
use crate::types::{GRPCBackendRef, GRPCRoute, GRPCRouteRule};

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

impl Default for GrpcRouteRules {
    fn default() -> Self {
        Self::new()
    }
}

impl GrpcRouteRules {
    pub fn new() -> Self {
        Self {
            resource_keys: RwLock::new(HashSet::new()),
            route_rules_list: RwLock::new(Vec::new()),
            match_engine: None,
        }
    }

    /// Match a gRPC route.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    /// - `hostname`: Request hostname for route-level hostname matching
    ///
    /// Returns matched route unit and the specific `GatewayInfo` that passed validation.
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        gateway_infos: &[GatewayInfo],
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        if let Some(ref engine) = self.match_engine {
            engine.match_route(session, gateway_infos, hostname)
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
                    let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
                    (items, weights)
                }
                _ => (vec![], vec![]),
            };
            rule.backend_finder.init(items, weights);
        }

        // Select backend
        let backend_ref = rule.backend_finder.select().map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

        // Check if this backend reference is denied (no matching ReferenceGrant)
        if let Some(ref denied) = backend_ref.ref_denied {
            return Err(EdError::RefDenied {
                target_namespace: denied.target_namespace.clone(),
                target_name: denied.target_name.clone(),
                reason: denied
                    .reason
                    .clone()
                    .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
            });
        }

        // Note: BackendTLSPolicy query is performed in grpc peer selection
        // where route namespace is available for proper namespace inheritance

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

impl Default for DomainGrpcRouteRules {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainGrpcRouteRules {
    pub fn new() -> Self {
        Self {
            grpc_routes: ArcSwap::from_pointee(GrpcRouteRules::new()),
        }
    }

    /// Match a route based on service/method.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    /// - `hostname`: Request hostname for route-level hostname matching
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        gateway_infos: &[GatewayInfo],
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        let grpc_routes = self.grpc_routes.load();
        grpc_routes.match_route(session, gateway_infos, hostname)
    }
}

/// Global gRPC route manager
pub struct GrpcRouteManager {
    /// Single global gRPC route table shared by all gateways/listeners.
    pub global_grpc_routes: ArcSwap<DomainGrpcRouteRules>,

    /// Stores all GRPCRoute resources for lookup during delete events.
    /// Key format: "namespace/name"
    /// Uses Mutex since route updates are serialized.
    pub grpc_routes: Mutex<HashMap<RouteKey, GRPCRoute>>,
}

impl Default for GrpcRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GrpcRouteManager {
    pub fn new() -> Self {
        Self {
            global_grpc_routes: ArcSwap::from_pointee(DomainGrpcRouteRules::new()),
            grpc_routes: Mutex::new(HashMap::new()),
        }
    }

    /// Get the current global gRPC route table snapshot.
    pub fn get_global_grpc_routes(&self) -> arc_swap::Guard<Arc<DomainGrpcRouteRules>> {
        self.global_grpc_routes.load()
    }
}

// Global GrpcRouteManager instance
static GLOBAL_GRPC_ROUTE_MANAGER: LazyLock<Arc<GrpcRouteManager>> = LazyLock::new(|| Arc::new(GrpcRouteManager::new()));

/// Get the global GrpcRouteManager instance
pub fn get_global_grpc_route_manager() -> Arc<GrpcRouteManager> {
    GLOBAL_GRPC_ROUTE_MANAGER.clone()
}
