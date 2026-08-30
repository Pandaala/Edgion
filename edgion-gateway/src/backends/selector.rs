use crate::backends::discovery::get_roundrobin_store;
use crate::backends::health;
use crate::backends::health::get_health_status_store;
use crate::backends::outlier_detection::{get_outlier_detection_store, OutlierDetectionStore};
use crate::lb::{compiled_selector, EdgionBackend, PolicyKind};
use edgion_resources::resources::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use edgion_resources::uri_utils;
use edgion_resources::EdgionErrorCode;
use pingora_proxy::Session;
use std::sync::Arc;

/// Lazily hydrate `cell` from the EndpointSlice data layer when it is empty.
///
/// `update_affected_lbs` / `update_all_existing_lbs` populate cells reactively
/// as EP updates arrive, but a freshly created cell (from `get_or_create_cell`
/// on first access) is empty until one of those updates fires for its service
/// key. Without this fallback, a request that arrives before the first EP push
/// for a service would always return `not_found`.
///
/// Reads the current EndpointSlice store and seeds the cell. Subsequent EP
/// updates call `replace_backends` and remain authoritative.
///
/// Returns `true` if the cell ended up non-empty after this call.
fn hydrate_cell_if_empty(cell: &compiled_selector::ServiceSelectorCell, service_key: &str) -> bool {
    cell.hydrate_if_empty(|| get_roundrobin_store().get_backends_for_service(service_key))
}

/// Health predicate without rejection accounting. Used by the panic-threshold
/// counting pass so it does not double-count outlier-detection rejections.
/// Lazy outlier recovery may still decrement the ejected-endpoint gauge.
#[inline]
pub(crate) fn is_backend_healthy_quiet(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
    outlier_store: &OutlierDetectionStore,
) -> bool {
    let backend_hash = health::check::backend_hash(backend);
    if !health_store.is_healthy(service_key, backend_hash) {
        return false;
    }
    !outlier_store.is_ejected(service_key, backend_hash)
}

/// Health predicate for the selection walk. Same logic as
/// [`is_backend_healthy_quiet`] but records a outlier-detection rejection metric
/// when a backend is rejected for being ejected (preserves existing behavior).
#[inline]
pub(crate) fn is_backend_healthy(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
    outlier_store: &OutlierDetectionStore,
) -> bool {
    let backend_hash = health::check::backend_hash(backend);
    // 1. Active health check filter
    if !health_store.is_healthy(service_key, backend_hash) {
        return false;
    }
    // 2. Outlier detection filter (records the rejection metric)
    if outlier_store.is_ejected(service_key, backend_hash) {
        crate::observe::metrics::record_cb_rejection(service_key);
        return false;
    }
    true
}

/// Decide whether panic mode should engage for this selection. Gated so the
/// O(n) survivor count only runs for services that actually have health-check
/// or outlier-detection state (otherwise every backend passes and panic can never
/// trigger). Records the `lb_panic_mode` gauge when it evaluates.
fn panic_engaged(
    service_key: &str,
    selector: &compiled_selector::CompiledSelector,
    health_store: &health::check::HealthStatusStore,
    outlier_store: &OutlierDetectionStore,
) -> bool {
    if !health_store.has_service(service_key) && !outlier_store.has_service(service_key) {
        return false;
    }
    let threshold = crate::backends::panic_threshold::resolve_panic_threshold(service_key);
    if threshold == 0 {
        return false;
    }
    let total = selector.backend_count();
    if total == 0 {
        return false;
    }
    let floor = crate::backends::panic_threshold::panic_floor(threshold, total);
    let engaged = selector.panic_should_engage(floor, |b| {
        is_backend_healthy_quiet(service_key, b.inner(), health_store, outlier_store)
    });
    crate::observe::metrics::record_lb_panic_mode(service_key, engaged);
    engaged
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
    let outlier_store = get_outlier_detection_store();
    if panic_engaged(service_key, &selector, &health_store, &outlier_store) {
        return selector.select(None, |_| true);
    }
    selector.select(None, |b| {
        is_backend_healthy(service_key, b.inner(), &health_store, &outlier_store)
    })
}

/// Get the current backend list for `service_key`.
///
/// Reads from the per-service `ServiceSelectorCell`, which the control plane
/// keeps in sync with the EP/EPS data layer. Used by health/CB convenience
/// APIs and by `session_persistence::sp_lookup_rule_scoped` for session-affinity
/// map construction.
pub fn get_backends_for_service_key(service_key: &str) -> Arc<Vec<Arc<EdgionBackend>>> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    let _ = hydrate_cell_if_empty(&cell, service_key);
    cell.backends_snapshot()
}

/// Extract hash key from request based on LB policy configuration.
///
/// `remote_ip` is the RealIp-normalized client source IP (`ctx.request_info.remote_ip`),
/// used only by the `SourceIp` mode; the other modes read from the request headers/URI.
pub(crate) fn extract_hash_key<'a>(
    session: &'a Session,
    lb_policy: &Option<ParsedLBPolicy>,
    remote_ip: &'a str,
    query: Option<&'a str>,
) -> &'a [u8] {
    let Some(ParsedLBPolicy::ConsistentHash(hash_on)) = lb_policy else {
        return &[];
    };

    let req_header = session.req_header();

    match hash_on {
        ConsistentHashOn::Header(header_name) => req_header
            .headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(str::as_bytes)
            .unwrap_or_default(),
        ConsistentHashOn::Cookie(cookie_name) => req_header
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                    let (name, value) = cookie.split_once('=')?;
                    if name == cookie_name {
                        Some(value.as_bytes())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default(),
        ConsistentHashOn::Arg(arg_name) => query
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
                    let matched =
                        name == arg_name || (name.contains(['%', '+']) && uri_utils::url_decode(name) == *arg_name);
                    matched.then_some(value.as_bytes())
                })
            })
            .unwrap_or_default(),
        // SourceIp hashes on the normalized client IP; an empty IP yields an
        // empty key, driving the internal RR fallback like the other modes when
        // their value is missing.
        ConsistentHashOn::SourceIp => remote_ip.as_bytes(),
    }
}

