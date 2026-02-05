//! Plugin store module for EdgionPlugins resources

pub mod basic_auth;
mod conf_handler_impl;
pub mod cors;
pub mod csrf;
pub mod ctx_setter;
pub mod ip_restriction;
pub mod jwt_auth;
pub mod mock;
mod plugin_store;
pub mod proxy_rewrite;
pub mod rate_limiter;
pub mod request_restriction;
pub mod response_rewrite;

pub use basic_auth::BasicAuth;
pub use conf_handler_impl::create_plugin_handler;
pub use cors::Cors;
pub use csrf::Csrf;
pub use ctx_setter::CtxSetter;
pub use ip_restriction::IpRestriction;
pub use jwt_auth::JwtAuth;
pub use mock::Mock;
pub use plugin_store::{get_global_plugin_store, PluginStore};
pub use proxy_rewrite::ProxyRewrite;
pub use rate_limiter::RateLimiter;
pub use request_restriction::RequestRestriction;
pub use response_rewrite::ResponseRewrite;

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::{
    BasicAuthConfig, CorsConfig, CsrfConfig, CtxSetterConfig, IpRestrictionConfig, JwtAuthConfig, MockConfig,
    ProxyRewriteConfig, RateLimiterConfig, RequestRestrictionConfig, ResponseRewriteConfig,
};
