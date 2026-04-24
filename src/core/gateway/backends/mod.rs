pub mod circuit_breaker;
pub mod discovery;
pub mod endpoint_mode;
pub mod gateway_backend_tls_store;
pub mod health;
pub mod peer_builder;
pub mod policy;
pub mod port_resolver;
pub mod preload;
pub mod service_kind;
pub mod validation;

pub use circuit_breaker::{get_circuit_breaker_store, parse_circuit_breaker_annotation};
pub use discovery::{
    create_endpoint_handler, create_ep_slice_handler, create_service_handler, get_endpoint_roundrobin_store,
    get_global_service_store, get_roundrobin_store, EndpointDiscovery, EndpointExt, EndpointSliceExt, EndpointStore,
    EpSliceStore, ServiceStore,
};
pub use endpoint_mode::{
    get_global_endpoint_mode, init_global_endpoint_mode, is_endpoint_slice_mode, try_get_global_endpoint_mode,
    EndpointMode,
};
pub use gateway_backend_tls_store::{get_global_gateway_backend_tls_store, GatewayBackendTlsStore};
pub use health::{get_health_check_manager, get_health_status_store};
pub use policy::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, query_backend_tls_policy_for_service,
    BackendTLSPolicyStore,
};
pub use port_resolver::{resolve_target_port, PortLookupSource};
pub use preload::preload_load_balancers;
pub use service_kind::EdgionService;
pub use validation::validate_endpoint_in_route;

use crate::core::gateway::{end_response_500, end_response_503};
use crate::types::edgion_status::EdgionStatus;
use crate::types::{ConsistentHashOn, EdgionHttpContext, ParsedLBPolicy};
use peer_builder::{
    apply_backend_app_protocol, create_tls_peer, detect_backend_app_protocol, extract_tls_config,
    record_tls_to_upstream, validate_backend_addr,
};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;

use crate::core::gateway::lb::runtime_state;
use crate::core::gateway::lb::selection;

/// Health predicate for all self-impl selection algorithms.
///
/// Returns true if the service has no health check configured (default healthy),
/// or if the backend is marked healthy in the health status store.
#[inline]
fn is_backend_healthy(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    // 1. Active health check filter (existing)
    if health_store.has_service(service_key)
        && !health_store.is_healthy(service_key, health::check::backend_hash(backend))
    {
        return false;
    }
    // 2. Circuit breaker filter (new)
    let cb_store = get_circuit_breaker_store();
    if cb_store.is_ejected(service_key, health::check::backend_hash(backend)) {
        crate::core::gateway::observe::metrics::record_cb_rejection(service_key);
        return false;
    }
    true
}

/// Select backend from a backend list using self-impl round-robin.
/// The `RoundRobinSelector` is cached per service in `runtime_state`.
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
///
/// EndpointMode only controls which resources are synced:
/// - Auto/Both/EndpointSlice: use EndpointSlice for backend selection
/// - Endpoint: use Endpoints for backend selection
///
/// Use `ServiceEndpoint` or `ServiceEndpointSlice` in BackendRef.kind to override.
pub fn select_roundrobin_backend(service_key: &str) -> Option<pingora_load_balancing::Backend> {
    let backends = get_backends_for_service(service_key);
    select_rr(service_key, &backends)
}

/// Get backend list for a service from the appropriate store based on endpoint mode.
pub(crate) fn get_backends_for_service(service_key: &str) -> Vec<pingora_load_balancing::Backend> {
    match get_global_endpoint_mode() {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            get_roundrobin_store().get_backends_for_service(service_key)
        }
        EndpointMode::Endpoint => get_endpoint_roundrobin_store().get_backends_for_service(service_key),
    }
}

