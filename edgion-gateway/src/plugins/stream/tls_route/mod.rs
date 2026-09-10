//! TLSRoute-specific plugins, evaluated after TLSRoute matching.
//!
//! For terminated TLS, the runtime receives post-handshake TLS and mTLS route
//! context. For passthrough TLS, it receives route context derived from the
//! inspected ClientHello and SNI before the end-to-end handshake is forwarded.
//! TLSRoute attachments execute only this runtime; Stage-1-only attachments
//! are rejected unless they explicitly declare `tlsRoutePlugins`.

pub mod ip_restriction;
mod tls_route_plugin_runtime;
mod tls_route_plugin_trait;

pub use ip_restriction::TlsRouteIpRestriction;
pub use tls_route_plugin_runtime::TlsRoutePluginRuntime;
pub use tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
