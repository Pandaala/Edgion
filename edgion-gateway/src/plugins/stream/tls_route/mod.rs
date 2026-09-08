//! TLSRoute-specific plugins, evaluated after TLSRoute matching.
//!
//! For terminated TLS, the runtime receives post-handshake TLS and mTLS route
//! context. For passthrough TLS, it receives route context derived from the
//! inspected ClientHello and SNI before the end-to-end handshake is forwarded.
//! The separate connection-context runtime can still run through the legacy
//! TLSRoute compatibility fallback, but new configurations must not rely on it.

pub mod ip_restriction;
mod tls_route_plugin_runtime;
mod tls_route_plugin_trait;

pub use ip_restriction::TlsRouteIpRestriction;
pub use tls_route_plugin_runtime::TlsRoutePluginRuntime;
pub use tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
