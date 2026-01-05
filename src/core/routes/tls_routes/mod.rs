//! TLS Routes and Proxy Implementation
//!
//! This module contains all TLS-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TLS proxy implementation ([`edgion_tls`])
//! - Gateway-level route caching ([`gateway_tls_routes`])

mod conf_handler_impl;
mod gateway_tls_routes;
mod routes_mgr;

// TLS proxy module
pub mod edgion_tls;

pub use routes_mgr::{get_global_tls_route_manager, TlsRouteManager};

pub use conf_handler_impl::create_tls_route_handler;

pub use gateway_tls_routes::GatewayTlsRoutes;

// Export TLS proxy type
pub use edgion_tls::EdgionTls;
