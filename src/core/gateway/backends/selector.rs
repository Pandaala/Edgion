use crate::core::gateway::backends::circuit_breaker::get_circuit_breaker_store;
use crate::core::gateway::backends::discovery::{get_endpoint_roundrobin_store, get_roundrobin_store};
use crate::core::gateway::backends::endpoint_mode::{get_global_endpoint_mode, EndpointMode};
use crate::core::gateway::backends::health;
use crate::core::gateway::backends::health::get_health_status_store;
use crate::core::gateway::lb::{runtime_state, selection};
use crate::types::edgion_status::EdgionStatus;
use crate::types::{ConsistentHashOn, ParsedLBPolicy};
use pingora_proxy::Session;

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

fn select_rr(
    service_key: &str,
    backends: &[pingora_load_balancing::Backend],
) -> Option<pingora_load_balancing::Backend> {
    if backends.is_empty() {
        return None;
    }
    let rr = runtime_state::get_rr_selector(service_key, backends);
    let health_store = get_health_status_store();
    rr.select(256, |b| is_backend_healthy(service_key, b, &health_store))
}

/// Select backend using round-robin based on endpoint mode.
pub fn select_roundrobin_backend(service_key: &str) -> Option<pingora_load_balancing::Backend> {
    let backends = get_backends_for_service(service_key);
    select_rr(service_key, &backends)
}

/// Get backend list for a service from the appropriate store based on endpoint mode.
pub fn get_backends_for_service(service_key: &str) -> Vec<pingora_load_balancing::Backend> {
    match get_global_endpoint_mode() {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            get_roundrobin_store().get_backends_for_service(service_key)
        }
        EndpointMode::Endpoint => get_endpoint_roundrobin_store().get_backends_for_service(service_key),
    }
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
                    if name == arg_name {
                        Some(value.as_bytes().to_vec())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default(),
    }
}

/// Select backend based on endpoint mode and LB policy.
pub(crate) fn select_backend_by_policy(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    match get_global_endpoint_mode() {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            select_from_endpoint_slice(service_key, lb_policy, session)
        }
        EndpointMode::Endpoint => select_from_endpoints(service_key, lb_policy, session),
    }
}

fn select_from_backends(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
    backends: Vec<pingora_load_balancing::Backend>,
    not_found_rr_default: EdgionStatus,
    not_found_rr: EdgionStatus,
    not_found_ch: EdgionStatus,
    not_found_lc: EdgionStatus,
    not_found_ewma: EdgionStatus,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    if backends.is_empty() {
        return Err(not_found_rr_default);
    }
    let health_store = get_health_status_store();

    match lb_policy {
        None => {
            let rr = runtime_state::get_rr_selector(service_key, &backends);
            rr.select(256, |b| is_backend_healthy(service_key, b, &health_store))
                .ok_or(not_found_rr_default)
        }
        Some(ParsedLBPolicy::ConsistentHash(_)) => {
            let hash_key = extract_hash_key(session, lb_policy);
            if hash_key.is_empty() {
                let rr = runtime_state::get_rr_selector(service_key, &backends);
                rr.select(256, |b| is_backend_healthy(service_key, b, &health_store))
                    .ok_or(not_found_rr)
            } else {
                let ring_lock = runtime_state::get_ch_ring(service_key, &backends);
                let ring = ring_lock.read();
                ring.select(&hash_key, 256, |b| is_backend_healthy(service_key, b, &health_store))
                    .ok_or(not_found_ch)
            }
        }
        Some(ParsedLBPolicy::LeastConn) => selection::least_conn::select(&backends, service_key, 256, |b| {
            is_backend_healthy(service_key, b, &health_store)
        })
        .ok_or(not_found_lc),
        Some(ParsedLBPolicy::Ewma) => selection::ewma::select(&backends, service_key, 256, |b| {
            is_backend_healthy(service_key, b, &health_store)
        })
        .ok_or(not_found_ewma),
    }
}

pub(crate) fn select_from_endpoint_slice(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    let backends = get_roundrobin_store().get_backends_for_service(service_key);
    select_from_backends(
        service_key,
        lb_policy,
        session,
        backends,
        EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault,
        EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin,
        EdgionStatus::BackendEndpointSliceNotFoundByConsistent,
        EdgionStatus::BackendEndpointSliceNotFoundByLeastConn,
        EdgionStatus::BackendEndpointSliceNotFoundByEwma,
    )
}

pub(crate) fn select_from_endpoints(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    let backends = get_endpoint_roundrobin_store().get_backends_for_service(service_key);
    select_from_backends(
        service_key,
        lb_policy,
        session,
        backends,
        EdgionStatus::BackendEndpointNotFoundByRoundRobinDefault,
        EdgionStatus::BackendEndpointNotFoundByRoundRobin,
        EdgionStatus::BackendEndpointNotFoundByConsistent,
        EdgionStatus::BackendEndpointNotFoundByLeastConn,
        EdgionStatus::BackendEndpointNotFoundByEwma,
    )
}
