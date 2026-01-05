//! Plugin store module for EdgionPlugins resources

pub mod basic_auth;
mod conf_handler_impl;
pub mod cors;
pub mod csrf;
pub mod ip_restriction;
pub mod mock;
mod plugin_store;

pub use basic_auth::BasicAuth;
pub use conf_handler_impl::create_plugin_handler;
pub use cors::Cors;
pub use csrf::Csrf;
pub use ip_restriction::IpRestriction;
pub use mock::Mock;
pub use plugin_store::{get_global_plugin_store, PluginStore};

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::{
    BasicAuthConfig, CorsConfig, CsrfConfig, IpRestrictionConfig, MockConfig,
};
