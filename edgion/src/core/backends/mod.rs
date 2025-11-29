pub mod services;
pub mod endpoint_slice;

pub use services::{ServiceStore, get_global_service_store, create_service_handler};
pub use endpoint_slice::{EpSliceStore, get_global_ep_slice_store, get_service_key, create_ep_slice_handler};

use pingora_core::protocols::l4::socket::SocketAddr;
use crate::types::{HTTPBackendRef, MatchInfo};

/// Get peer address from service and endpoint slice stores
pub fn get_peer(match_info: &MatchInfo, br: &HTTPBackendRef) -> Option<SocketAddr> {
    let service_key = format!("{}/{}", match_info.rns, br.name);
    
    // Get endpoint slice for this service
    let ep_store = get_global_ep_slice_store();
    let ep_slice = ep_store.get_by_service(&service_key)?;
    
    // Get ready endpoint address
    for endpoint in ep_slice.endpoints {
        if let Some(conditions) = &endpoint.conditions {
            if conditions.ready != Some(true) {
                continue;
            }
        }
        if let Some(addr) = endpoint.addresses.first() {
            let port = br.port.unwrap_or(80);
            let socket_addr: SocketAddr = format!("{}:{}", addr, port).parse().ok()?;
            return Some(socket_addr);
        }
    }
    
    None
}

