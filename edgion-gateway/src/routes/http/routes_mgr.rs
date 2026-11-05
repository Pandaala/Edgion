use super::conf_handler_impl::{patch_domain_route_rules, HttpRouteChanges};
use super::delegation_build::{is_delegation_child, HttpDelegationIndex};
use crate::backends::LogicalBackendHandle;
use crate::ctx::EdgionHttpContext;
use crate::lb::{ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
use crate::routes::delegation::descent::{ChildHit, DescentState};
use crate::routes::delegation::DelegationSlotTable;
use crate::routes::http::match_engine::radix_route_match::RadixRouteMatchEngine;
use crate::routes::http::match_engine::regex_routes_engine::RegexRoutesEngine;
use crate::routes::http::match_unit::RouteMatchResult;
use crate::routes::http::HttpRouteRuleUnit;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use edgion_algo::matcher::radix_hostname::RadixHostnameMatcher;
use edgion_algo::matcher::radix_hostname::RadixHostnameQuery;
use edgion_resources::err::EdError;
use edgion_resources::HTTPRoute;
use edgion_resources::ListenerKey;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, Weak};

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

/// The single denial predicate shared by both backend matchers.
///
/// `select_backend` asks it about the slot weighted selection landed on;
/// `find_backend_by_name` asks it about the slot a named jump addressed. Keeping
/// one construction site means a change to the denial shape cannot land in only
/// one of them and silently diverge.
#[inline]
fn ref_denied_error(schema: &edgion_resources::HTTPBackendRef) -> Option<EdError> {
    schema.ref_denied.as_ref().map(|denied| EdError::RefDenied {
        target_namespace: denied.target_namespace.clone(),
        target_name: denied.target_name.clone(),
        reason: denied
            .reason
            .clone()
            .unwrap_or_else(|| "NoMatchingReferenceGrant".to_string()),
    })
}

impl RouteRules {
    /// Select backend from the matched route rule
    pub fn select_backend(unit: &Arc<HttpRouteRuleUnit>) -> Result<LogicalBackendHandle, EdError> {
        // The unit's `backend_finder` is eagerly initialized at unit construction
        // (`build_backend_selector`), so no lazy init is needed here.
        let index = unit.backend_finder.select().map_err(|err_code| match err_code {
            ERR_NO_BACKEND_REFS => EdError::BackendNotFound(),
            ERR_INCONSISTENT_WEIGHT => EdError::InconsistentWeight(),
            _ => EdError::BackendNotFound(),
        })?;

        // Check if this backend reference is denied (no matching ReferenceGrant)
        let backend = unit.backends.get(index).ok_or_else(EdError::BackendNotFound)?;
        if let Some(err) = ref_denied_error(backend.schema()) {
            return Err(err);
        }

        // Note: BackendTLSPolicy query is performed in pg_upstream_peer.rs
        // where route namespace is available for proper namespace inheritance

        LogicalBackendHandle::new(Arc::clone(&unit.backends), index).ok_or_else(EdError::BackendNotFound)
    }

    /// Find a specific backend_ref by name from the route rule's backend_refs.
    ///
    /// Shares one name matcher with `validate_backend_by_name`, which plugins
    /// use before publishing a jump. This owned form pins the generation when
    /// `pick_backend_ref` consumes the jump instead of weighted selection.
    ///
    /// Security: this is the enforcement point for the Controller's
    /// ReferenceGrant decision on a named target — a matching backend_ref
    /// carrying `ref_denied` yields `EdError::RefDenied`, never a handle.
    /// Callers must not collapse that into the `BackendNotFound` miss, which is
    /// the only outcome allowed to fall through to weighted selection.
    ///
    /// Note: This function intentionally does NOT filter by weight.
    /// A backend_ref with weight=0 can still be targeted via DynamicInternalUpstream,
    /// enabling "hidden backend" patterns for debugging/testing.
    pub fn find_backend_by_name(
        unit: &Arc<HttpRouteRuleUnit>,
        target_name: &str,
        target_namespace: Option<&str>,
    ) -> Result<LogicalBackendHandle, EdError> {
        let index = Self::find_backend_index_by_name(unit, target_name, target_namespace)?;
        LogicalBackendHandle::new(Arc::clone(&unit.backends), index).ok_or_else(EdError::BackendNotFound)
    }

