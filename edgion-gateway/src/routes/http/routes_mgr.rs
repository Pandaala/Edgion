use crate::ctx::EdgionHttpContext;
use crate::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::routes::http::match_unit::CompiledBackend;
use crate::routes::http::match_unit::RouteMatchResult;
use crate::routes::http::HttpRouteRuleUnit;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use edgion_resources::err::EdError;
use edgion_resources::matcher::host_match::radix_match::RadixHostMatchEngine;
use edgion_resources::HTTPRoute;
use edgion_resources::ListenerKey;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

type DomainStr = String;

pub struct RouteRules {
    /// All resource keys (HTTPRoute) that apply to this hostname
    /// Format: "namespace/name"
    pub(crate) resource_keys: HashSet<String>,

    /// Exact and prefix match routes (handled by radix tree)
    /// Stored as Arc to avoid cloning when returning from match_route
    pub(crate) route_rules_list: Vec<Arc<HttpRouteRuleUnit>>,
    /// Match engine for exact/prefix routes. None if there are no normal routes (only regex routes)
    pub(crate) match_engine: Option<Arc<RadixRouteMatchEngine>>,

    /// Regex match routes (handled separately)
    /// Uses HttpRouteRuleUnit with path_regex field set
    /// Stored as Arc to avoid cloning when returning from match_route
    /// Kept for debugging; matching uses regex_routes_engine
    pub(crate) regex_routes: Vec<Arc<HttpRouteRuleUnit>>,
    /// Regex routes engine for lock-free matching. None if there are no regex routes
    pub(crate) regex_routes_engine: Option<Arc<RegexRoutesEngine>>,
}

impl Clone for RouteRules {
    fn clone(&self) -> Self {
        Self {
            resource_keys: self.resource_keys.clone(),
            route_rules_list: self.route_rules_list.clone(),
            match_engine: self.match_engine.clone(),
            regex_routes: self.regex_routes.clone(),
            regex_routes_engine: self.regex_routes_engine.clone(),
        }
    }
}

impl RouteRules {
    /// Select backend from the matched route rule
    pub fn select_backend(unit: &Arc<HttpRouteRuleUnit>) -> Result<CompiledBackend, EdError> {
        // The unit's `backend_finder` is eagerly initialized at unit construction
        // (`build_backend_selector`), so no lazy init is needed here.
        let backend = unit.backend_finder.select().map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

        // Check if this backend reference is denied (no matching ReferenceGrant)
        if let Some(ref denied) = backend.schema.ref_denied {
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

        Ok(backend)
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
        unit: &Arc<HttpRouteRuleUnit>,
        target_name: &str,
        target_namespace: Option<&str>,
        route_namespace: &str,
    ) -> Result<CompiledBackend, EdError> {
        if unit.backends.is_empty() {
            return Err(EdError::BackendNotFound());
        }

        for cb in unit.backends.iter() {
            let br = &cb.schema;
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

            return Ok(cb.clone());
        }

        Err(EdError::BackendNotFound())
    }

    /// Match a route using the match engines.
    ///
    /// Per Gateway API spec, path match priority is: Exact > PathPrefix > RegularExpression.
    /// Radix engine handles Exact and PathPrefix; regex engine handles RegularExpression.
    /// Try radix first, fall back to regex only when radix finds no match.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `index`: Pre-built gateway/listener index for this port
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<RouteMatchResult, EdError> {
        if let Some(ref match_engine) = self.match_engine {
            match match_engine.match_route(session, ctx, index) {
                Ok(result) => return Ok(result),
                Err(_) => {}
            }
        }

        if let Some(ref regex_engine) = self.regex_routes_engine {
            if let Some(result) = regex_engine.match_route(session, ctx, index)? {
                return Ok(result);
            }
        }

        Err(EdError::RouteNotFound())
    }
}

pub struct DomainRouteRules {
    /// Exact domain matching (e.g., "example.com")
    /// Uses HashMap for O(1) lookup performance.
    /// Plain `Arc` — atomic swap granularity is provided by the outer
    /// `ArcSwap<DomainRouteRules>` in `HttpPortRouteManager`, so per-request
    /// loads do not need an extra atomic indirection here.
    pub(crate) exact_domain_map: Arc<HashMap<DomainStr, Arc<RouteRules>>>,

    /// Wildcard domain matching (e.g., "*.example.com")
    /// Uses RadixHostMatchEngine for wildcard support
    /// None if no wildcard domains are configured
    pub(crate) wildcard_engine: Option<Arc<RadixHostMatchEngine<RouteRules>>>,

