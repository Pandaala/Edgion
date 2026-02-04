pub mod backend_tls;
pub mod endpoint;
pub mod endpoint_slice;
pub mod preload;
pub mod services;

pub use backend_tls::{create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore};
pub use endpoint::{
    create_endpoint_handler, get_endpoint_consistent_store, get_endpoint_ewma_store, get_endpoint_leastconn_store,
    get_endpoint_roundrobin_store, EndpointStore,
};
pub use endpoint_slice::{
    create_ep_slice_handler, get_consistent_store, get_ewma_store, get_leastconn_store, get_roundrobin_store,
    EpSliceStore,
};
pub use preload::preload_load_balancers;
pub use services::{create_service_handler, get_global_service_store, ServiceStore};

use std::sync::OnceLock;

use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::core::gateway::end_response_503;
use crate::core::utils::net::is_localhost;
use crate::types::edgion_status::EdgionStatus;
use crate::types::resources::BackendTLSPolicy;
use crate::types::{ConsistentHashOn, EdgionHttpContext, HTTPBackendRef, ParsedLBPolicy};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
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

/// Select backend using round-robin based on endpoint mode.
/// Uses DCL pattern to lazily create LB if not exists.
///
/// EndpointMode only controls which resources are synced:
/// - Auto/Both/EndpointSlice: use EndpointSlice for backend selection
/// - Endpoint: use Endpoints for backend selection
///
/// Use `ServiceEndpoint` or `ServiceEndpointSlice` in BackendRef.kind to override.
pub fn select_roundrobin_backend(service_key: &str) -> Option<pingora_load_balancing::Backend> {
    match get_global_endpoint_mode() {
        // EndpointSlice, Both, Auto all default to EndpointSlice
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            let lb = get_roundrobin_store().get_or_create(service_key)?;
            lb.load_balancer().select(b"", 256)
        }
        // Only explicit Endpoint mode uses Endpoints
        EndpointMode::Endpoint => {
            let lb = get_endpoint_roundrobin_store().get_or_create(service_key)?;
            lb.load_balancer().select(b"", 256)
        }
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
                _ => EdgionService::Service, // Default to Service for unknown types
            },
        }
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

fn select_from_endpoint_slice(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    // Get RoundRobin store for shared data layer
    let roundrobin_store = get_roundrobin_store();

    match lb_policy {
        None => {
            // DCL: Get or create LB from RoundRobin store
            let lb = roundrobin_store
                .get_or_create(service_key)
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault)
        }
        Some(ParsedLBPolicy::ConsistentHash(_)) => {
            let hash_key = extract_hash_key(session, lb_policy);
            if hash_key.is_empty() {
                // Fallback to RoundRobin
                let lb = roundrobin_store
                    .get_or_create(service_key)
                    .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin)?;
                lb.load_balancer()
                    .select(b"", 256)
                    .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin)
            } else {
                // DCL: Get or create Consistent LB with data from RoundRobin store
                let consistent_store = get_consistent_store();
                let lb = consistent_store
                    .get_or_create_with_provider(service_key, |key| roundrobin_store.get_slices_for_service(key))
                    .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByConsistent)?;
                lb.load_balancer()
                    .select(&hash_key, 256)
                    .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByConsistent)
            }
        }
        Some(ParsedLBPolicy::LeastConn) => {
            // DCL: Get or create LeastConn LB with data from RoundRobin store
            let leastconn_store = get_leastconn_store();
            let lb = leastconn_store
                .get_or_create_with_provider(service_key, |key| roundrobin_store.get_slices_for_service(key))
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByLeastConn)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByLeastConn)
        }
        Some(ParsedLBPolicy::Ewma) => {
            // DCL: Get or create EWMA LB with data from RoundRobin store
            let ewma_store = get_ewma_store();
            let lb = ewma_store
                .get_or_create_with_provider(service_key, |key| roundrobin_store.get_slices_for_service(key))
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByEwma)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByEwma)
        }
    }
}

