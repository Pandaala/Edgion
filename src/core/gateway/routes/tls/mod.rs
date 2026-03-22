//! TLS Routes and Proxy Implementation
//!
//! This module contains all TLS-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TLS proxy implementation ([`proxy`]) — Terminate mode
//! - TLS passthrough proxy ([`passthrough_proxy`]) — Passthrough mode
//! - ClientHello SNI extraction ([`sni_parser`])
//! - Per-port route table ([`gateway_tls_routes`])

mod conf_handler_impl;
pub(crate) mod gateway_tls_routes;
mod routes_mgr;

// TLS proxy modules
pub mod passthrough_proxy;
pub mod proxy;
pub mod sni_parser;

pub use routes_mgr::{get_global_tls_route_managers, GlobalTlsRouteManagers, TlsRouteManager, TlsRouteManagerStats};

pub use conf_handler_impl::create_tls_route_handler;

pub use gateway_tls_routes::TlsRouteTable;

// Export TLS proxy types
pub use passthrough_proxy::EdgionTlsPassthroughProxy;
pub use proxy::EdgionTlsTcpProxy;