    /// Validate a named jump without acquiring an owned backend generation.
    pub fn validate_backend_by_name(
        unit: &HttpRouteRuleUnit,
        target_name: &str,
        target_namespace: Option<&str>,
    ) -> Result<(), EdError> {
        Self::find_backend_index_by_name(unit, target_name, target_namespace).map(|_| ())
    }

    /// Canonical named-target matching and admission for validation and selection.
    fn find_backend_index_by_name(
        unit: &HttpRouteRuleUnit,
        target_name: &str,
        target_namespace: Option<&str>,
    ) -> Result<usize, EdError> {
        if unit.backends.is_empty() {
            return Err(EdError::BackendNotFound());
        }

        for (index, cb) in unit.backends.iter().enumerate() {
            let crate::backends::runtime::CompiledBackendTarget::Service(target) = cb.target() else {
                continue;
            };
            if target.name() != target_name {
                continue;
            }
            if target_namespace.is_some_and(|namespace| namespace != target.namespace()) {
                continue;
            }
            let br = cb.schema();

            // Check cross-namespace reference denial
            if let Some(err) = ref_denied_error(br) {
                return Err(err);
            }

            return LogicalBackendHandle::is_valid_slot(&unit.backends, index)
                .then_some(index)
                .ok_or_else(EdError::BackendNotFound);
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
    /// - `descent`: per-request delegation state, created lazily on the first
    ///   delegating hit and shared across hostname groups (`None` otherwise)
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        descent: &mut Option<DescentState>,
    ) -> Result<RouteMatchResult, EdError> {
        if let Some(ref match_engine) = self.match_engine {
            match match_engine.match_route(session, ctx, index, descent) {
                Ok(result) => return Ok(result),
                Err(EdError::RouteNotFound()) => {}
                Err(error) => return Err(error),
            }
        }

        if let Some(ref regex_engine) = self.regex_routes_engine {
            if let Some(result) = regex_engine.match_route(session, ctx, index)? {
                return Ok(result);
            }
        }

        Err(EdError::RouteNotFound())
    }

    /// Child-level match of a delegation subtree: radix candidates by their
    /// precomputed ranks, then the regex fallback. Child units are checked with
    /// `deep_match_child` (no Gateway eligibility; decided at the root).
    pub(crate) fn match_child(
        &self,
        session: &pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        state: &mut DescentState,
    ) -> Result<Option<ChildHit>, EdError> {
        if let Some(ref match_engine) = self.match_engine {
            if let Some(hit) = match_engine.match_child(session, ctx, state)? {
                return Ok(Some(hit));
            }
        }
        if let Some(ref regex_engine) = self.regex_routes_engine {
            return regex_engine.match_child(session, ctx, state);
        }
        Ok(None)
    }
}

pub struct DomainRouteRules {
    /// Declared hostname groups, published with their path rules in this snapshot.
    pub(crate) hosts: RadixHostnameMatcher<RouteRules>,

    /// Catch-all routes from HTTPRoutes with no spec.hostnames.
    /// Stored separately from named patterns so only an omitted or empty
    /// authored list can opt into catch-all behavior.
    pub(crate) catch_all_routes: Option<Arc<RouteRules>>,

    /// Cold publication state used to force the next partial update through a
    /// complete Listener rebuild after hostname topology construction failed.
    pub(crate) hostname_build_failed: bool,
}

impl Default for DomainRouteRules {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainRouteRules {
    pub fn new() -> Self {
        Self {
            hosts: RadixHostnameMatcher::default(),
            catch_all_routes: None,
            hostname_build_failed: false,
        }
    }

