//! TLS route stage plugins — post-handshake, post-route-match.
//!
//! This module contains the Stage 2 stream plugin system for TLS routes.
//! Stage 1 (ConnectionFilter, pre-TLS) lives in `connection_filter_bridge.rs`.

pub mod ip_restriction;
mod tls_route_plugin_runtime;
mod tls_route_plugin_trait;

pub use ip_restriction::TlsRouteIpRestriction;
pub use tls_route_plugin_runtime::TlsRoutePluginRuntime;
pub use tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
