//! Stream plugin runtimes for connection and TLS-route policy.
//!
//! `StreamPluginRuntime` evaluates connection context (`DirectPeerIp` and the
//! listener port) at listener admission and TCP/UDP route session creation.
//! TLSRoute can also invoke it through a legacy compatibility fallback; new
//! configurations must not rely on that fallback. `TlsRoutePluginRuntime` runs
//! after TLSRoute matching: terminated TLS supplies post-handshake TLS context,
//! while passthrough supplies inspected ClientHello/SNI route context before
//! forwarding the end-to-end handshake.

pub mod connection_filter_bridge;
pub mod connection_rate_limit;
pub mod geoip_location;
pub mod global_connection_ip_restriction;
pub mod ip_restriction;
mod stream_plugin_runtime;
mod stream_plugin_store;
mod stream_plugin_trait;
pub mod tls_route;

pub use connection_filter_bridge::StreamPluginConnectionFilter;
pub use connection_rate_limit::ConnectionRateLimitPlugin;
pub use geoip_location::GeoIpLocationStreamPlugin;
pub use global_connection_ip_restriction::GlobalConnectionIpRestrictionPlugin;
pub use ip_restriction::StreamIpRestriction;
pub use stream_plugin_runtime::StreamPluginRuntime;
pub use stream_plugin_store::{
    compile_stream_plugins, create_stream_plugin_handler, get_global_stream_plugin_store, CompiledStreamPlugins,
    StreamPluginStore,
};
pub use stream_plugin_trait::{StreamContext, StreamLayer, StreamPlugin, StreamPluginResult};
pub use tls_route::{TlsRouteContext, TlsRoutePlugin, TlsRoutePluginRuntime};
