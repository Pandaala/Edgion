//! Stream plugin system for TCP/UDP routes and connection-level filtering.
//!
//! Two-stage architecture:
//! - **Stage 1 (ConnectionFilter):** pre-TLS, IP-only — `connection_filter_bridge.rs`
//! - **Stage 2 (TlsRoute):** post-handshake, post-route-match — `tls_route/`

pub mod connection_filter_bridge;
pub mod connection_rate_limit;
pub mod global_connection_ip_restriction;
pub mod ip_restriction;
mod stream_plugin_runtime;
mod stream_plugin_store;
mod stream_plugin_trait;
pub mod tls_route;

pub use connection_filter_bridge::StreamPluginConnectionFilter;
pub use connection_rate_limit::ConnectionRateLimitPlugin;
pub use global_connection_ip_restriction::GlobalConnectionIpRestrictionPlugin;
pub use ip_restriction::StreamIpRestriction;
pub use stream_plugin_runtime::StreamPluginRuntime;
pub use stream_plugin_store::{
    compile_stream_plugins, create_stream_plugin_handler, get_global_stream_plugin_store, CompiledStreamPlugins,
    StreamPluginStore,
};
pub use stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
pub use tls_route::{TlsRouteContext, TlsRoutePlugin, TlsRoutePluginRuntime};
