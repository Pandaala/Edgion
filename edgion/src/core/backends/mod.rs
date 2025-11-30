pub mod services;
pub mod endpoint_slice;

pub use services::{ServiceStore, get_global_service_store, create_service_handler};
pub use endpoint_slice::{EpSliceStore, get_global_ep_slice_store, get_service_key, create_ep_slice_handler};

use pingora_core::protocols::l4::socket::SocketAddr;
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

/// Get peer address from service and endpoint slice stores using load balancing
pub fn get_peer(match_info: &MatchInfo, br: &HTTPBackendRef) -> Option<SocketAddr> {
    // Determine service type from br.kind
    let service_type = EdgionService::from_kind(br.kind.as_ref());
    
    // Only process Service type
    if service_type != EdgionService::Service {
        return None;
    }
    
    let service_key = format!("{}/{}", match_info.rns, br.name);
    
    // Get endpoint slice load balancer for this service
    let ep_store = get_global_ep_slice_store();
    let ep_lb = ep_store.get_by_service(&service_key)?;
    
    // Use load balancer to select a backend using RoundRobin algorithm
    let lb = ep_lb.load_balancer();
    
    // Use request_id or other key for consistent hashing if needed
    // For now, use empty key for pure round-robin
    let backend = lb.select(b"", 256)?;
    
    // Return the backend address
    Some(backend.addr)
}

