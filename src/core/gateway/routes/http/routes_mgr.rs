use crate::core::common::matcher::host_match::radix_match::RadixHostMatchEngine;
use crate::core::gateway::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::core::gateway::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::core::gateway::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::core::gateway::routes::http::match_unit::RouteMatchResult;
use crate::core::gateway::routes::http::HttpRouteRuleUnit;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::ctx::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::HTTPRoute;
use crate::types::{HTTPBackendRef, HTTPRouteRule};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

type DomainStr = String;

pub struct RouteRules {
    /// All resource keys (HTTPRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub(crate) resource_keys: RwLock<HashSet<String>>,

    /// Exact and prefix match routes (handled by radix tree)
    /// Stored as Arc to avoid cloning when returning from match_route
    pub(crate) route_rules_list: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
    /// Match engine for exact/prefix routes. None if there are no normal routes (only regex routes)
    pub(crate) match_engine: Option<Arc<RadixRouteMatchEngine>>,

    /// Regex match routes (handled separately)
    /// Uses HttpRouteRuleUnit with path_regex field set
    /// Stored as Arc to avoid cloning when returning from match_route
    /// Kept for backward compatibility and debugging, but matching uses regex_routes_engine
    pub(crate) regex_routes: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
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
    pub fn select_backend(rule: &Arc<HTTPRouteRule>) -> Result<HTTPBackendRef, EdError> {
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

        // Note: BackendTLSPolicy query is performed in pg_upstream_peer.rs
        // where route namespace is available for proper namespace inheritance

        Ok(backend_ref)
    }

    /// Find a specific backend_ref by name from the route rule's backend_refs.
    ///
    /// Used by DynamicInternalUpstream plugin to bypass weighted selection and
    /// target a specific backend.
    ///
    /// Security: The target backend_ref must exist in the route's backend_refs
    /// list, which was already validated at route configuration time
    /// (including ReferenceGrant for cross-namespace references).
    ///
    /// Note: This function intentionally does NOT filter by weight.
    /// A backend_ref with weight=0 can still be targeted via DynamicInternalUpstream,
    /// enabling "hidden backend" patterns for debugging/testing.
    pub fn find_backend_by_name(
        rule: &Arc<HTTPRouteRule>,
        target_name: &str,
        target_namespace: Option<&str>,
        route_namespace: &str,
    ) -> Result<HTTPBackendRef, EdError> {
        let backend_refs = rule
            .backend_refs
            .as_ref()
            .filter(|refs| !refs.is_empty())
            .ok_or(EdError::BackendNotFound())?;

        for br in backend_refs {
            // Match by name
            if br.name != target_name {
                continue;
            }

            // If namespace is specified in jump target, verify it matches
            if let Some(ns) = target_namespace {
                let br_ns = br.namespace.as_deref().unwrap_or(route_namespace);
                if br_ns != ns {
                    continue;
                }
            }

            // Check cross-namespace reference denial
            if let Some(ref denied) = br.ref_denied {
                return Err(EdError::RefDenied {
                    target_namespace: denied.target_namespace.clone(),
                    target_name: denied.target_name.clone(),
                    reason: denied
                        .reason
                        .clone()
                        .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
                });
            }

            return Ok(br.clone());
        }

        Err(EdError::BackendNotFound())
    }

    /// Match a route using the match engines.
    /// Try match in order: regex → radix (exact + prefix).
    /// Returns `RouteMatchResult` on success (route + matched gateway context).
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<RouteMatchResult, EdError> {
        if let Some(ref regex_engine) = self.regex_routes_engine {
            if let Some(result) = regex_engine.match_route(session, ctx, gateway_infos)? {
                return Ok(result);
            }
        }

        if let Some(ref match_engine) = self.match_engine {
            return match_engine.match_route(session, ctx, gateway_infos);
        }

        Err(EdError::RouteNotFound())
    }
}

pub struct DomainRouteRules {
    /// Exact domain matching (e.g., "example.com")
    /// Uses HashMap for O(1) lookup performance
    pub(crate) exact_domain_map: ArcSwap<HashMap<DomainStr, Arc<RouteRules>>>,

    /// Wildcard domain matching (e.g., "*.example.com")
    /// Uses RadixHostMatchEngine for wildcard support
    /// None if no wildcard domains are configured
    pub(crate) wildcard_engine: ArcSwap<Option<RadixHostMatchEngine<RouteRules>>>,

    /// Catch-all routes from HTTPRoutes with no spec.hostnames.
    /// Stored separately from exact_domain_map to avoid mixing fallback
    /// semantics with exact-match semantics (previously used "*" sentinel key).
    pub(crate) catch_all_routes: ArcSwap<Option<Arc<RouteRules>>>,
}

impl DomainRouteRules {
    pub fn new() -> Self {
        Self {
            exact_domain_map: ArcSwap::from_pointee(HashMap::new()),
            wildcard_engine: ArcSwap::from_pointee(None),
            catch_all_routes: ArcSwap::from_pointee(None),
        }
    }

