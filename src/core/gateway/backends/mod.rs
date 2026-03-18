pub mod discovery;
pub mod health;
pub mod policy;
pub mod preload;
pub mod validation;

pub use crate::core::controller::conf_mgr::conf_center::EndpointMode;
pub use discovery::{
    create_endpoint_handler, create_ep_slice_handler, create_service_handler, get_endpoint_roundrobin_store,
    get_global_service_store, get_roundrobin_store, EndpointDiscovery, EndpointExt, EndpointSliceExt, EndpointStore,
    EpSliceStore, ServiceStore,
};
pub use health::{get_health_check_manager, get_health_status_store};
pub use policy::{create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore};
pub use preload::preload_load_balancers;
pub use validation::validate_endpoint_in_route;

use std::sync::OnceLock;

use crate::core::common::utils::net::is_localhost;
use crate::core::gateway::{end_response_500, end_response_503};
use crate::types::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::types::edgion_status::EdgionStatus;
use crate::types::resources::BackendTLSPolicy;
use crate::types::{ConsistentHashOn, EdgionHttpContext, HTTPBackendRef, ParsedLBPolicy};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use pingora_core::protocols::tls::CaType;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use pingora_core::tls::pkey::PKey;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use pingora_core::tls::x509::X509;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use pingora_core::utils::tls::CertKey;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use std::sync::Arc;

/// Global endpoint mode - initialized once at startup, zero-cost read access.
static GLOBAL_ENDPOINT_MODE: OnceLock<EndpointMode> = OnceLock::new();

/// Initialize global endpoint mode (called once at startup).
///
/// Uses get_or_init to avoid panics on restart and logs mismatch if detected.
pub fn init_global_endpoint_mode(mode: EndpointMode) {
    let existing = GLOBAL_ENDPOINT_MODE.get_or_init(|| {
        tracing::info!(
            component = "backends",
            mode = ?mode,
            "Global endpoint mode initialized"
        );
        mode
    });

    if *existing != mode {
        tracing::warn!(
            component = "backends",
            existing_mode = ?existing,
            new_mode = ?mode,
            "Endpoint mode mismatch on restart - using existing mode"
        );
    }
}

/// Get global endpoint mode (zero-cost after initialization).
#[inline]
pub fn get_global_endpoint_mode() -> EndpointMode {
    *GLOBAL_ENDPOINT_MODE
        .get()
        .expect("GLOBAL_ENDPOINT_MODE not initialized - call init_global_endpoint_mode first")
}

/// Try to get global endpoint mode without panicking.
///
/// Returns `None` if the endpoint mode has not been initialized yet.
/// Useful for code that needs to check endpoint mode but may run before initialization.
#[inline]
pub fn try_get_global_endpoint_mode() -> Option<EndpointMode> {
    GLOBAL_ENDPOINT_MODE.get().copied()
}

/// Check if using EndpointSlice mode.
#[inline]
pub fn is_endpoint_slice_mode() -> bool {
    matches!(get_global_endpoint_mode(), EndpointMode::EndpointSlice)
}

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
    if !health_store.has_service(service_key) {
        return true;
    }
    health_store.is_healthy(service_key, health::check::backend_hash(backend))
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
fn get_backends_for_service(service_key: &str) -> Vec<pingora_load_balancing::Backend> {
    match get_global_endpoint_mode() {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            get_roundrobin_store().get_backends_for_service(service_key)
        }
        EndpointMode::Endpoint => get_endpoint_roundrobin_store().get_backends_for_service(service_key),
    }
}

