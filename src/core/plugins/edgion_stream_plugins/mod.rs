//! Stream plugin system for TCP/UDP routes

mod stream_plugin_trait;
mod stream_plugin_runtime;
pub mod ip_restriction;

pub use stream_plugin_trait::{StreamPlugin, StreamPluginResult, StreamContext};
pub use stream_plugin_runtime::StreamPluginRuntime;
pub use ip_restriction::StreamIpRestriction;

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::IpRestrictionConfig;