    /// Match a route for the given hostname and session against the global route table.
    ///
    /// Matching priority (per Gateway API spec):
    /// 1. Exact domain match (HashMap lookup - O(1))
    /// 2. Wildcard domain match (RadixHostMatchEngine - O(log n))
    /// 3. Catch-all ("*") domain match
    ///
    /// Gateway/listener validation happens inside `deep_match` for each candidate route.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `gateway_infos`: All gateway/listener contexts available on this listener
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<RouteMatchResult, EdError> {
        let hostname = &ctx.request_info.hostname;

        let exact_map = self.exact_domain_map.load();
        let wildcard_engine = self.wildcard_engine.load();

        if let Some(route_rules) = exact_map.get(&hostname.to_lowercase()) {
            return route_rules.match_route(session, ctx, gateway_infos);
        }

        // Step 2: Try wildcard domain match (O(log n))
        if let Some(ref engine) = wildcard_engine.as_ref() {
            if let Some(route_rules) = engine.match_host(hostname) {
                return route_rules.match_route(session, ctx, gateway_infos);
            }
        }

        // Step 3: Try catch-all routes (from HTTPRoutes with no spec.hostnames)
        if let Some(ref catch_all) = **self.catch_all_routes.load() {
            return catch_all.match_route(session, ctx, gateway_infos);
        }

        Err(EdError::RouteNotFound())
    }
}

// ============================================================================
// Per-port HTTP route manager (mirrors TcpPortRouteManager pattern)
// ============================================================================

/// Per-port HTTP route manager.
///
/// Each instance owns an atomically-swappable `DomainRouteRules` snapshot.
/// `pg_request_filter` loads the per-port table via `load_route_table()`
/// for lock-free lookups.
pub struct HttpPortRouteManager {
    route_table: ArcSwap<DomainRouteRules>,
}

impl Default for HttpPortRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpPortRouteManager {
    pub fn new() -> Self {
        Self {
            route_table: ArcSwap::from_pointee(DomainRouteRules::new()),
        }
    }

    /// Load the current route table snapshot (hot path, per-request).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<DomainRouteRules>> {
        self.route_table.load()
    }

    /// Replace the route table with a new snapshot.
    pub fn store_route_table(&self, table: DomainRouteRules) {
        self.route_table.store(Arc::new(table));
    }
}

// ============================================================================
// Global HTTP route managers (mirrors GlobalTcpRouteManagers pattern)
// ============================================================================

/// Global wrapper managing `port -> Arc<HttpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> HTTPRoute`) and
/// implements `ConfHandler<HTTPRoute>`. On every change it rebuilds
/// per-port managers by resolving each route's target ports from its
/// `spec.resolved_ports`.
pub struct GlobalHttpRouteManagers {
    /// Canonical route store: resource_key -> HTTPRoute
    pub(crate) route_cache: DashMap<String, HTTPRoute>,

    /// port -> per-port manager (stable Arc held by request filter)
    pub(crate) by_port: DashMap<u16, Arc<HttpPortRouteManager>>,
}

impl Default for GlobalHttpRouteManagers {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalHttpRouteManagers {
    pub fn new() -> Self {
        Self {
            route_cache: DashMap::new(),
            by_port: DashMap::new(),
        }
    }

