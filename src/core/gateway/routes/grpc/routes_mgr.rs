use arc_swap::ArcSwap;
use dashmap::DashMap;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use crate::core::gateway::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::core::gateway::routes::grpc::{GrpcMatchEngine, GrpcRouteRuleUnit};
use crate::core::gateway::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::ctx::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::{GRPCBackendRef, GRPCRoute, GRPCRouteRule, ListenerKey};

/// gRPC route rules for a specific domain.
///
/// Instances are constructed once in `build_grpc_route_rules_from_units` and then
/// wrapped in `Arc<GrpcRouteRules>` and published via the outer `ArcSwap`, so the
/// fields are effectively immutable after construction. No interior mutability
/// (no `RwLock`/`Mutex`) is required.
#[derive(Clone)]
pub struct GrpcRouteRules {
    /// All resource keys (GRPCRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub resource_keys: HashSet<String>,

    /// All route rule units (stored as Arc to avoid cloning)
    pub route_rules_list: Vec<Arc<GrpcRouteRuleUnit>>,

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
            resource_keys: HashSet::new(),
            route_rules_list: Vec::new(),
            match_engine: None,
        }
    }

    /// Match a gRPC route.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing the normalized current path
    /// - `index`: Pre-built gateway/listener index for this port
    /// - `hostname`: Request hostname for route-level hostname matching
    ///
    /// Returns matched route unit and the specific `GatewayInfo` that passed validation.
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        if let Some(ref engine) = self.match_engine {
            engine.match_route(session, ctx, index, hostname)
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
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, GatewayInfo), EdError> {
        let grpc_routes = self.grpc_routes.load();
        grpc_routes.match_route(session, ctx, index, hostname)
    }

    /// Returns true if this port has any gRPC routes registered.
    pub fn has_routes(&self) -> bool {
        self.grpc_routes.load().match_engine.is_some()
    }
}

// ============================================================================
// Per-listener gRPC route manager (mirrors HttpPortRouteManager pattern)
// ============================================================================

/// Per-listener gRPC route manager.
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

/// Global wrapper managing `ListenerKey -> Arc<GrpcPortRouteManager>`.
pub struct GlobalGrpcRouteManagers {
    /// Canonical route store: resource_key -> GRPCRoute
    pub(crate) route_cache: DashMap<String, GRPCRoute>,

    /// listener -> per-listener manager
    pub(crate) by_listener: DashMap<ListenerKey, Arc<GrpcPortRouteManager>>,

    /// Cached parsed route units per resource_key.
    pub(crate) route_units_cache: Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>,

    /// Serializes the read-snapshot-then-rewrite sequences that mutate
    /// `route_cache` + `route_units_cache` + `by_listener`. The GRPCRoute cache
    /// task (`full_set`/`partial_update`) and the Gateway cache task
    /// (`rebuild_all_listener_managers` via `runtime::handler`) reach this manager
    /// concurrently under different per-kind locks; this is the only lock both
    /// contend on, preventing a stale-snapshot rebuild leak. Synchronous only.
    rebuild_lock: parking_lot::Mutex<()>,
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
            by_listener: DashMap::new(),
            route_units_cache: Mutex::new(HashMap::new()),
            rebuild_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Acquire the rebuild lock. Held for the whole body of `full_set`,
    /// `partial_update`, and the public `rebuild_all_listener_managers`.
    /// NOT reentrant — internal callers use the `*_locked` variants.
    pub(crate) fn lock_rebuild(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.rebuild_lock.lock()
    }