/// Errors raised when the cell yields no backend, broken out per LB policy
/// so the caller maps to the historically-correct `EdgionErrorCode`.
struct NotFoundMap {
    /// Empty backend list (no policy applies).
    empty: EdgionErrorCode,
    /// Round-robin selection found nothing healthy.
    rr: EdgionErrorCode,
    /// Consistent hash selected nothing (non-empty key path).
    ch: EdgionErrorCode,
    /// LeastConn found nothing.
    lc: EdgionErrorCode,
    /// EWMA found nothing.
    ewma: EdgionErrorCode,
}

impl NotFoundMap {
    fn for_kind(&self, kind: PolicyKind, ch_used_hash_key: bool) -> EdgionErrorCode {
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
    remote_ip: &str,
    query: Option<&str>,
    not_found: NotFoundMap,
) -> Result<Arc<EdgionBackend>, EdgionErrorCode> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    if !hydrate_cell_if_empty(&cell, service_key) {
        return Err(not_found.empty);
    }

    let kind = policy_to_kind(lb_policy);
    let selector = cell.get_or_build(kind);

    // Build the hash key only for ConsistentHash; an empty key triggers the
    // internal RR fallback inside `CompiledSelector::ConsistentHash::select`.
    let (hash_key, hash_key_used) = if matches!(kind, PolicyKind::ConsistentHash) {
        let key = extract_hash_key(session, lb_policy, remote_ip, query);
        let used = !key.is_empty();
        (Some(key), used)
    } else {
        (None, false)
    };

    let health_store = get_health_status_store();
    let outlier_store = get_outlier_detection_store();
    let result = if panic_engaged(service_key, &selector, &health_store, &outlier_store) {
        selector.select(hash_key, |_| true)
    } else {
        selector.select(hash_key, |b| {
            is_backend_healthy(service_key, b.inner(), &health_store, &outlier_store)
        })
    };
    result.ok_or(not_found.for_kind(kind, hash_key_used))
}

pub(crate) fn select_from_endpoint_slice(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
    remote_ip: &str,
    query: Option<&str>,
) -> Result<Arc<EdgionBackend>, EdgionErrorCode> {
    pick(
        service_key,
        lb_policy,
        session,
        remote_ip,
        query,
        NotFoundMap {
            empty: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault,
            rr: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobin,
            ch: EdgionErrorCode::BackendEndpointSliceNotFoundByConsistent,
            lc: EdgionErrorCode::BackendEndpointSliceNotFoundByLeastConn,
            ewma: EdgionErrorCode::BackendEndpointSliceNotFoundByEwma,
        },
    )
}

#[cfg(test)]
mod panic_tests {
    use super::*;
    use crate::backends::health::get_health_status_store;
    use crate::backends::panic_threshold::{configure_lb_panic_default, get_panic_threshold_store};
    use crate::lb::compiled_selector;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backend(i: u8) -> Backend {
        Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 80)),
            weight: 1,
            ext: Default::default(),
        }
    }

    fn seed_cell(service_key: &str, n: u8) {
        let cell = compiled_selector::get_or_create_cell(service_key);
        let bs: Vec<crate::lb::EdgionBackend> = (0..n).map(|i| crate::lb::EdgionBackend::new(backend(i + 1))).collect();
        cell.replace_backends(bs);
    }

    fn mark_all_unhealthy(service_key: &str, n: u8) {
        let hs = get_health_status_store();
        let hashes: Vec<u64> = (0..n).map(|i| health::check::backend_hash(&backend(i + 1))).collect();
        hs.register_service(service_key, hashes.clone());
        for h in hashes {
            for _ in 0..5 {
                hs.record_check(service_key, h, false, 1, 1);
            }
        }
    }

    #[test]
    fn quiet_and_metric_predicates_agree_with_no_config() {
        let hs = get_health_status_store();
        let outlier_store = get_outlier_detection_store();
        let b = backend(1);
        // No HC/CB config for this service key → both predicates return true.
        assert!(is_backend_healthy_quiet(
            "default/unconfigured-svc",
            &b,
            &hs,
            &outlier_store
        ));
        assert!(is_backend_healthy("default/unconfigured-svc", &b, &hs, &outlier_store));
    }

    #[test]
    fn panic_off_all_unhealthy_returns_none() {
        let svc = "default/panic-off";
        configure_lb_panic_default(0);
        get_panic_threshold_store().set_service_threshold(svc, None);
        seed_cell(svc, 4);
        mark_all_unhealthy(svc, 4);
        assert!(select_roundrobin_backend_key(svc).is_none());
    }

    #[test]
    fn panic_on_all_unhealthy_selects_backend() {
        let svc = "default/panic-on";
        get_panic_threshold_store().set_service_threshold(svc, Some(50));
        seed_cell(svc, 4);
        mark_all_unhealthy(svc, 4);
        assert!(select_roundrobin_backend_key(svc).is_some());
        get_panic_threshold_store().set_service_threshold(svc, None);
    }
}