    /// Get or create a per-port `HttpPortRouteManager`.
    ///
    /// The returned `Arc` is stable; route updates only swap the inner
    /// `ArcSwap<DomainRouteRules>`.
    pub fn get_or_create_port_manager(&self, port: u16) -> Arc<HttpPortRouteManager> {
        self.by_port
            .entry(port)
            .or_insert_with(|| Arc::new(HttpPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Rebuild all per-port managers from the current route cache.
    pub fn rebuild_all_port_managers(&self) {
        // Pre-create port managers for all known ports:
        // 1. From parentRef.port fields on routes
        // 2. From PortGatewayInfoStore (populated by gateway startup / Gateway config handler)
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

        let port_buckets = self.bucket_routes_by_port();

        let mut rebuilt_ports = 0u32;

        for (port, routes) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            let table =
                crate::core::gateway::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(routes);
            manager.store_route_table(table);
            rebuilt_ports += 1;
        }

        // Remove stale port entries: ports that have no routes AND are not
        // registered in PortGatewayInfoStore (i.e. no Gateway listener on that port).
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
        // Clear route tables for ports that still exist (active listener) but have no routes
        for entry in self.by_port.iter() {
            let port = *entry.key();
            if !port_buckets.contains_key(&port) {
                entry.value().store_route_table(DomainRouteRules::new());
            }
        }

        let total_routes: usize = port_buckets.values().map(|r| r.len()).sum();
        tracing::info!(
            component = "global_http_route_managers",
            ports = port_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_ports,
            removed_stale_ports = stale_ports.len(),
            "Rebuilt all per-port HTTP route managers"
        );
    }

    /// Rebuild only the per-port managers for the given set of affected ports.
    pub fn rebuild_affected_port_managers(&self, affected_ports: &HashSet<u16>) {
        if affected_ports.is_empty() {
            return;
        }

        let mut port_buckets: HashMap<u16, HashMap<String, HTTPRoute>> = HashMap::new();
        for &port in affected_ports {
            port_buckets.insert(port, HashMap::new());
        }

        for entry in self.route_cache.iter() {
            let route = entry.value();
            let ports = resolved_ports_for_route(route);
            let effective_ports: Vec<u16> = if !ports.is_empty() {
                ports.to_vec()
            } else {
                let fb = extract_ports_from_parent_refs(route);
                if fb.is_empty() {
                    affected_ports.iter().copied().collect()
                } else {
                    fb
                }
            };
            for port in effective_ports {
                if let Some(bucket) = port_buckets.get_mut(&port) {
                    bucket.insert(entry.key().clone(), entry.value().clone());
                }
            }
        }

        let pgis = crate::core::gateway::runtime::store::get_port_gateway_info_store();
        let active_ports: HashSet<u16> = pgis.all_ports().into_iter().collect();
        let mut removed_stale = 0usize;
        for (port, routes) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            let table =
                crate::core::gateway::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(routes);
            manager.store_route_table(table);
            if routes.is_empty() && !active_ports.contains(port) {
                self.by_port.remove(port);
                removed_stale += 1;
            }
        }

        tracing::info!(
            component = "global_http_route_managers",
            affected = affected_ports.len(),
            removed_stale_ports = removed_stale,
            "Rebuilt affected per-port HTTP route managers"
        );
    }

    /// Bucket all cached routes by their resolved ports.
    ///
    /// When a route has no `resolved_ports` (e.g. controller hasn't resolved yet,
    /// or legacy config), it falls back to ports extracted directly from
    /// `parentRef.port`. If still empty, the route is distributed to ALL known
    /// ports so it remains reachable (backward-compatible with the old global model).
    fn bucket_routes_by_port(&self) -> HashMap<u16, HashMap<String, HTTPRoute>> {
        let mut port_buckets: HashMap<u16, HashMap<String, HTTPRoute>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let ports = resolved_ports_for_route(&route);

            if !ports.is_empty() {
                for &port in ports {
                    port_buckets
                        .entry(port)
                        .or_default()
                        .insert(route_key.clone(), route.clone());
                }
            } else {
                let fallback_ports = extract_ports_from_parent_refs(&route);
                if !fallback_ports.is_empty() {
                    for port in &fallback_ports {
                        port_buckets
                            .entry(*port)
                            .or_default()
                            .insert(route_key.clone(), route.clone());
                    }
                } else {
                    for existing in self.by_port.iter() {
                        port_buckets
                            .entry(*existing.key())
                            .or_default()
                            .insert(route_key.clone(), route.clone());
                    }
                }
            }
        }

        port_buckets
    }

    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> HttpRouteManagerStats {
        let route_cache = self.route_cache.len();
        let port_count = self.by_port.len();

        let mut total_exact_domains = 0usize;
        let mut total_wildcard_domains = 0usize;
        let mut total_catch_all = false;

        for entry in self.by_port.iter() {
            let table = entry.value().load_route_table();
            let exact_map = table.exact_domain_map.load();
            total_exact_domains += exact_map.len();
            let wc = table.wildcard_engine.load();
            total_wildcard_domains += wc.as_ref().as_ref().map_or(0, |e| e.host_count());
            if table.catch_all_routes.load().is_some() {
                total_catch_all = true;
            }
        }

        HttpRouteManagerStats {
            exact_domains: total_exact_domains,
            wildcard_domains: total_wildcard_domains,
            has_catch_all: total_catch_all,
            http_routes: route_cache,
            port_count,
        }
    }
}

/// Get the resolved listener ports for an HTTPRoute.
///
/// Uses `spec.resolved_ports` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.port.
pub(crate) fn resolved_ports_for_route(route: &HTTPRoute) -> &[u16] {
    route.spec.resolved_ports.as_deref().unwrap_or_default()
}

/// Extract ports directly from parentRef.port fields.
/// Used as fallback when `resolved_ports` is not set.
fn extract_ports_from_parent_refs(route: &HTTPRoute) -> Vec<u16> {
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
pub struct HttpRouteManagerStats {
    pub exact_domains: usize,
    pub wildcard_domains: usize,
    pub has_catch_all: bool,
    pub http_routes: usize,
    pub port_count: usize,
}

static GLOBAL_HTTP_ROUTE_MANAGERS: OnceLock<GlobalHttpRouteManagers> = OnceLock::new();

pub fn get_global_http_route_managers() -> &'static GlobalHttpRouteManagers {
    GLOBAL_HTTP_ROUTE_MANAGERS.get_or_init(GlobalHttpRouteManagers::new)
}

// Legacy aliases for backward compatibility during migration
pub type RouteManager = GlobalHttpRouteManagers;

pub fn get_global_route_manager() -> &'static GlobalHttpRouteManagers {
    get_global_http_route_managers()
}

impl GlobalHttpRouteManagers {
    /// Test helper: get the per-port manager for port 80.
    /// The returned Arc is stable; inner route table updates are
    /// visible through `load_route_table()`.
    #[cfg(test)]
    pub fn get_global_routes(&self) -> Arc<HttpPortRouteManager> {
        self.get_or_create_port_manager(80)
    }
}