    /// Match a route for the given hostname and session against the global route table.
    ///
    /// Matching priority (per Gateway API spec):
    /// 1. Exact hostname group
    /// 2. All wildcard groups, longest suffix first
    /// 3. Catch-all ("*") domain match
    ///
    /// Gateway/listener validation happens inside `deep_match` for each candidate route.
    ///
    /// # Parameters
    /// - `session`: The HTTP session
    /// - `ctx`: Request context containing hostname and other request info
    /// - `index`: Pre-built gateway/listener index for this port
    #[inline]
    pub fn match_route(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<RouteMatchResult, EdError> {
        self.match_route_with_descent(session, ctx, index, &mut None)
    }

    /// [`Self::match_route`] with caller-owned descent state, so one request
    /// shares a single visited set across hostname groups. `descent` stays
    /// `None` unless a delegating rule matched.
    pub fn match_route_with_descent(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        descent: &mut Option<DescentState>,
    ) -> Result<RouteMatchResult, EdError> {
        // ctx.request_info.hostname is a NormalizedHostname — already lowercased,
        // port-stripped, FQDN-dot-removed. No further normalization needed.
        let hostname: &str = &ctx.request_info.hostname;

        // A valid HTTP authority outside named hostname syntax (for example an
        // IP literal) may still use an eligible hostless Route and Listener.
        if !self.hosts.is_empty() {
            if let Ok(query) = RadixHostnameQuery::new(hostname) {
                for rules in self.hosts.matching_values(query) {
                    match rules.match_route(session, ctx, index, descent) {
                        Err(EdError::RouteNotFound()) => {}
                        result => return result,
                    }
                }
            }
        }

        // Step 3: Try catch-all routes (from HTTPRoutes with no spec.hostnames)
        if let Some(catch_all) = self.catch_all_routes.as_ref() {
            return catch_all.match_route(session, ctx, index, descent);
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
/// Cold publication captures the per-port table via `load_route_table()`
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

    /// Load a compiled table for cold rebuilding and inspection.
    /// Serving requests use their captured listener publication.
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
/// Owns the canonical route cache (`resource_key -> Arc<HTTPRoute>`) and
/// implements `ConfHandler<HTTPRoute>`. On every change it rebuilds
/// per-listener managers by resolving each route's target listeners from its
/// `spec.resolved_listeners`.
pub struct GlobalHttpRouteManagers {
    /// Canonical route store: resource_key -> Arc<HTTPRoute>
    pub(crate) route_cache: DashMap<String, Arc<HTTPRoute>>,

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

    /// HTTPRoute delegation slots. Delegating rule units hold their slot from
    /// compile time; child publications swap subtrees inside it. Touched only
    /// under `rebuild_lock`.
    pub(crate) delegation_slots: DelegationSlotTable<RouteRules>,

    /// Gateway-side child index (child -> accepted keys + compiled units, and
    /// key -> children). Touched only under `rebuild_lock`.
    pub(crate) delegation_index: parking_lot::Mutex<HttpDelegationIndex>,
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
            delegation_slots: DelegationSlotTable::new(),
            delegation_index: parking_lot::Mutex::new(HttpDelegationIndex::default()),
        }
    }

    /// Acquire the rebuild lock. Callers that mutate `route_cache`/`by_listener`
    /// (`full_set`, `partial_update`, the public `rebuild_all_listener_managers`)
    /// hold this for their whole body so the three paths are mutually exclusive.
    /// NOT reentrant — internal callers must use the `*_locked` variants.
    pub(crate) fn lock_rebuild(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.rebuild_lock.lock()
    }

    /// Capture table Arcs while the caller holds the family rebuild mutex.
    pub(crate) fn publication_tables(
        &self,
    ) -> HashMap<ListenerKey, (Weak<HttpPortRouteManager>, Arc<DomainRouteRules>)> {
        self.by_listener
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    (
                        Arc::downgrade(entry.value()),
                        Arc::clone(&entry.value().load_route_table()),
                    ),
                )
            })
            .collect()
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
        crate::plugins::publication::global_publisher().update_http_routes(
            HashMap::new(),
            &HashSet::new(),
            self.publication_tables(),
        );
    }

    /// Rebuild body. The caller MUST hold `rebuild_lock`.
    pub(crate) fn rebuild_all_listener_managers_locked(&self) {
        let active_listener_keys = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        let declared = crate::runtime::store::get_global_gateway_store()
            .read()
            .declared_http_listener_keys();
        self.rebuild_all_listener_managers_with_active(active_listener_keys, declared);
    }

    pub(crate) fn rebuild_all_listener_managers_with_active(
        &self,
        active_listener_keys: Vec<ListenerKey>,
        declared: HashSet<ListenerKey>,
    ) {
        // Pre-create listener managers for all known listeners from ListenerGatewayInfoStore
        // Physical listener keys the request path reads from; routes whose
        // resolved_listeners are not among these are unreachable.
        tracing::debug!(
            component = "global_http_route_managers",
            listener_keys = ?active_listener_keys,
            "Per-listener route manager rebuild: known listener keys"
        );
        for key in &active_listener_keys {
            self.get_or_create_listener_manager(key);
        }

        let mut listener_buckets = self.bucket_routes_by_listener();
        let eligible: HashSet<_> = active_listener_keys.into_iter().collect();
        listener_buckets.retain(|key, _| !declared.contains(key) || eligible.contains(key));

        let mut rebuilt_listeners = 0u32;

        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let table = crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes(
                routes,
                &manager.load_route_table(),
                &self.delegation_slots,
            );
            manager.store_route_table(table);
            rebuilt_listeners += 1;
        }