/// EdgionService defines the types of backend services
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgionService {
    /// Standard Kubernetes Service (default behavior based on endpoint_mode)
    /// In Both mode: tries EndpointSlice first, falls back to Endpoint
    Service,
    /// Service with ClusterIP (direct connection, no LB)
    ServiceClusterIp,
    /// ServiceImport for multi-cluster
    ServiceImport,
    /// Service with ExternalName
    ServiceExternalName,
    /// Force use Endpoints resource for backend discovery
    /// Useful in Both mode to explicitly select Endpoint over EndpointSlice
    ServiceEndpoint,
    /// Force use EndpointSlice resource for backend discovery
    /// Useful in Both mode to explicitly select EndpointSlice
    ServiceEndpointSlice,
    /// Unsupported backend kind (Gateway API conformance: must return 500)
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendAppProtocol {
    H2c,
    WebSocket,
}

impl EdgionService {
    /// Parse service type from kind string
    pub fn from_kind(kind: Option<&String>) -> Self {
        match kind {
            None => EdgionService::Service,
            Some(k) if k.is_empty() => EdgionService::Service,
            Some(k) => match k.as_str() {
                "Service" => EdgionService::Service,
                "ServiceClusterIp" => EdgionService::ServiceClusterIp,
                "ServiceImport" => EdgionService::ServiceImport,
                "ServiceExternalName" => EdgionService::ServiceExternalName,
                // Explicit endpoint mode selection (for Both mode)
                "ServiceEndpoint" | "Endpoint" => EdgionService::ServiceEndpoint,
                "ServiceEndpointSlice" | "EndpointSlice" => EdgionService::ServiceEndpointSlice,
                _ => EdgionService::Unsupported(k.clone()),
            },
        }
    }
}

fn detect_backend_app_protocol(service_key: &str, backend_port: Option<i32>) -> Option<BackendAppProtocol> {
    let service = get_global_service_store().get(service_key)?;
    let ports = service.spec.as_ref()?.ports.as_ref()?;

    let target_port = backend_port.or_else(|| ports.first().map(|p| p.port))?;
    let port = ports.iter().find(|p| p.port == target_port).or_else(|| ports.first())?;
    let app_protocol = port.app_protocol.as_deref()?.to_ascii_lowercase();

    match app_protocol.as_str() {
        "kubernetes.io/h2c" | "h2c" => Some(BackendAppProtocol::H2c),
        "kubernetes.io/ws" | "ws" | "kubernetes.io/wss" | "wss" => Some(BackendAppProtocol::WebSocket),
        _ => None,
    }
}

fn apply_backend_app_protocol(
    peer: &mut Box<HttpPeer>,
    app_protocol: Option<BackendAppProtocol>,
    ctx: &mut EdgionHttpContext,
) {
    match app_protocol {
        Some(BackendAppProtocol::H2c) => {
            // H2C backend: force cleartext HTTP/2 upstream.
            peer.options.set_http_version(2, 2);
        }
        Some(BackendAppProtocol::WebSocket) => {
            // Explicitly use HTTP/1.1 upgrade style for WebSocket backends.
            peer.options.set_http_version(1, 1);
            if ctx.request_info.discover_protocol.is_none() {
                ctx.request_info.discover_protocol = Some("websocket".to_string());
            }
        }
        None => {}
    }
}

/// Get port from BackendRef or Service spec
/// Returns error if port is not available in either place
#[inline]
#[allow(dead_code)]
fn get_port_from_backend_ref_or_service(
    br: &HTTPBackendRef,
    service: &k8s_openapi::api::core::v1::Service,
) -> Result<u16, EdgionStatus> {
    match br.port {
        Some(p) => Ok(p as u16),
        None => service
            .spec
            .as_ref()
            .and_then(|spec| spec.ports.as_ref())
            .and_then(|ports| ports.first())
            .map(|p| p.port as u16)
            .ok_or(EdgionStatus::BackendPortResolutionFailed),
    }
}

/// Which endpoint resource to query when resolving targetPort by port name.
#[derive(Debug, Clone, Copy)]
enum PortLookupSource {
    /// Determine from global endpoint mode (for `EdgionService::Service`).
    Auto,
    /// Force EndpointSlice (for `EdgionService::ServiceEndpointSlice`).
    EndpointSlice,
    /// Force Endpoint (for `EdgionService::ServiceEndpoint`).
    Endpoint,
}

/// Resolve the correct targetPort for a backend address selected from the load balancer.
///
/// The LB stores backends with the targetPort from the first port entry in EndpointSlice/Endpoint.
/// For multi-port Services, `br_port` (backendRef.port = Service port number) may refer to a
/// different port entry. This function looks up the correct targetPort via:
///   br_port (Service port) -> Service.spec.ports[].name -> EndpointSlice/Endpoint port name -> targetPort
///
/// If the Service has only one port, or `br_port` is None, the address is already correct.
fn resolve_target_port(addr: &mut SocketAddr, br_port: Option<i32>, service_key: &str, source: PortLookupSource) {
    let Some(br_port_val) = br_port else {
        return;
    };

    let svc_store = get_global_service_store();
    let Some(service) = svc_store.get(service_key) else {
        return;
    };

    let Some(svc_ports) = service.spec.as_ref().and_then(|s| s.ports.as_ref()) else {
        return;
    };

    if svc_ports.len() <= 1 {
        return;
    }

    let Some(svc_port_entry) = svc_ports.iter().find(|p| p.port == br_port_val) else {
        tracing::warn!(
            service_key = %service_key,
            br_port = br_port_val,
            "backendRef.port does not match any Service port"
        );
        return;
    };

    let Some(port_name) = svc_port_entry.name.as_deref().filter(|n| !n.is_empty()) else {
        return;
    };

    let use_endpoint_slice = match source {
        PortLookupSource::EndpointSlice => true,
        PortLookupSource::Endpoint => false,
        PortLookupSource::Auto => !matches!(get_global_endpoint_mode(), EndpointMode::Endpoint),
    };

    let resolved = if use_endpoint_slice {
        get_roundrobin_store()
            .get_slices_for_service(service_key)
            .and_then(|slices| {
                slices.iter().find_map(|s| {
                    s.ports.as_ref()?.iter().find_map(|p| {
                        if p.name.as_deref() == Some(port_name) {
                            p.port.map(|v| v as u16)
                        } else {
                            None
                        }
                    })
                })
            })
    } else {
        get_endpoint_roundrobin_store()
            .get_endpoint_for_service(service_key)
            .and_then(|ep| {
                ep.subsets.as_ref()?.iter().find_map(|subset| {
                    subset.ports.as_ref()?.iter().find_map(|p| {
                        if p.name.as_deref() == Some(port_name) {
                            Some(p.port as u16)
                        } else {
                            None
                        }
                    })
                })
            })
    };

    if let Some(target_port) = resolved {
        let current_port = addr.as_inet().map(|a| a.port()).unwrap_or(0);
        if current_port != target_port {
            tracing::debug!(
                service_key = %service_key,
                port_name = port_name,
                from_port = current_port,
                to_port = target_port,
                "Resolved targetPort for multi-port Service"
            );
            addr.set_port(target_port);
        }
    } else {
        tracing::warn!(
            service_key = %service_key,
            port_name = port_name,
            "Could not resolve targetPort by port name in endpoint resources"
        );
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
                let ring = ring_lock.read().unwrap_or_else(|e| e.into_inner());
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

/// Extract TLS configuration from BackendTLSPolicy
///
/// Returns (use_tls, sni) tuple:
/// - If policy exists: (true, hostname from policy)
/// - Otherwise: (false, empty string)
#[inline]
fn extract_tls_config(policy: &Option<Arc<BackendTLSPolicy>>) -> (bool, String) {
    policy
        .as_ref()
        .map(|p| (true, p.spec.validation.hostname.clone()))
        .unwrap_or((false, String::new()))
}

#[cfg(any(feature = "boringssl", feature = "openssl"))]
fn build_client_cert_key(policy: &BackendTLSPolicy) -> Result<Option<Arc<CertKey>>, EdgionStatus> {
    let Some(secret) = &policy.spec.resolved_client_certificate else {
        return Ok(None);
    };

    let data = secret.data.as_ref().ok_or(EdgionStatus::Unknown)?;
    let cert_pem = data.get(CERT).ok_or(EdgionStatus::Unknown)?;
    let key_pem = data.get(KEY).ok_or(EdgionStatus::Unknown)?;

    let cert = X509::from_pem(cert_pem.0.as_slice()).map_err(|_| EdgionStatus::Unknown)?;
    let key = PKey::private_key_from_pem(key_pem.0.as_slice()).map_err(|_| EdgionStatus::Unknown)?;

    Ok(Some(Arc::new(CertKey::new(vec![cert], key))))
}

#[cfg(not(any(feature = "boringssl", feature = "openssl")))]
fn build_client_cert_key(policy: &BackendTLSPolicy) -> Result<Option<Arc<()>>, EdgionStatus> {
    if policy.spec.resolved_client_certificate.is_some() {
        tracing::warn!(
            policy = %format!("{}/{}", policy.namespace().unwrap_or(""), policy.name()),
            "upstream mTLS client cert is not supported by this TLS feature set"
        );
    }
    Ok(None)
}

#[cfg(any(feature = "boringssl", feature = "openssl"))]
fn build_ca_chain(policy: &BackendTLSPolicy) -> Option<Arc<CaType>> {
    let mut ca_chain = Vec::new();
    for secret in policy.spec.resolved_ca_certificates.as_ref()? {
        let Some(data) = &secret.data else {
            continue;
        };
        let Some(ca_pem) = data.get(CA_CERT) else {
            continue;
        };
        if let Ok(ca) = X509::from_pem(ca_pem.0.as_slice()) {
            ca_chain.push(ca);
        }
    }

    if ca_chain.is_empty() {
        None
    } else {
        Some(Arc::new(ca_chain.into_boxed_slice()))
    }
}

#[cfg(not(any(feature = "boringssl", feature = "openssl")))]
fn build_ca_chain(_policy: &BackendTLSPolicy) -> Option<Arc<()>> {
    None
}

fn create_tls_peer(addr: SocketAddr, policy: &Option<Arc<BackendTLSPolicy>>) -> Result<Box<HttpPeer>, EdgionStatus> {
    let Some(policy) = policy.as_ref() else {
        return Ok(Box::new(HttpPeer::new(addr, false, String::new())));
    };

    let sni = policy.spec.validation.hostname.clone();

    #[cfg(any(feature = "boringssl", feature = "openssl"))]
    let mut peer = if let Some(cert_key) = build_client_cert_key(policy)? {
        HttpPeer::new_mtls(addr, sni.clone(), cert_key)
    } else {
        HttpPeer::new(addr, true, sni)
    };

    #[cfg(not(any(feature = "boringssl", feature = "openssl")))]
    let mut peer = HttpPeer::new(addr, true, sni);

    #[cfg(any(feature = "boringssl", feature = "openssl"))]
    if let Some(ca_chain) = build_ca_chain(policy) {
        peer.options.ca = Some(ca_chain);
    }

    Ok(Box::new(peer))
}

/// Record TLS configuration to upstream context
///
/// Updates the current upstream's TLS info if use_tls is true.
/// Called after extracting TLS config to record it for observability.
#[inline]
fn record_tls_to_upstream(ctx: &mut EdgionHttpContext, use_tls: bool, sni: &str) {
    if use_tls {
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.tls = Some(crate::types::BackendTlsInfo {
                sni: if sni.is_empty() { None } else { Some(sni.to_string()) },
                handshake_ok: None, // Will be updated after connection
                protocol: None,
                cipher: None,
            });
        }
    }
}

/// Validate backend address for security
///
/// Rejects localhost connections to prevent SSRF attacks.
#[inline]
fn validate_backend_addr(addr: &SocketAddr, service_key: &str) -> Result<(), EdgionStatus> {
    if is_localhost(addr) {
        tracing::error!(
            addr = %addr,
            service_key = %service_key,
            "Rejected localhost backend for security reasons"
        );
        return Err(EdgionStatus::BackendLocalhostNotAllowed);
    }
    Ok(())
}

/// Internal: try to get peer, returns Result with EdgionStatus on error
fn try_get_peer(ctx: &mut EdgionHttpContext, session: &Session, is_grpc: bool) -> Result<Box<HttpPeer>, EdgionStatus> {
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
        .map(|bc| bc.namespace.as_str())
        .ok_or(EdgionStatus::Unknown)?;

    let service_key = format!("{}/{}", namespace, br_name);
    let backend_app_protocol = detect_backend_app_protocol(&service_key, br_port);

    match service_type {
        EdgionService::Service => {
            if !get_global_service_store().contains(&service_key) {
                return Err(EdgionStatus::BackendServiceNotFound);
            }
            let backend = select_backend_by_policy(&service_key, lb_policy, session)?;

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

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy)?;
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

            // Security check: reject localhost connections
            validate_backend_addr(&addr, &service_key)?;

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy)?;
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

            // Security check: reject localhost connections
            validate_backend_addr(&addr, &service_key)?;

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceImport => {
            tracing::warn!(service_key = %service_key, "ServiceImport is not yet implemented");
            Err(EdgionStatus::BackendServiceImportNotImplemented)
        }

        EdgionService::Unsupported(ref kind) => {
            tracing::error!(service_key = %service_key, kind = %kind, "Unsupported backend kind");
            Err(EdgionStatus::BackendUnsupportedKind)
        }

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

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy)?;
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

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }
    }
}

