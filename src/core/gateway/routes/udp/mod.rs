//! UDP Routes and Proxy Implementation
//!
//! This module contains all UDP-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - UDP proxy implementation ([`edgion_udp`])
//! - Gateway-level route caching ([`gateway_udp_routes`])

mod conf_handler_impl;
mod gateway_udp_routes;
mod routes_mgr;

// UDP proxy module
pub mod edgion_udp;

pub use routes_mgr::{get_global_udp_route_manager, UdpRouteManager};

pub use conf_handler_impl::create_udp_route_handler;

pub use gateway_udp_routes::GatewayUdpRoutes;

// Export UDP proxy type
pub use edgion_udp::EdgionUdpProxy;
