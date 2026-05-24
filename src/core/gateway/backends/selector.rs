use crate::core::gateway::backends::circuit_breaker::get_circuit_breaker_store;
use crate::core::gateway::backends::discovery::get_roundrobin_store;
use crate::core::gateway::backends::health;
use crate::core::gateway::backends::health::get_health_status_store;
use crate::core::gateway::lb::{compiled_selector, EdgionBackend, PolicyKind};
use crate::core::gateway::routes::HttpRouteRuleUnit;
use crate::types::edgion_status::EdgionStatus;
use crate::types::{ConsistentHashOn, ParsedLBPolicy};
use pingora_proxy::Session;
use std::sync::Arc;

/// Lazily hydrate `cell` from the EndpointSlice data layer when it is empty.
///
/// The control plane only pushes backends into a cell via
/// `update_affected_lbs` / `update_all_existing_lbs`, both of which iterate
/// `service_lbs` (legacy LB cache), not the data layer. Under the lazy-LB
/// design that legacy cache is empty until a request triggers
/// `get_or_create_with_provider`, so on a cold cell the very first
/// pre-request push never happens — the cell would stay empty forever and
/// every selection returns `not_found`.
///
/// We mirror the legacy `get_backends_for_service_key` path: read whatever
/// the EndpointSlice store currently has and seed the cell with it.
/// Subsequent partial updates run through `replace_backends` and stay
/// authoritative.
///
/// Returns `true` if the cell ended up non-empty after this call.
fn hydrate_cell_if_empty(cell: &compiled_selector::ServiceSelectorCell, service_key: &str) -> bool {
    cell.hydrate_if_empty(|| get_roundrobin_store().get_backends_for_service(service_key))
}

/// Health predicate for all self-impl selection algorithms.
///
/// Returns true if the service has no health check configured (default healthy),
/// or if the backend is marked healthy in the health status store.
#[inline]
pub(crate) fn is_backend_healthy(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    // 1. Active health check filter
    if health_store.has_service(service_key)
        && !health_store.is_healthy(service_key, health::check::backend_hash(backend))
    {
        return false;
    }
    // 2. Circuit breaker filter
    let cb_store = get_circuit_breaker_store();
    if cb_store.is_ejected(service_key, health::check::backend_hash(backend)) {
        crate::core::gateway::observe::metrics::record_cb_rejection(service_key);
        return false;
    }
    true
}

/// Map a parsed LB policy to the discriminator used by `ServiceSelectorCell`.
#[inline]
fn policy_to_kind(lb_policy: &Option<ParsedLBPolicy>) -> PolicyKind {
    match lb_policy {
        None => PolicyKind::RoundRobin,
        Some(ParsedLBPolicy::ConsistentHash(_)) => PolicyKind::ConsistentHash,
        Some(ParsedLBPolicy::LeastConn) => PolicyKind::LeastConn,
        Some(ParsedLBPolicy::Ewma) => PolicyKind::Ewma,
    }
}

/// Select a round-robin backend by canonical `"namespace/name"` service key.
///
/// Used by TCP / UDP / TLS routes and the request_mirror plugin, none of which
/// thread an LB policy through. Routes through the per-service
/// `ServiceSelectorCell` so the cached selector is shared with the HTTP path.
pub fn select_roundrobin_backend_key(service_key: &str) -> Option<Arc<EdgionBackend>> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    if !hydrate_cell_if_empty(&cell, service_key) {
        return None;
    }
    let selector = cell.get_or_build(PolicyKind::RoundRobin);
    let health_store = get_health_status_store();
    selector.select(None, |b| is_backend_healthy(service_key, b.inner(), &health_store))
}

/// Get the current backend list for `service_key`.
///
/// Reads from the per-service `ServiceSelectorCell`, which the control plane
/// keeps in sync with the EP/EPS data layer. Used by health/CB convenience
/// APIs and by `pg_upstream_peer::sp_resolve_target` for session-affinity map
/// construction.
pub fn get_backends_for_service_key(service_key: &str) -> Arc<Vec<Arc<EdgionBackend>>> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    let _ = hydrate_cell_if_empty(&cell, service_key);
    cell.backends_snapshot()
}

