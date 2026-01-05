//! Stream plugin system for TCP/UDP routes

pub mod ip_restriction;
mod stream_plugin_runtime;
mod stream_plugin_store;
mod stream_plugin_trait;

pub use ip_restriction::StreamIpRestriction;
pub use stream_plugin_runtime::StreamPluginRuntime;
pub use stream_plugin_store::{create_stream_plugin_handler, get_global_stream_plugin_store, StreamPluginStore};
pub use stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::IpRestrictionConfig;