/// Extract hash key from request based on LB policy configuration
///
/// Returns the hash key bytes for consistent hashing, or empty bytes if not applicable
fn extract_hash_key(session: &Session, lb_policy: &Option<ParsedLBPolicy>) -> Vec<u8> {
    let Some(ParsedLBPolicy::ConsistentHash(hash_on)) = lb_policy else {
        return Vec::new();
    };

    let req_header = session.req_header();

    match hash_on {
        ConsistentHashOn::Header(header_name) => {
            // Extract value from request header
            req_header
                .headers
                .get(header_name.as_str())
                .and_then(|v| v.to_str().ok())
                .map(|s| s.as_bytes().to_vec())
                .unwrap_or_default()
        }
        ConsistentHashOn::Cookie(cookie_name) => {
            // Extract value from Cookie header
            req_header
                .headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    // Parse cookies: "name1=value1; name2=value2"
                    cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                        let (name, value) = cookie.split_once('=')?;

                        if name == cookie_name {
                            Some(value.as_bytes().to_vec())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_default()
        }
        ConsistentHashOn::Arg(arg_name) => {
            // Extract value from query string
            req_header
                .uri
                .query()
                .and_then(|query| {
                    // Parse query string: "name1=value1&name2=value2"
                    query.split('&').find_map(|param| {
                        let (name, value) = param.split_once('=')?;

                        if name == arg_name {
                            Some(value.as_bytes().to_vec())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_default()
        }
    }
}

/// Select backend based on endpoint mode and LB policy.
///
/// EndpointMode only controls which resources are synced:
/// - Auto/Both/EndpointSlice: use EndpointSlice for backend selection
/// - Endpoint: use Endpoints for backend selection
///
/// Use `ServiceEndpoint` or `ServiceEndpointSlice` in BackendRef.kind to override.
fn select_backend_by_policy(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    match get_global_endpoint_mode() {
        // EndpointSlice, Both, Auto all default to EndpointSlice
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            select_from_endpoint_slice(service_key, lb_policy, session)
        }
        // Only explicit Endpoint mode uses Endpoints
        EndpointMode::Endpoint => select_from_endpoints(service_key, lb_policy, session),
    }
}

/// Core selection logic shared by both EndpointSlice and Endpoints paths.
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

fn select_from_endpoint_slice(
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

fn select_from_endpoints(
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

/// Internal: try to get peer, returns Result with EdgionStatus on error
pub(crate) fn try_get_peer(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    is_grpc: bool,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let gateway_key = ctx.gateway_info.gateway_key();
    let gateway_key_ref = Some(gateway_key.as_str());

    // Extract needed fields from backend_ref to avoid clone and borrow checker issues
    let (br_name, br_port, br_kind, lb_policy, backend_tls_policy) = if is_grpc {
        // gRPC backend
        let grpc_br = ctx
            .selected_grpc_backend
            .as_ref()
            .ok_or(EdgionStatus::GrpcUpstreamNotBackendRefs)?;
        (
            &grpc_br.name,
            grpc_br.port,
            grpc_br.kind.as_ref(),
            &grpc_br.extension_info.lb_policy,
            &grpc_br.backend_tls_policy,
        )
    } else {
        // HTTP backend
        let http_br = ctx
            .selected_backend
            .as_ref()
            .ok_or(EdgionStatus::UpstreamNotBackendRefs)?;
        (
            &http_br.name,
            http_br.port,
            http_br.kind.as_ref(),
            &http_br.extension_info.lb_policy,
            &http_br.backend_tls_policy,
        )
    };

    let service_type = EdgionService::from_kind(br_kind);
    let backend_tls_policy = backend_tls_policy.clone();

    // Get backend info for service key
    let namespace = ctx
        .backend_context
        .as_ref()
        .map(|bc| bc.rsc.ns.as_str())
        .ok_or(EdgionStatus::Unknown)?;

    let service_key = format!("{}/{}", namespace, br_name);
    let backend_app_protocol = detect_backend_app_protocol(&service_key, br_port);

    match service_type {
        EdgionService::Service => {
            if !get_global_service_store().contains(&service_key) {
                return Err(EdgionStatus::BackendServiceNotFound);
            }

            // Session persistence: if sp_target_addr is set, use it directly (skip LB)
            let backend = if let Some(target_addr) = ctx.sp_target_addr.take() {
                let health_store = get_health_status_store();
                let b = pingora_load_balancing::Backend {
                    addr: pingora_core::protocols::l4::socket::SocketAddr::Inet(target_addr),
                    weight: 1,
                    ext: Default::default(),
                };
                if is_backend_healthy(&service_key, &b, &health_store) {
                    b
                } else {
                    // Target unhealthy — fall through to normal LB (fallback handled by caller)
                    return Err(EdgionStatus::BackendEndpointSliceNotFoundByConsistent);
                }
            } else {
                select_backend_by_policy(&service_key, lb_policy, session)?
            };

            let lb_addr = backend.addr.clone();
            let mut peer_addr = lb_addr.clone();
            resolve_target_port(&mut peer_addr, br_port, &service_key, PortLookupSource::Auto);

            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);

            let lb_policy_clone = lb_policy.clone();

            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(peer_addr.clone());
                upstream.lb_backend_addr = Some(lb_addr);
                upstream.service_key = Some(service_key.clone());
                upstream.lb_policy = lb_policy_clone;

                if use_tls {
                    upstream.tls = Some(crate::types::BackendTlsInfo {
                        sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                        handshake_ok: None,
                        protocol: None,
                        cipher: None,
                    });
                }
            }

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceClusterIp => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;

            let cluster_ip = service
                .spec
                .as_ref()
                .and_then(|spec| spec.cluster_ip.as_ref())
                .ok_or(EdgionStatus::BackendClusterIpNotFound)?;

            // Get port from br_port or service
            let port = match br_port {
                Some(p) => p as u16,
                None => service
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.ports.as_ref())
                    .and_then(|ports| ports.first())
                    .map(|p| p.port as u16)
                    .ok_or(EdgionStatus::BackendPortResolutionFailed)?,
            };

            let addr_str = format!("{}:{}", cluster_ip, port);
            let addr = addr_str
                .parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            validate_backend_addr(&addr)?;

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceExternalName => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;

            let external_name = service
                .spec
                .as_ref()
                .and_then(|spec| spec.external_name.as_ref())
                .ok_or(EdgionStatus::BackendExternalNameNotFound)?;

            // Get port from br_port or service
            let port = match br_port {
                Some(p) => p as u16,
                None => service
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.ports.as_ref())
                    .and_then(|ports| ports.first())
                    .map(|p| p.port as u16)
                    .ok_or(EdgionStatus::BackendPortResolutionFailed)?,
            };

            let addr_str = format!("{}:{}", external_name, port);
            let addr = addr_str
                .parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            validate_backend_addr(&addr)?;

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceImport => Err(EdgionStatus::BackendServiceImportNotImplemented),

        EdgionService::Unsupported(_) => Err(EdgionStatus::BackendUnsupportedKind),

        EdgionService::ServiceEndpoint => {
            let backend = select_from_endpoints(&service_key, lb_policy, session)?;

            let lb_addr = backend.addr.clone();
            let mut peer_addr = lb_addr.clone();
            resolve_target_port(&mut peer_addr, br_port, &service_key, PortLookupSource::Endpoint);

            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            let lb_policy_clone = lb_policy.clone();

            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(peer_addr.clone());
                upstream.lb_backend_addr = Some(lb_addr);
                upstream.service_key = Some(service_key.clone());
                upstream.lb_policy = lb_policy_clone;
                if use_tls {
                    upstream.tls = Some(crate::types::BackendTlsInfo {
                        sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                        handshake_ok: None,
                        protocol: None,
                        cipher: None,
                    });
                }
            }

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceEndpointSlice => {
            let backend = select_from_endpoint_slice(&service_key, lb_policy, session)?;

            let lb_addr = backend.addr.clone();
            let mut peer_addr = lb_addr.clone();
            resolve_target_port(&mut peer_addr, br_port, &service_key, PortLookupSource::EndpointSlice);

            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            let lb_policy_clone = lb_policy.clone();

            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(peer_addr.clone());
                upstream.lb_backend_addr = Some(lb_addr);
                upstream.service_key = Some(service_key.clone());
                upstream.lb_policy = lb_policy_clone;
                if use_tls {
                    upstream.tls = Some(crate::types::BackendTlsInfo {
                        sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                        handshake_ok: None,
                        protocol: None,
                        cipher: None,
                    });
                }
            }

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }
    }
}

/// Get HTTP peer from service and endpoint slice stores using load balancing
///
/// On error, sets error status to ctx and sends an error response:
/// - 500 for invalid backend configuration (non-existent service, unsupported kind,
///   denied cross-namespace ref) per Gateway API conformance
/// - 503 for transient backend resolution failures (no endpoints, address errors)
///
/// # Parameters
/// - `is_grpc`: true if this is for gRPC backend, false for HTTP backend
pub async fn get_peer(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    is_grpc: bool,
) -> pingora_core::Result<Box<HttpPeer>> {
    match try_get_peer(ctx, session, is_grpc) {
        Ok(peer) => Ok(peer),
        Err(status) => {
            ctx.add_error(status);
            let server_header_opts = crate::core::gateway::runtime::server::server_header::ServerHeaderOpts::default();
            match status {
                EdgionStatus::BackendUnsupportedKind
                | EdgionStatus::BackendServiceNotFound
                | EdgionStatus::RefDenied
                | EdgionStatus::BackendServiceImportNotImplemented => {
                    let _ = end_response_500(session, ctx, &server_header_opts).await;
                }
                _ => {
                    let _ = end_response_503(session, ctx, &server_header_opts).await;
                }
            }
            Err(PingoraError::new(ErrorType::InternalError))
        }
    }
}
