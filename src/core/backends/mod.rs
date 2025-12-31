pub mod services;
pub mod endpoint_slice;
pub mod endpoint;
pub mod backend_tls;

pub use services::{ServiceStore, get_global_service_store, create_service_handler};
pub use endpoint_slice::{EpSliceStore, get_roundrobin_store, get_consistent_store, get_leastconn_store, get_ewma_store, create_ep_slice_handler};
pub use endpoint::{EndpointStore, get_endpoint_roundrobin_store, get_endpoint_consistent_store, get_endpoint_leastconn_store, get_endpoint_ewma_store, create_endpoint_handler};
pub use backend_tls::{BackendTLSPolicyStore, get_global_backend_tls_policy_store, create_backend_tls_policy_handler};

use std::sync::Arc;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use crate::core::gateway::end_response_503;
use crate::core::utils::net::is_localhost;
use crate::types::edgion_status::EdgionStatus;
use crate::types::resources::BackendTLSPolicy;
use crate::types::{ConsistentHashOn, EdgionHttpContext, HTTPBackendRef, ParsedLBPolicy};

/// EdgionService defines the types of backend services
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgionService {
    /// Standard Kubernetes Service
    Service,
    /// Service with ClusterIP
    ServiceClusterIp,
    /// ServiceImport for multi-cluster
    ServiceImport,
    /// Service with ExternalName
    ServiceExternalName,
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
                _ => EdgionService::Service, // Default to Service for unknown types
            }
        }
    }
}