    /// Get or create a per-listener `GrpcPortRouteManager`.
    pub fn get_or_create_listener_manager(&self, key: &ListenerKey) -> Arc<GrpcPortRouteManager> {
        self.by_listener
            .entry(key.clone())
            .or_insert_with(|| Arc::new(GrpcPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Rebuild all per-listener managers from the current route cache and units cache.
    ///
    /// Public entry point (called from `runtime::handler` on listener changes);
    /// acquires the rebuild lock. `full_set` uses `*_locked` (it already holds it).
    pub fn rebuild_all_listener_managers(&self) {
        let _guard = self.rebuild_lock.lock();
        self.rebuild_all_listener_managers_locked();
    }

    /// Rebuild body. The caller MUST hold `rebuild_lock`.
    pub(crate) fn rebuild_all_listener_managers_locked(&self) {
        // Pre-create listener managers for all known listeners from ListenerGatewayInfoStore
        let pgis = crate::core::gateway::runtime::store::get_listener_gateway_info_store();
        for key in pgis.all_listener_keys() {
            self.get_or_create_listener_manager(&key);
        }

        let listener_buckets = self.bucket_units_by_listener();

        let mut rebuilt_listeners = 0u32;

        for (key, units) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let table = build_grpc_route_rules_from_units(units);
            manager.store_route_table(table);
            rebuilt_listeners += 1;
        }

        let active_listeners: HashSet<ListenerKey> = pgis.all_listener_keys().into_iter().collect();
        let stale_listeners: Vec<ListenerKey> = self
            .by_listener
            .iter()
            .filter(|e| !listener_buckets.contains_key(e.key()) && !active_listeners.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for key in &stale_listeners {
            self.by_listener.remove(key);
        }
        for entry in self.by_listener.iter() {
            let key = entry.key().clone();
            if !listener_buckets.contains_key(&key) {
                entry.value().store_route_table(DomainGrpcRouteRules::new());
            }
        }

        tracing::info!(
            component = "global_grpc_route_managers",
            listeners = listener_buckets.len(),
            rebuilt_listeners,
            removed_stale_listeners = stale_listeners.len(),
            "Rebuilt all per-listener gRPC route managers"
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected listeners.
    pub fn rebuild_affected_listener_managers(&self, affected_listeners: &HashSet<ListenerKey>) {
        if affected_listeners.is_empty() {
            return;
        }

        let units_cache = self.route_units_cache.lock();
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> = HashMap::new();
        for key in affected_listeners {
            listener_buckets.insert(key.clone(), HashMap::new());
        }

        for entry in self.route_cache.iter() {
            let route = entry.value();
            let listeners = resolved_listeners_for_grpc_route(route);
            for key in listeners {
                if let Some(bucket) = listener_buckets.get_mut(key) {
                    if let Some(units) = units_cache.get(entry.key()) {
                        bucket.insert(entry.key().clone(), units.clone());
                    }
                }
            }
        }
        drop(units_cache);

        let pgis = crate::core::gateway::runtime::store::get_listener_gateway_info_store();
        let active_listeners: HashSet<ListenerKey> = pgis.all_listener_keys().into_iter().collect();
        let mut removed_stale = 0usize;
        for (key, units) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let table = build_grpc_route_rules_from_units(units);
            manager.store_route_table(table);
            if units.is_empty() && !active_listeners.contains(key) {
                self.by_listener.remove(key);
                removed_stale += 1;
            }
        }

        // Sweep orphan entries left over from earlier rebuild_all calls — see the
        // matching comment in HTTP routes_mgr::rebuild_affected_listener_managers.
        let route_referenced: HashSet<ListenerKey> = self
            .route_cache
            .iter()
            .flat_map(|e| resolved_listeners_for_grpc_route(e.value()).to_vec())
            .collect();
        let orphans: Vec<ListenerKey> = self
            .by_listener
            .iter()
            .filter(|e| !route_referenced.contains(e.key()) && !active_listeners.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for key in &orphans {
            self.by_listener.remove(key);
        }
        removed_stale += orphans.len();

        tracing::info!(
            component = "global_grpc_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener gRPC route managers"
        );
    }

    /// Bucket all cached units by their routes' resolved listeners.
    ///
    /// When a route has no `resolved_listeners`, it is skipped with a warning.
    fn bucket_units_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> {
        let units_cache = self.route_units_cache.lock();
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value();
            let listeners = resolved_listeners_for_grpc_route(route);

            if listeners.is_empty() {
                tracing::warn!(route = %route_key, "GRPCRoute has no resolved_listeners, skipping");
                continue;
            }

            if let Some(units) = units_cache.get(&route_key) {
                for key in listeners {
                    listener_buckets
                        .entry(key.clone())
                        .or_default()
                        .insert(route_key.clone(), units.clone());
                }
            }
        }

        listener_buckets
    }

    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> GrpcRouteManagerStats {
        let grpc_routes = self.route_cache.len();
        let listener_count = self.by_listener.len();
        let route_units_cache = self.route_units_cache.lock().len();

        GrpcRouteManagerStats {
            grpc_routes,
            resource_keys: grpc_routes,
            route_units_cache,
            listener_count,
        }
    }
}

/// Build DomainGrpcRouteRules from a per-listener units map.
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
        resource_keys,
        route_rules_list,
        match_engine,
    });

    let domain = DomainGrpcRouteRules::new();
    domain.grpc_routes.store(grpc_route_rules);
    domain
}

/// Get the resolved listeners for a GRPCRoute.
pub(crate) fn resolved_listeners_for_grpc_route(route: &GRPCRoute) -> &[ListenerKey] {
    route.spec.resolved_listeners.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GrpcRouteManagerStats {
    pub grpc_routes: usize,
    pub resource_keys: usize,
    pub route_units_cache: usize,
    pub listener_count: usize,
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
