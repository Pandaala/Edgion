pub mod services;
pub mod endpoint_slice;

pub use services::{ServiceStore, get_global_service_store, create_service_handler};
pub use endpoint_slice::{EpSliceStore, get_roundrobin_store, get_consistent_store, get_random_store, create_ep_slice_handler};

use pingora_core::protocols::l4::socket::SocketAddr;
use crate::types::edgion_status::EdgionStatus;
use crate::types::{HTTPBackendRef, MatchInfo};

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

/// Get peer address from service and endpoint slice stores using load balancing
pub fn get_peer(match_info: &MatchInfo, br: &HTTPBackendRef) -> Result<SocketAddr, EdgionStatus> {
    
    let service_type = EdgionService::from_kind(br.kind.as_ref());
    
    let namespace = br.namespace.as_ref()
        .map(|s| s.as_str())
        .unwrap_or(&match_info.rns);
    let service_key = format!("{}/{}", namespace, br.name);
    
    match service_type {
        EdgionService::Service => {
            // Use RoundRobin store by default
            let ep_store = get_roundrobin_store();
            let ep_lb = ep_store.get_by_service(&service_key)
                .ok_or(EdgionStatus::BackendEndpointSliceNotFound)?;
            let lb = ep_lb.load_balancer();
            
            // Use request_id or other key for consistent hashing if needed
            // For now, use empty key for pure round-robin
            let backend = lb.select(b"", 256)
                .ok_or(EdgionStatus::BackendLoadBalancerSelectionFailed)?;
            
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

