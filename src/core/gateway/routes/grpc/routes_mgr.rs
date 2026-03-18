use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::core::gateway::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::core::gateway::routes::grpc::{GrpcMatchEngine, GrpcRouteRuleUnit};
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::err::EdError;
use crate::types::{GRPCBackendRef, GRPCRoute, GRPCRouteRule};

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
        if !rule.backend_finder.is_initialized() {
            let (items, weights) = match &rule.backend_refs {
                Some(refs) if !refs.is_empty() => {
                    let items: Vec<GRPCBackendRef> = refs.clone();
                    let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
                    (items, weights)
                }
                _ => (vec![], vec![]),
            };
            rule.backend_finder.init(items, weights);
        }

        let backend_ref = rule.backend_finder.select().map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

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

/// gRPC route rules container (no hostname-based separation — gRPC uses a flat engine)
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

// ============================================================================
// Per-port gRPC route manager (mirrors HttpPortRouteManager pattern)
// ============================================================================

/// Per-port gRPC route manager.
pub struct GrpcPortRouteManager {
    route_table: ArcSwap<DomainGrpcRouteRules>,
}

impl Default for GrpcPortRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GrpcPortRouteManager {
    pub fn new() -> Self {
        Self {
            route_table: ArcSwap::from_pointee(DomainGrpcRouteRules::new()),
        }
    }

    /// Load the current route table snapshot (hot path, per-request).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<DomainGrpcRouteRules>> {
        self.route_table.load()
    }

    /// Replace the route table with a new snapshot.
    pub fn store_route_table(&self, table: DomainGrpcRouteRules) {
        self.route_table.store(Arc::new(table));
    }
}

// ============================================================================
// Global gRPC route managers (mirrors GlobalHttpRouteManagers pattern)
// ============================================================================

/// Global wrapper managing `port -> Arc<GrpcPortRouteManager>`.
pub struct GlobalGrpcRouteManagers {
    /// Canonical route store: resource_key -> GRPCRoute
    pub(crate) route_cache: DashMap<String, GRPCRoute>,

    /// port -> per-port manager
    pub(crate) by_port: DashMap<u16, Arc<GrpcPortRouteManager>>,

    /// Cached parsed route units per resource_key.
    pub(crate) route_units_cache: Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>,
}

