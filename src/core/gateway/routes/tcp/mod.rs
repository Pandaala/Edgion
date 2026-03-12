//! TCP Routes and Proxy Implementation
//!
//! This module contains all TCP-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TCP proxy implementation ([`edgion_tcp`])
//! - Gateway-level route caching ([`gateway_tcp_routes`])

mod conf_handler_impl;
mod gateway_tcp_routes;
mod routes_mgr;

// TCP proxy module
pub mod edgion_tcp;

pub use routes_mgr::{get_global_tcp_route_manager, TcpRouteManager};

pub use conf_handler_impl::create_tcp_route_handler;

pub use gateway_tcp_routes::GatewayTcpRoutes;

// Export TCP proxy types
pub use edgion_tcp::{EdgionTcpProxy, TcpContext, TcpStatus};
