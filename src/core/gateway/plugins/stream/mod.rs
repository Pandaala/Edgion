//! Stream plugin system for TCP/UDP routes and connection-level filtering.
//!
//! Two-stage architecture:
//! - **Stage 1 (ConnectionFilter):** pre-TLS, IP-only — `connection_filter_bridge.rs`
//! - **Stage 2 (TlsRoute):** post-handshake, post-route-match — `tls_route/`

pub mod connection_filter_bridge;
pub mod ip_restriction;
mod stream_plugin_runtime;
mod stream_plugin_store;
mod stream_plugin_trait;
pub mod tls_route;

pub use connection_filter_bridge::StreamPluginConnectionFilter;
pub use ip_restriction::StreamIpRestriction;
pub use stream_plugin_runtime::StreamPluginRuntime;
pub use stream_plugin_store::{create_stream_plugin_handler, get_global_stream_plugin_store, StreamPluginStore};
pub use stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
pub use tls_route::{TlsRouteContext, TlsRoutePlugin, TlsRoutePluginRuntime};

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::IpRestrictionConfig;