        // Remove stale listener entries: listeners that have no routes AND are not
        // registered in ListenerGatewayInfoStore (i.e. no Gateway listener for that key).
        let mut active_listeners = eligible;
        active_listeners.extend(declared);
        let stale_listeners: Vec<ListenerKey> = self
            .by_listener
            .iter()
            .filter(|e| !listener_buckets.contains_key(e.key()) && !active_listeners.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        let mut removed_stale = 0usize;
        for key in &stale_listeners {
            if let Some((_, manager)) = self.by_listener.remove(key) {
                manager.store_route_table(DomainRouteRules::new());
                removed_stale += 1;
            }
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
            removed_stale_listeners = removed_stale,
            "Rebuilt all per-listener HTTP route managers"
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected listeners.
    ///
    /// The caller MUST hold `rebuild_lock` — this is only invoked from
    /// `partial_update`, which acquires the lock for its whole body.
    pub(crate) fn rebuild_affected_listener_managers(
        &self,
        affected_listeners: &HashSet<ListenerKey>,
        changes: &HttpRouteChanges,
    ) {
        if affected_listeners.is_empty() {
            return;
        }
        let active_listener_keys = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        let declared = crate::runtime::store::get_global_gateway_store()
            .read()
            .declared_http_listener_keys();
        self.rebuild_affected_listener_managers_with_active(
            affected_listeners,
            active_listener_keys,
            changes,
            declared,
        );
    }

    pub(crate) fn rebuild_affected_listener_managers_with_active(
        &self,
        affected_listeners: &HashSet<ListenerKey>,
        active_listener_keys: Vec<ListenerKey>,
        changes: &HttpRouteChanges,
        declared: HashSet<ListenerKey>,
    ) {
        let eligible: HashSet<_> = active_listener_keys.into_iter().collect();
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<HTTPRoute>>> = HashMap::new();
        for key in affected_listeners {
            listener_buckets.insert(key.clone(), HashMap::new());
        }

        // Single pass over route_cache: bucket affected routes into listener_buckets
        // and collect all referenced listener keys into route_referenced (used by
        // the orphan sweep below).
        let mut route_referenced: HashSet<ListenerKey> = HashSet::new();
        for entry in self.route_cache.iter() {
            let route = entry.value();
            let listeners = resolved_listeners_for_route(route); // &Arc coerces to &HTTPRoute
            for key in listeners {
                if declared.contains(key) && !eligible.contains(key) {
                    continue;
                }
                if let Some(bucket) = listener_buckets.get_mut(key) {
                    bucket.insert(entry.key().clone(), entry.value().clone()); // Arc clone
                }
                route_referenced.insert(key.clone());
            }
        }

        let mut active_listeners = eligible;
        active_listeners.extend(declared);
        let mut removed_stale = 0usize;
        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            let previous = manager.load_route_table();
            let table = patch_domain_route_rules(routes, &previous, changes, &self.delegation_slots);
            manager.store_route_table(table);
            if routes.is_empty() && !active_listeners.contains(key) && self.by_listener.remove(key).is_some() {
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
            if let Some((_, manager)) = self.by_listener.remove(key) {
                manager.store_route_table(DomainRouteRules::new());
                removed_stale += 1;
            }
        }

        tracing::info!(
            component = "global_http_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener HTTP route managers"
        );
    }

    /// Bucket all cached routes by their resolved listeners. Delegation
    /// children are skipped silently: they have no listeners and are routed to
    /// the delegation slots instead.
    pub(crate) fn bucket_routes_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, Arc<HTTPRoute>>> {
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<HTTPRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone(); // Arc clone
            if is_delegation_child(&route) {
                continue;
            }
            let listeners = resolved_listeners_for_route(&route); // &Arc coerces to &HTTPRoute

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
                        .insert(route_key.clone(), route.clone()); // Arc clone
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
            for (pattern, _) in table.hosts.groups() {
                if pattern.is_wildcard() {
                    total_wildcard_domains += 1;
                } else {
                    total_exact_domains += 1;
                }
            }
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