/// Get port from BackendRef or Service spec
/// Returns error if port is not available in either place
#[inline]
fn get_port_from_backend_ref_or_service(br: &HTTPBackendRef, service: &k8s_openapi::api::core::v1::Service) -> Result<u16, EdgionStatus> {
    match br.port {
        Some(p) => Ok(p as u16),
        None => {
            service.spec.as_ref()
                .and_then(|spec| spec.ports.as_ref())
                .and_then(|ports| ports.first())
                .map(|p| p.port as u16)
                .ok_or(EdgionStatus::BackendPortResolutionFailed)
        }
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
            req_header.headers
                .get(header_name.as_str())
                .and_then(|v| v.to_str().ok())
                .map(|s| s.as_bytes().to_vec())
                .unwrap_or_default()
        }
        ConsistentHashOn::Cookie(cookie_name) => {
            // Extract value from Cookie header
            req_header.headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    // Parse cookies: "name1=value1; name2=value2"
                    cookies.split(';')
                        .map(|s| s.trim())
                        .find_map(|cookie| {
                            let mut parts = cookie.splitn(2, '=');
                            let name = parts.next()?;
                            let value = parts.next()?;
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
            req_header.uri.query()
                .and_then(|query| {
                    // Parse query string: "name1=value1&name2=value2"
                    query.split('&')
                        .find_map(|param| {
                            let mut parts = param.splitn(2, '=');
                            let name = parts.next()?;
                            let value = parts.next()?;
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

/// Internal: try to get peer, returns Result with EdgionStatus on error
fn try_get_peer(ctx: &mut EdgionHttpContext, session: &Session, is_grpc: bool) -> Result<Box<HttpPeer>, EdgionStatus> {
    // Extract needed fields from backend_ref to avoid clone and borrow checker issues
    let (br_name, br_port, br_kind, lb_policy, backend_tls_policy) = if is_grpc {
        // gRPC backend
        let grpc_br = ctx.selected_grpc_backend.as_ref()
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
        let http_br = ctx.selected_backend.as_ref()
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
    let namespace = ctx.backend_context.as_ref()
        .map(|bc| bc.namespace.as_str())
        .ok_or(EdgionStatus::Unknown)?;
    
    let service_key = format!("{}/{}", namespace, br_name);
    
    match service_type {
        EdgionService::Service => {
            // Select backend based on pre-parsed LB policy from extension_info
            // None branch first for better branch prediction (most common case)
            let backend = match lb_policy {
                None => {
                    // Default: RoundRobin (most common case)
                    let ep_store = get_roundrobin_store();
                    ep_store.select_peer(&service_key, b"", 256)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault)?
                }
                Some(ParsedLBPolicy::ConsistentHash(_)) => {
                    let hash_key = extract_hash_key(session, lb_policy);
                    // Fallback to RoundRobin when hash_key is empty
                    if hash_key.is_empty() {
                        let ep_store = get_roundrobin_store();
                        ep_store.select_peer(&service_key, b"", 256)
                            .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin)?
                    } else {
                        let ep_store = get_consistent_store();
                        ep_store.select_peer(&service_key, &hash_key, 256)
                            .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByConsistent)?
                    }
                }
                Some(ParsedLBPolicy::LeastConn) => {
                    let ep_store = get_leastconn_store();
                    ep_store.select_peer(&service_key, b"", 256)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByLeastConn)?
                }
                Some(ParsedLBPolicy::Ewma) => {
                    let ep_store = get_ewma_store();
                    ep_store.select_peer(&service_key, b"", 256)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByEwma)?
                }
            };
            
            let mut addr = backend.addr;
            if let Some(port) = br_port {
                addr.set_port(port as u16);
            }
            
            // Extract TLS configuration from BackendTLSPolicy
            let (use_tls, sni) = if let Some(ref policy) = backend_tls_policy {
                (true, policy.spec.validation.hostname.clone())
            } else {
                (false, String::new())
            };
            
            // Store backend address and LB policy in context for connection counting
            let addr_clone = addr.clone();
            let lb_policy_clone = lb_policy.clone();
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(addr_clone);
                upstream.lb_policy = lb_policy_clone;
            }
            
            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
        }
        
        EdgionService::ServiceClusterIp => {
            let svc_store = get_global_service_store();
            let service = svc_store.get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;
            
            let cluster_ip = service.spec.as_ref()
                .and_then(|spec| spec.cluster_ip.as_ref())
                .ok_or(EdgionStatus::BackendClusterIpNotFound)?;
            
            // Get port from br_port or service
            let port = match br_port {
                Some(p) => p as u16,
                None => {
                    service.spec.as_ref()
                        .and_then(|spec| spec.ports.as_ref())
                        .and_then(|ports| ports.first())
                        .map(|p| p.port as u16)
                        .ok_or(EdgionStatus::BackendPortResolutionFailed)?
                }
            };
            
            let addr_str = format!("{}:{}", cluster_ip, port);
            let addr = addr_str.parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;
            
            // Security check: reject localhost connections
            if is_localhost(&addr) {
                tracing::error!(
                    addr = %addr,
                    service_key = %service_key,
                    "Rejected localhost backend for security reasons"
                );
                return Err(EdgionStatus::BackendLocalhostNotAllowed);
            }
            
            // Extract TLS configuration from BackendTLSPolicy
            let (use_tls, sni) = if let Some(ref policy) = backend_tls_policy {
                (true, policy.spec.validation.hostname.clone())
            } else {
                (false, String::new())
            };
            
            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
        }
        
        EdgionService::ServiceExternalName => {
            let svc_store = get_global_service_store();
            let service = svc_store.get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;
            
            let external_name = service.spec.as_ref()
                .and_then(|spec| spec.external_name.as_ref())
                .ok_or(EdgionStatus::BackendExternalNameNotFound)?;
            
            // Get port from br_port or service
            let port = match br_port {
                Some(p) => p as u16,
                None => {
                    service.spec.as_ref()
                        .and_then(|spec| spec.ports.as_ref())
                        .and_then(|ports| ports.first())
                        .map(|p| p.port as u16)
                        .ok_or(EdgionStatus::BackendPortResolutionFailed)?
                }
            };
            
            let addr_str = format!("{}:{}", external_name, port);
            let addr = addr_str.parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;
            
            // Security check: reject localhost connections
            if is_localhost(&addr) {
                tracing::error!(
                    addr = %addr,
                    service_key = %service_key,
                    "Rejected localhost backend for security reasons"
                );
                return Err(EdgionStatus::BackendLocalhostNotAllowed);
            }
            
            // Extract TLS configuration from BackendTLSPolicy
            let (use_tls, sni) = if let Some(ref policy) = backend_tls_policy {
                (true, policy.spec.validation.hostname.clone())
            } else {
                (false, String::new())
            };
            
            Ok(Box::new(HttpPeer::new(addr, use_tls, sni)))
        }
        
        EdgionService::ServiceImport => {
            tracing::warn!(service_key = %service_key, "ServiceImport is not yet implemented");
            Err(EdgionStatus::BackendServiceImportNotImplemented)
        }
    }
}

/// Query BackendTLSPolicy for a given Service
/// 
/// Performs reverse lookup: finds all BackendTLSPolicies whose targetRefs point to the given Service.
/// Returns the highest priority policy (sorted by Gateway API precedence rules).
pub fn query_backend_tls_policy_for_service(
    group: &str,
    kind: &str,
    name: &str,
    namespace: Option<&str>,
) -> Option<Arc<BackendTLSPolicy>> {
    let policy_store = get_global_backend_tls_policy_store();
    let policies = policy_store.get_policies_for_target(group, kind, name, namespace);
    
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
pub async fn get_peer(session: &mut Session, ctx: &mut EdgionHttpContext, is_grpc: bool) -> pingora_core::Result<Box<HttpPeer>> {
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
