pub mod services;
pub mod endpoint_slice;

pub use services::{ServiceStore, get_global_service_store, create_service_handler};
pub use endpoint_slice::{EpSliceStore, get_roundrobin_store, get_consistent_store, get_leastconn_store, create_ep_slice_handler};

use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_proxy::Session;
use crate::types::edgion_status::EdgionStatus;
use crate::types::{ConsistentHashOn, HTTPBackendRef, MatchInfo, ParsedLBPolicy};

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

/// Get peer address from service and endpoint slice stores using load balancing
/// 
/// # Arguments
/// * `match_info` - Route match information
/// * `br` - Backend reference with LB policy
/// * `session` - HTTP session for extracting hash key from request
pub fn get_peer(match_info: &MatchInfo, br: &HTTPBackendRef, session: &Session) -> Result<SocketAddr, EdgionStatus> {
    
    let service_type = EdgionService::from_kind(br.kind.as_ref());
    
    let namespace = br.namespace.as_ref()
        .map(|s| s.as_str())
        .unwrap_or(&match_info.rns);
    let service_key = format!("{}/{}", namespace, br.name);
    
    match service_type {
        EdgionService::Service => {
            // Select backend based on pre-parsed LB policy from extension_info
            let backend = match &br.extension_info.lb_policy {
                Some(ParsedLBPolicy::ConsistentHash(_)) => {
                    // Extract hash key only for consistent hashing
                    let hash_key = extract_hash_key(session, &br.extension_info.lb_policy);
                    let ep_store = get_consistent_store();
                    let ep_lb = ep_store.get_by_service(&service_key)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByConsistent)?;
                    ep_lb.load_balancer().select(&hash_key, 256)
                }
                Some(ParsedLBPolicy::LeastConn) => {
                    let ep_store = get_leastconn_store();
                    let ep_lb = ep_store.get_by_service(&service_key)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByLeastConn)?;
                    ep_lb.load_balancer().select(b"", 256)
                }
                None => {
                    // Default to RoundRobin when no LB policy is specified
                    let ep_store = get_roundrobin_store();
                    let ep_lb = ep_store.get_by_service(&service_key)
                        .ok_or(EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin)?;
                    ep_lb.load_balancer().select(b"", 256)
                }
            }.ok_or(EdgionStatus::BackendLoadBalancerSelectionFailed)?;
            
            // Override port if specified in BackendRef
            let mut addr = backend.addr;
            if let Some(port) = br.port {
                addr.set_port(port as u16);
            }
            
            Ok(addr)
        }
        
        EdgionService::ServiceClusterIp => {
            // Use Service ClusterIP directly (no load balancing, cluster IP is virtual)
            let svc_store = get_global_service_store();
            let service = svc_store.get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;
            
            // Get ClusterIP from Service spec
            let cluster_ip = service.spec.as_ref()
                .and_then(|spec| spec.cluster_ip.as_ref())
                .ok_or(EdgionStatus::BackendClusterIpNotFound)?;
            
            // Get port from BackendRef or Service spec
            let port = get_port_from_backend_ref_or_service(br, &service)?;
            
            // Parse ClusterIP:port as SocketAddr
            let addr_str = format!("{}:{}", cluster_ip, port);
            addr_str.parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)
        }
        
        EdgionService::ServiceExternalName => {
            // Use Service ExternalName (DNS name)
            let svc_store = get_global_service_store();
            let service = svc_store.get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;
            
            // Get ExternalName from Service spec
            let external_name = service.spec.as_ref()
                .and_then(|spec| spec.external_name.as_ref())
                .ok_or(EdgionStatus::BackendExternalNameNotFound)?;
            
            // Get port from BackendRef or Service spec
            let port = get_port_from_backend_ref_or_service(br, &service)?;
            
            // Parse ExternalName:port as SocketAddr
            // Note: ExternalName can be a DNS name, so parsing may fail
            let addr_str = format!("{}:{}", external_name, port);
            addr_str.parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)
        }
        
        EdgionService::ServiceImport => {
            // ServiceImport for multi-cluster not yet implemented
            tracing::warn!(
                service_key = %service_key,
                "ServiceImport is not yet implemented"
            );
            Err(EdgionStatus::BackendServiceImportNotImplemented)
        }
    }
}

