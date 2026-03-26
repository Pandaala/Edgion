//! UDP Routes and Proxy Implementation
//!
//! This module contains all UDP-related functionality:
//! - Per-port route management ([`routes_mgr`])
//! - UDP proxy implementation ([`edgion_udp`])
//! - Per-port route table ([`udp_route_table`])

mod conf_handler_impl;
mod routes_mgr;
pub(crate) mod udp_route_table;

pub mod edgion_udp;

pub use routes_mgr::{
    get_global_udp_route_managers, GlobalUdpRouteManagers, UdpPortRouteManager, UdpRouteManagerStats,
};

pub use conf_handler_impl::create_udp_route_handler;

pub use udp_route_table::UdpRouteTable;

pub use edgion_udp::EdgionUdpProxy;

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use tokio_util::sync::CancellationToken;

/// Registry that holds CancellationTokens for active UDP listeners,
/// allowing external lifecycle management (hot-reload, graceful shutdown).
pub struct UdpListenerRegistry {
    tokens: RwLock<HashMap<u16, CancellationToken>>,
}

impl UdpListenerRegistry {
    fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Register a cancel token for a UDP listener port.
    /// If a previous listener exists on this port, cancel it first.
    pub fn register(&self, port: u16, token: CancellationToken) {
        let mut map = self.tokens.write().expect("lock udp listener registry");
        if let Some(old) = map.insert(port, token) {
            old.cancel();
        }
    }

    /// Cancel and remove the listener on the given port.
    /// Returns true if a listener was found and cancelled.
    pub fn cancel(&self, port: u16) -> bool {
        let removed = self.tokens.write().expect("lock udp listener registry").remove(&port);
        if let Some(token) = removed {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all registered listeners (for graceful shutdown).
    pub fn cancel_all(&self) {
        let mut map = self.tokens.write().expect("lock udp listener registry");
        for (_, token) in map.drain() {
            token.cancel();
        }
    }
}

static UDP_LISTENER_REGISTRY: OnceLock<UdpListenerRegistry> = OnceLock::new();

pub fn get_udp_listener_registry() -> &'static UdpListenerRegistry {
    UDP_LISTENER_REGISTRY.get_or_init(UdpListenerRegistry::new)
}