impl Default for GlobalGrpcRouteManagers {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalGrpcRouteManagers {
    pub fn new() -> Self {
        Self {
            route_cache: DashMap::new(),
            by_port: DashMap::new(),
            route_units_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create a per-port `GrpcPortRouteManager`.
    pub fn get_or_create_port_manager(&self, port: u16) -> Arc<GrpcPortRouteManager> {
        self.by_port
            .entry(port)
            .or_insert_with(|| Arc::new(GrpcPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Rebuild all per-port managers from the current route cache and units cache.
    pub fn rebuild_all_port_managers(&self) {
        // Pre-create port managers for all known ports
        for entry in self.route_cache.iter() {
            if let Some(parent_refs) = &entry.value().spec.parent_refs {
                for pr in parent_refs {
                    if let Some(port) = pr.port {
                        self.get_or_create_port_manager(port as u16);
                    }
                }
            }
        }
        let pgis = crate::core::gateway::runtime::store::get_port_gateway_info_store();
        for port in pgis.all_ports() {
            self.get_or_create_port_manager(port);
        }

        let port_buckets = self.bucket_units_by_port();

        let mut rebuilt_ports = 0u32;

        for (port, units) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            let table = build_grpc_route_rules_from_units(units);
            manager.store_route_table(table);
            rebuilt_ports += 1;
        }

        let active_ports: HashSet<u16> = pgis.all_ports().into_iter().collect();
        let stale_ports: Vec<u16> = self
            .by_port
            .iter()
            .filter(|e| !port_buckets.contains_key(e.key()) && !active_ports.contains(e.key()))
            .map(|e| *e.key())
            .collect();
        for port in &stale_ports {
            self.by_port.remove(port);
        }
        for entry in self.by_port.iter() {
            let port = *entry.key();
            if !port_buckets.contains_key(&port) {
                entry.value().store_route_table(DomainGrpcRouteRules::new());
            }
        }

        tracing::info!(
            component = "global_grpc_route_managers",
            ports = port_buckets.len(),
            rebuilt_ports,
            removed_stale_ports = stale_ports.len(),
            "Rebuilt all per-port gRPC route managers"
        );
    }

    /// Rebuild only the per-port managers for the given set of affected ports.
    pub fn rebuild_affected_port_managers(&self, affected_ports: &HashSet<u16>) {
        if affected_ports.is_empty() {
            return;
        }

        let units_cache = self.route_units_cache.lock().unwrap();
        let mut port_buckets: HashMap<u16, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> = HashMap::new();
        for &port in affected_ports {
            port_buckets.insert(port, HashMap::new());
        }

        for entry in self.route_cache.iter() {
            let route = entry.value();
            let ports = resolved_ports_for_grpc_route(route);
            let effective_ports: Vec<u16> = if !ports.is_empty() {
                ports.to_vec()
            } else {
                let fb = extract_ports_from_grpc_parent_refs(route);
                if fb.is_empty() {
                    affected_ports.iter().copied().collect()
                } else {
                    fb
                }
            };
            for port in effective_ports {
                if let Some(bucket) = port_buckets.get_mut(&port) {
                    if let Some(units) = units_cache.get(entry.key()) {
                        bucket.insert(entry.key().clone(), units.clone());
                    }
                }
            }
        }
        drop(units_cache);

        let pgis = crate::core::gateway::runtime::store::get_port_gateway_info_store();
        let active_ports: HashSet<u16> = pgis.all_ports().into_iter().collect();
        let mut removed_stale = 0usize;
        for (port, units) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            let table = build_grpc_route_rules_from_units(units);
            manager.store_route_table(table);
            if units.is_empty() && !active_ports.contains(port) {
                self.by_port.remove(port);
                removed_stale += 1;
            }
        }

        tracing::info!(
            component = "global_grpc_route_managers",
            affected = affected_ports.len(),
            removed_stale_ports = removed_stale,
            "Rebuilt affected per-port gRPC route managers"
        );
    }

    /// Bucket all cached units by their routes' resolved ports.
    ///
    /// When a route has no `resolved_ports`, falls back to parentRef.port,
    /// then to all known ports for backward compatibility.
    fn bucket_units_by_port(&self) -> HashMap<u16, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> {
        let units_cache = self.route_units_cache.lock().unwrap();
        let mut port_buckets: HashMap<u16, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value();
            let ports = resolved_ports_for_grpc_route(route);

            let effective_ports: Vec<u16> = if !ports.is_empty() {
                ports.to_vec()
            } else {
                let fb = extract_ports_from_grpc_parent_refs(route);
                if fb.is_empty() {
                    self.by_port.iter().map(|e| *e.key()).collect()
                } else {
                    fb
                }
            };

            if let Some(units) = units_cache.get(&route_key) {
                for port in &effective_ports {
                    port_buckets
                        .entry(*port)
                        .or_default()
                        .insert(route_key.clone(), units.clone());
                }
            }
        }

        port_buckets
    }

    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> GrpcRouteManagerStats {
        let grpc_routes = self.route_cache.len();
        let port_count = self.by_port.len();
        let route_units_cache = self.route_units_cache.lock().unwrap().len();

        GrpcRouteManagerStats {
            grpc_routes,
            resource_keys: grpc_routes,
            route_units_cache,
            port_count,
        }
    }
}

/// Build DomainGrpcRouteRules from a per-port units map.
fn build_grpc_route_rules_from_units(all_units: &HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>) -> DomainGrpcRouteRules {
    let mut resource_keys = HashSet::new();
    let mut route_rules_list: Vec<Arc<GrpcRouteRuleUnit>> = Vec::new();

    for (key, units) in all_units {
        if !units.is_empty() {
            resource_keys.insert(key.clone());
            route_rules_list.extend(units.iter().cloned());
        }
    }

    let match_engine = if route_rules_list.is_empty() {
        None
    } else {
        Some(Arc::new(GrpcMatchEngine::new(route_rules_list.clone())))
    };

    let grpc_route_rules = Arc::new(GrpcRouteRules {
        resource_keys: RwLock::new(resource_keys),
        route_rules_list: RwLock::new(route_rules_list),
        match_engine,
    });

    let domain = DomainGrpcRouteRules::new();
    domain.grpc_routes.store(grpc_route_rules);
    domain
}

/// Get the resolved listener ports for a GRPCRoute.
pub(crate) fn resolved_ports_for_grpc_route(route: &GRPCRoute) -> &[u16] {
    route.spec.resolved_ports.as_deref().unwrap_or_default()
}

/// Extract ports directly from parentRef.port fields (fallback).
fn extract_ports_from_grpc_parent_refs(route: &GRPCRoute) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Some(parent_refs) = &route.spec.parent_refs {
        for pr in parent_refs {
            if let Some(port) = pr.port {
                ports.push(port as u16);
            }
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GrpcRouteManagerStats {
    pub grpc_routes: usize,
    pub resource_keys: usize,
    pub route_units_cache: usize,
    pub port_count: usize,
}

static GLOBAL_GRPC_ROUTE_MANAGERS: OnceLock<GlobalGrpcRouteManagers> = OnceLock::new();

pub fn get_global_grpc_route_managers() -> &'static GlobalGrpcRouteManagers {
    GLOBAL_GRPC_ROUTE_MANAGERS.get_or_init(GlobalGrpcRouteManagers::new)
}

// Legacy aliases for backward compatibility
pub type GrpcRouteManager = GlobalGrpcRouteManagers;

pub fn get_global_grpc_route_manager() -> &'static GlobalGrpcRouteManagers {
    get_global_grpc_route_managers()
}
