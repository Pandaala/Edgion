//! UDP Routes and Proxy Implementation
//!
//! This module contains all UDP-related functionality:
//! - Per-port route management ([`routes_mgr`])
//! - UDP proxy implementation ([`edgion_udp`])
//! - Per-port route table ([`udp_route_table`])

mod conf_handler_impl;
mod route_unit;
mod routes_mgr;
pub(crate) mod udp_route_table;

pub mod edgion_udp;

pub use route_unit::UdpRouteUnit;

pub use routes_mgr::{
    get_global_udp_route_managers, GlobalUdpRouteManagers, UdpPortRouteManager, UdpRouteManagerStats,
};

pub use conf_handler_impl::create_udp_route_handler;

pub use udp_route_table::UdpRouteTable;

pub use edgion_udp::EdgionUdpProxy;

use crate::types::ListenerKey;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

/// Registry that holds CancellationTokens for active UDP listeners,
/// allowing external lifecycle management (hot-reload, graceful shutdown).
pub struct UdpListenerRegistry {
    tokens: RwLock<HashMap<ListenerKey, CancellationToken>>,
}

impl UdpListenerRegistry {
    fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Register a cancel token for a UDP listener key.
    /// If a previous listener exists for this key, cancel it first.
    pub fn register(&self, key: &ListenerKey, token: CancellationToken) {
        let mut map = self.tokens.write();
        if let Some(old) = map.insert(key.clone(), token) {
            old.cancel();
        }
    }

    /// Cancel and remove the listener for the given key.
    /// Returns true if a listener was found and cancelled.
    pub fn cancel(&self, key: &ListenerKey) -> bool {
        let removed = self.tokens.write().remove(key);
        if let Some(token) = removed {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all registered listeners (for graceful shutdown).
    pub fn cancel_all(&self) {
        let mut map = self.tokens.write();
        for (_, token) in map.drain() {
            token.cancel();
        }
    }
}

static UDP_LISTENER_REGISTRY: OnceLock<UdpListenerRegistry> = OnceLock::new();

pub fn get_udp_listener_registry() -> &'static UdpListenerRegistry {
    UDP_LISTENER_REGISTRY.get_or_init(UdpListenerRegistry::new)
}