    /// Catch-all routes from HTTPRoutes with no spec.hostnames.
    /// Stored separately from exact_domain_map to avoid mixing fallback
    /// semantics with exact-match semantics.
    pub(crate) catch_all_routes: Option<Arc<RouteRules>>,
}

impl DomainRouteRules {
    pub fn new() -> Self {
        Self {
            exact_domain_map: Arc::new(HashMap::new()),
            wildcard_engine: None,
            catch_all_routes: None,
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
    /// - `index`: Pre-built gateway/listener index for this port
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<RouteMatchResult, EdError> {
        // ctx.request_info.hostname is a NormalizedHostname — already lowercased,
        // port-stripped, FQDN-dot-removed. No further normalization needed.
        let hostname: &str = &ctx.request_info.hostname;

        if let Some(route_rules) = self.exact_domain_map.get(hostname) {
            return route_rules.match_route(session, ctx, index);
        }

        // Step 2: Try wildcard domain match (O(log n))
        if let Some(engine) = self.wildcard_engine.as_ref() {
            if let Some(route_rules) = engine.match_host(hostname) {
                return route_rules.match_route(session, ctx, index);
            }
        }

        // Step 3: Try catch-all routes (from HTTPRoutes with no spec.hostnames)
        if let Some(catch_all) = self.catch_all_routes.as_ref() {
            return catch_all.match_route(session, ctx, index);
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

/// Global wrapper managing `ListenerKey -> Arc<HttpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> HTTPRoute`) and
/// implements `ConfHandler<HTTPRoute>`. On every change it rebuilds
/// per-listener managers by resolving each route's target listeners from its
/// `spec.resolved_listeners`.
pub struct GlobalHttpRouteManagers {
    /// Canonical route store: resource_key -> HTTPRoute
    pub(crate) route_cache: DashMap<String, HTTPRoute>,

    /// listener -> per-listener manager (stable Arc held by request filter)
    pub(crate) by_listener: DashMap<ListenerKey, Arc<HttpPortRouteManager>>,

    /// Serializes the read-snapshot-then-rewrite sequences that mutate
    /// `route_cache` + `by_listener`. Two independent dispatch tasks reach this
    /// manager concurrently: the HTTPRoute cache task (`full_set`/`partial_update`)
    /// and the Gateway cache task (`rebuild_all_listener_managers` via
    /// `runtime::handler` on listener changes). Both run under their own per-kind
    /// `CacheData` lock, which gives no mutual exclusion against each other, so
    /// without this lock a Gateway-driven `rebuild_all` can read a pre-deletion
    /// `route_cache` snapshot and write a stale route table after a concurrent
    /// `partial_update` already cleared it (orphan listener-bucket leak).
    /// Held only across synchronous work (no `.await` in the critical region).
    rebuild_lock: parking_lot::Mutex<()>,
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
            by_listener: DashMap::new(),
            rebuild_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Acquire the rebuild lock. Callers that mutate `route_cache`/`by_listener`
    /// (`full_set`, `partial_update`, the public `rebuild_all_listener_managers`)
    /// hold this for their whole body so the three paths are mutually exclusive.
    /// NOT reentrant — internal callers must use the `*_locked` variants.
    pub(crate) fn lock_rebuild(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.rebuild_lock.lock()
    }

    /// Get or create a per-listener `HttpPortRouteManager`.
    ///
    /// The returned `Arc` is stable; route updates only swap the inner
    /// `ArcSwap<DomainRouteRules>`.
    pub fn get_or_create_listener_manager(&self, key: &ListenerKey) -> Arc<HttpPortRouteManager> {
        self.by_listener
            .entry(key.clone())
            .or_insert_with(|| Arc::new(HttpPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Rebuild all per-listener managers from the current route cache.
    /// Public entry point (called from `runtime::handler` on listener changes).
    /// Acquires the rebuild lock so it cannot race a concurrent HTTPRoute
    /// `full_set`/`partial_update` on this manager. `full_set` calls the
    /// unlocked `*_locked` variant instead (it already holds the lock).
    pub fn rebuild_all_listener_managers(&self) {
        let _guard = self.rebuild_lock.lock();
        self.rebuild_all_listener_managers_locked();
    }

    /// Rebuild body. The caller MUST hold `rebuild_lock`.
    pub(crate) fn rebuild_all_listener_managers_locked(&self) {
        // Pre-create listener managers for all known listeners from ListenerGatewayInfoStore
        let pgis = crate::runtime::store::get_listener_gateway_info_store();
        // Physical listener keys the request path reads from; routes whose
        // resolved_listeners are not among these are unreachable.
        tracing::debug!(
            component = "global_http_route_managers",
            listener_keys = ?pgis.all_listener_keys(),
            "Per-listener route manager rebuild: known listener keys"
        );
        for key in pgis.all_listener_keys() {
            self.get_or_create_listener_manager(&key);
        }

        let listener_buckets = self.bucket_routes_by_listener();

        let mut rebuilt_listeners = 0u32;

        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let table = crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(routes);
            manager.store_route_table(table);
            rebuilt_listeners += 1;
        }

        // Remove stale listener entries: listeners that have no routes AND are not
        // registered in ListenerGatewayInfoStore (i.e. no Gateway listener for that key).
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
        // Clear route tables for listeners that still exist (active listener) but have no routes
        for entry in self.by_listener.iter() {
            let key = entry.key().clone();
            if !listener_buckets.contains_key(&key) {
                entry.value().store_route_table(DomainRouteRules::new());
            }
        }

        let total_routes: usize = listener_buckets.values().map(|r| r.len()).sum();
        tracing::info!(
            component = "global_http_route_managers",
            listeners = listener_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_listeners,
            removed_stale_listeners = stale_listeners.len(),
            "Rebuilt all per-listener HTTP route managers"
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected listeners.
    ///
    /// The caller MUST hold `rebuild_lock` — this is only invoked from
    /// `partial_update`, which acquires the lock for its whole body.
    pub fn rebuild_affected_listener_managers(&self, affected_listeners: &HashSet<ListenerKey>) {
        if affected_listeners.is_empty() {
            return;
        }

        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, HTTPRoute>> = HashMap::new();
        for key in affected_listeners {
            listener_buckets.insert(key.clone(), HashMap::new());
        }

        // Single pass over route_cache: bucket affected routes into listener_buckets
        // and collect all referenced listener keys into route_referenced (used by
        // the orphan sweep below).
        let mut route_referenced: HashSet<ListenerKey> = HashSet::new();
        for entry in self.route_cache.iter() {
            let route = entry.value();
            let listeners = resolved_listeners_for_route(route);
            for key in listeners {
                if let Some(bucket) = listener_buckets.get_mut(key) {
                    bucket.insert(entry.key().clone(), entry.value().clone());
                }
                route_referenced.insert(key.clone());
            }
        }

        let pgis = crate::runtime::store::get_listener_gateway_info_store();
        let active_listeners: HashSet<ListenerKey> = pgis.all_listener_keys().into_iter().collect();
        let mut removed_stale = 0usize;
        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let table = crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(routes);
            manager.store_route_table(table);
            if routes.is_empty() && !active_listeners.contains(key) {
                self.by_listener.remove(key);
                removed_stale += 1;
            }
        }

        // Sweep orphan entries: a partial update only touches affected listeners,
        // but earlier rebuild_all calls may have pre-created by_listener entries
        // for listeners that no longer appear in PGIS and are not referenced by
        // any current route. Without this sweep, those entries (and their stale
        // route tables) leak indefinitely — they are never the target of a
        // subsequent partial update.
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
            component = "global_http_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener HTTP route managers"
        );
    }

    /// Bucket all cached routes by their resolved listeners.
    fn bucket_routes_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, HTTPRoute>> {
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, HTTPRoute>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let listeners = resolved_listeners_for_route(&route);

            // Listener keys this route is bucketed under, from the
            // controller-computed spec.resolved_listeners.
            tracing::debug!(
                component = "global_http_route_managers",
                route = %route_key,
                resolved_listeners = ?listeners,
                "Bucketing HTTPRoute by resolved_listeners"
            );

            if listeners.is_empty() {
                tracing::warn!(route = %route_key, "HTTPRoute has no resolved_listeners, skipping");
            } else {
                for key in listeners {
                    listener_buckets
                        .entry(key.clone())
                        .or_default()
                        .insert(route_key.clone(), route.clone());
                }
            }
        }

        listener_buckets
    }

    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> HttpRouteManagerStats {
        let route_cache = self.route_cache.len();
        let listener_count = self.by_listener.len();

        let mut total_exact_domains = 0usize;
        let mut total_wildcard_domains = 0usize;
        let mut total_catch_all = false;

        for entry in self.by_listener.iter() {
            let table = entry.value().load_route_table();
            total_exact_domains += table.exact_domain_map.len();
            total_wildcard_domains += table.wildcard_engine.as_ref().map_or(0, |e| e.host_count());
            if table.catch_all_routes.is_some() {
                total_catch_all = true;
            }
        }

        HttpRouteManagerStats {
            exact_domains: total_exact_domains,
            wildcard_domains: total_wildcard_domains,
            has_catch_all: total_catch_all,
            http_routes: route_cache,
            listener_count,
        }
    }
}

/// Get the resolved listeners for an HTTPRoute.
///
/// Uses `spec.resolved_listeners` which is pre-computed by the controller
/// from parentRef → Gateway listener matching.
pub(crate) fn resolved_listeners_for_route(route: &HTTPRoute) -> &[ListenerKey] {
    route.spec.resolved_listeners.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HttpRouteManagerStats {
    pub exact_domains: usize,
    pub wildcard_domains: usize,
    pub has_catch_all: bool,
    pub http_routes: usize,
    pub listener_count: usize,
}

static GLOBAL_HTTP_ROUTE_MANAGERS: OnceLock<GlobalHttpRouteManagers> = OnceLock::new();

pub fn get_global_http_route_managers() -> &'static GlobalHttpRouteManagers {
    GLOBAL_HTTP_ROUTE_MANAGERS.get_or_init(GlobalHttpRouteManagers::new)
}

// Legacy type aliases for backward compatibility.
pub type RouteManager = GlobalHttpRouteManagers;

pub fn get_global_route_manager() -> &'static GlobalHttpRouteManagers {
    get_global_http_route_managers()
}

impl GlobalHttpRouteManagers {
    /// Test helper: get the per-listener manager for the default HTTP listener (port 80).
    /// The returned Arc is stable; inner route table updates are
    /// visible through `load_route_table()`.
    #[cfg(test)]
    pub fn get_global_routes(&self) -> Arc<HttpPortRouteManager> {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 80);
        self.get_or_create_listener_manager(&ListenerKey::new(addr, "HTTP"))
    }
}
