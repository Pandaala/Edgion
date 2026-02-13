//! Plugin store module for EdgionPlugins resources

pub mod all_endpoint_status;
pub mod bandwidth_limit;
pub mod basic_auth;
pub mod common;
mod conf_handler_impl;
pub mod cors;
pub mod csrf;
pub mod ctx_set;
pub mod direct_endpoint;
pub mod forward_auth;
pub mod ip_restriction;
pub mod jwt_auth;
pub mod key_auth;
pub mod mock;
pub mod openid_connect;
mod plugin_store;
pub mod proxy_rewrite;
pub mod rate_limit;
pub mod real_ip;
pub mod request_restriction;
pub mod response_rewrite;

pub use all_endpoint_status::AllEndpointStatus;
pub use bandwidth_limit::BandwidthLimit;
pub use basic_auth::BasicAuth;
pub use conf_handler_impl::create_plugin_handler;
pub use cors::Cors;
pub use csrf::Csrf;
pub use ctx_set::CtxSet;
pub use forward_auth::ForwardAuth;
pub use ip_restriction::IpRestriction;
pub use jwt_auth::JwtAuth;
pub use key_auth::KeyAuth;
pub use mock::Mock;
pub use openid_connect::OpenidConnect;
pub use plugin_store::{get_global_plugin_store, PluginStore};
pub use proxy_rewrite::ProxyRewrite;
pub use rate_limit::RateLimit;
pub use real_ip::RealIp;
pub use request_restriction::RequestRestriction;
pub use response_rewrite::ResponseRewrite;

// Re-export plugin configs from types
pub use crate::types::resources::edgion_plugins::{
    AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CorsConfig, CsrfConfig, CtxSetConfig,
    ForwardAuthConfig, IpRestrictionConfig, JwtAuthConfig, KeyAuthConfig, MockConfig, OpenidConnectConfig,
    ProxyRewriteConfig, RateLimitConfig, RealIpConfig, RequestRestrictionConfig, ResponseRewriteConfig,
};