/// Query BackendTLSPolicy for a given Service
///
/// Performs reverse lookup: finds all BackendTLSPolicies whose targetRefs point to the given Service.
/// Returns the highest priority policy (sorted by Gateway API precedence rules).
pub fn query_backend_tls_policy_for_service(name: &str, namespace: Option<&str>) -> Option<Arc<BackendTLSPolicy>> {
    let policy_store = get_global_backend_tls_policy_store();
    let policies = policy_store.get_policies_for_target(name, namespace);

    if policies.is_empty() {
        return None;
    }

    // Return the highest priority policy (first one, already sorted)
    policies.into_iter().next()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edgion_service_from_kind_default() {
        // None or empty string should default to Service
        assert_eq!(EdgionService::from_kind(None), EdgionService::Service);
        assert_eq!(EdgionService::from_kind(Some(&String::new())), EdgionService::Service);
    }

    #[test]
    fn test_edgion_service_from_kind_standard() {
        assert_eq!(
            EdgionService::from_kind(Some(&"Service".to_string())),
            EdgionService::Service
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceClusterIp".to_string())),
            EdgionService::ServiceClusterIp
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceImport".to_string())),
            EdgionService::ServiceImport
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceExternalName".to_string())),
            EdgionService::ServiceExternalName
        );
    }

    #[test]
    fn test_edgion_service_from_kind_explicit_endpoint() {
        // ServiceEndpoint and Endpoint should map to ServiceEndpoint
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceEndpoint".to_string())),
            EdgionService::ServiceEndpoint
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"Endpoint".to_string())),
            EdgionService::ServiceEndpoint
        );
    }

    #[test]
    fn test_edgion_service_from_kind_explicit_endpoint_slice() {
        // ServiceEndpointSlice and EndpointSlice should map to ServiceEndpointSlice
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceEndpointSlice".to_string())),
            EdgionService::ServiceEndpointSlice
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"EndpointSlice".to_string())),
            EdgionService::ServiceEndpointSlice
        );
    }

    #[test]
    fn test_edgion_service_from_kind_unknown() {
        assert_eq!(
            EdgionService::from_kind(Some(&"UnknownKind".to_string())),
            EdgionService::Unsupported("UnknownKind".to_string())
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"RandomString".to_string())),
            EdgionService::Unsupported("RandomString".to_string())
        );
    }
}