/// Extract hash key from request based on LB policy configuration.
pub(crate) fn extract_hash_key(session: &Session, lb_policy: &Option<ParsedLBPolicy>) -> Vec<u8> {
    let Some(ParsedLBPolicy::ConsistentHash(hash_on)) = lb_policy else {
        return Vec::new();
    };

    let req_header = session.req_header();

    match hash_on {
        ConsistentHashOn::Header(header_name) => req_header
            .headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default(),
        ConsistentHashOn::Cookie(cookie_name) => req_header
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                    let (name, value) = cookie.split_once('=')?;
                    if name == cookie_name {
                        Some(value.as_bytes().to_vec())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default(),
        ConsistentHashOn::Arg(arg_name) => req_header
            .uri
            .query()
            .and_then(|query| {
                query.split('&').find_map(|param| {
                    let (name, value) = param.split_once('=')?;
                    // Fast path: most query param names are plain identifiers
                    // that match arg_name byte-for-byte, so the common case stays
                    // allocation-free. Only percent/plus-encoded names need
                    // decoding before comparison (e.g. `session%20id` / `foo+bar`
                    // matching an arg_name of `session id` / `foo bar`). The value
                    // is intentionally left raw — decoding it would change the hash
                    // key of existing deployments.
                    let matched = name == arg_name
                        || (name.contains(['%', '+'])
                            && HttpRouteRuleUnit::url_decode(name) == *arg_name);
                    matched.then(|| value.as_bytes().to_vec())
                })
            })
            .unwrap_or_default(),
    }
}

/// Errors raised when the cell yields no backend, broken out per LB policy
/// so the caller maps to the historically-correct `EdgionStatus`.
struct NotFoundMap {
    /// Empty backend list (no policy applies).
    empty: EdgionStatus,
    /// Round-robin selection found nothing healthy.
    rr: EdgionStatus,
    /// Consistent hash selected nothing (non-empty key path).
    ch: EdgionStatus,
    /// LeastConn found nothing.
    lc: EdgionStatus,
    /// EWMA found nothing.
    ewma: EdgionStatus,
}

impl NotFoundMap {
    fn for_kind(&self, kind: PolicyKind, ch_used_hash_key: bool) -> EdgionStatus {
        match kind {
            PolicyKind::RoundRobin => self.rr,
            PolicyKind::ConsistentHash if ch_used_hash_key => self.ch,
            // Empty hash key falls back to RR-style selection internally, so
            // surface the RR not-found code to match historical behaviour.
            PolicyKind::ConsistentHash => self.rr,
            PolicyKind::LeastConn => self.lc,
            PolicyKind::Ewma => self.ewma,
        }
    }
}

fn pick(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
    not_found: NotFoundMap,
) -> Result<Arc<EdgionBackend>, EdgionStatus> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    if !hydrate_cell_if_empty(&cell, service_key) {
        return Err(not_found.empty);
    }

    let kind = policy_to_kind(lb_policy);
    let selector = cell.get_or_build(kind);

    // Build the hash key only for ConsistentHash; an empty key triggers the
    // internal RR fallback inside `CompiledSelector::ConsistentHash::select`.
    let (hash_key, hash_key_used) = if matches!(kind, PolicyKind::ConsistentHash) {
        let key = extract_hash_key(session, lb_policy);
        let used = !key.is_empty();
        (Some(key), used)
    } else {
        (None, false)
    };

    let health_store = get_health_status_store();
    selector
        .select(hash_key.as_deref(), |b| {
            is_backend_healthy(service_key, b.inner(), &health_store)
        })
        .ok_or(not_found.for_kind(kind, hash_key_used))
}

pub(crate) fn select_from_endpoint_slice(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<Arc<EdgionBackend>, EdgionStatus> {
    pick(
        service_key,
        lb_policy,
        session,
        NotFoundMap {
            empty: EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault,
            rr: EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin,
            ch: EdgionStatus::BackendEndpointSliceNotFoundByConsistent,
            lc: EdgionStatus::BackendEndpointSliceNotFoundByLeastConn,
            ewma: EdgionStatus::BackendEndpointSliceNotFoundByEwma,
        },
    )
}