fn select_from_endpoints(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    // Get RoundRobin store for shared data layer
    let roundrobin_store = get_endpoint_roundrobin_store();

    match lb_policy {
        None => {
            // DCL: Get or create LB from RoundRobin store
            let lb = roundrobin_store
                .get_or_create(service_key)
                .ok_or(EdgionStatus::BackendEndpointNotFoundByRoundRobinDefault)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointNotFoundByRoundRobinDefault)
        }
        Some(ParsedLBPolicy::ConsistentHash(_)) => {
            let hash_key = extract_hash_key(session, lb_policy);
            if hash_key.is_empty() {
                // Fallback to RoundRobin
                let lb = roundrobin_store
                    .get_or_create(service_key)
                    .ok_or(EdgionStatus::BackendEndpointNotFoundByRoundRobin)?;
                lb.load_balancer()
                    .select(b"", 256)
                    .ok_or(EdgionStatus::BackendEndpointNotFoundByRoundRobin)
            } else {
                // DCL: Get or create Consistent LB with data from RoundRobin store
                let consistent_store = get_endpoint_consistent_store();
                let lb = consistent_store
                    .get_or_create_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key))
                    .ok_or(EdgionStatus::BackendEndpointNotFoundByConsistent)?;
                lb.load_balancer()
                    .select(&hash_key, 256)
                    .ok_or(EdgionStatus::BackendEndpointNotFoundByConsistent)
            }
        }
        Some(ParsedLBPolicy::LeastConn) => {
            // DCL: Get or create LeastConn LB with data from RoundRobin store
            let leastconn_store = get_endpoint_leastconn_store();
            let lb = leastconn_store
                .get_or_create_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key))
                .ok_or(EdgionStatus::BackendEndpointNotFoundByLeastConn)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointNotFoundByLeastConn)
        }
        Some(ParsedLBPolicy::Ewma) => {
            // DCL: Get or create EWMA LB with data from RoundRobin store
            let ewma_store = get_endpoint_ewma_store();
            let lb = ewma_store
                .get_or_create_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key))
                .ok_or(EdgionStatus::BackendEndpointNotFoundByEwma)?;
            lb.load_balancer()
                .select(b"", 256)
                .ok_or(EdgionStatus::BackendEndpointNotFoundByEwma)
        }
    }
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

    // Get backend info for service key
    let namespace = ctx
        .backend_context
        .as_ref()
        .map(|bc| bc.namespace.as_str())
        .ok_or(EdgionStatus::Unknown)?;

    let service_key = format!("{}/{}", namespace, br_name);

    match service_type {
        EdgionService::Service => {
            let backend = select_backend_by_policy(&service_key, lb_policy, session)?;

            let mut addr = backend.addr;
            if let Some(port) = br_port {
                addr.set_port(port as u16);
            }

            // Extract TLS configuration from BackendTLSPolicy
            let (use_tls, sni) = extract_tls_config(backend_tls_policy);

            // Clone lb_policy before mutable borrow of ctx
            let lb_policy_clone = lb_policy.clone();

            // Extract hash_key for test metrics before mutable borrow (if test mode enabled)
            // Hash key is saved to ctx for logging stage to build test data
            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            // Store backend address, LB policy, and TLS info in context
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(addr.clone());
                upstream.lb_policy = lb_policy_clone;

                // Record TLS configuration if enabled (inline to avoid double mutable borrow)
                if use_tls {
                    upstream.tls = Some(crate::types::BackendTlsInfo {
                        sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                        handshake_ok: None,
                        protocol: None,
                        cipher: None,
                    });
                }
            }

            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
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
            let (use_tls, sni) = extract_tls_config(backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
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
            let (use_tls, sni) = extract_tls_config(backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
        }

        EdgionService::ServiceImport => {
            tracing::warn!(service_key = %service_key, "ServiceImport is not yet implemented");
            Err(EdgionStatus::BackendServiceImportNotImplemented)
        }

        EdgionService::ServiceEndpoint => {
            // Force use Endpoints (ignore EndpointSlice even in Both mode)
            let backend = select_from_endpoints(&service_key, lb_policy, session)?;

            let mut addr = backend.addr;
            if let Some(port) = br_port {
                addr.set_port(port as u16);
            }

            let (use_tls, sni) = extract_tls_config(backend_tls_policy);
            let lb_policy_clone = lb_policy.clone();

            // Extract hash_key for test metrics
            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(addr.clone());
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

            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
        }

        EdgionService::ServiceEndpointSlice => {
            // Force use EndpointSlice (ignore Endpoint even in Both mode)
            let backend = select_from_endpoint_slice(&service_key, lb_policy, session)?;

            let mut addr = backend.addr;
            if let Some(port) = br_port {
                addr.set_port(port as u16);
            }

            let (use_tls, sni) = extract_tls_config(backend_tls_policy);
            let lb_policy_clone = lb_policy.clone();

            // Extract hash_key for test metrics
            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(addr.clone());
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

            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
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
/// On error, sets error status to ctx and sends 503 response
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
            // Use default server header options for error response
            let server_header_opts = crate::core::gateway::server_header::ServerHeaderOpts::default();
            let _ = end_response_503(session, ctx, &server_header_opts).await;
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
        // Unknown kinds should default to Service
        assert_eq!(
            EdgionService::from_kind(Some(&"UnknownKind".to_string())),
            EdgionService::Service
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"RandomString".to_string())),
            EdgionService::Service
        );
    }
}
