//! Plugin store module for EdgionPlugins resources

pub mod all_endpoint_status;
pub mod bandwidth_limit;
pub mod basic_auth;
pub mod canary;
pub mod common;
mod conf_handler_impl;
pub mod cors;
pub mod csrf;
pub mod ctx_set;
pub mod direct_endpoint;
pub mod dsl;
pub mod dynamic_external_upstream;
pub mod dynamic_internal_upstream;
pub mod ext_proc;
pub mod fault_injection;
pub mod forward_auth;
pub mod global_access_control;
pub mod header_cert_auth;
pub mod hmac_auth;
pub mod ip_restriction;
pub mod json_schema_validation;
pub mod jwe_decrypt;
pub mod jwt_auth;
pub mod key_auth;
pub mod ldap_auth;
pub mod mock;
pub mod openid_connect;
mod plugin_store;
pub mod proxy_rewrite;
pub mod rate_limit;
pub mod rate_limit_redis;
pub mod real_ip;
pub mod region_route;
pub mod request_body_buffer;
pub mod request_mirror;
pub mod request_restriction;
pub mod response_rewrite;
pub mod trace_context;
pub mod wasm;

pub use all_endpoint_status::AllEndpointStatus;
pub use bandwidth_limit::BandwidthLimit;
pub use basic_auth::BasicAuth;
pub use canary::Canary;
pub use conf_handler_impl::create_plugin_handler;
pub use cors::Cors;
pub use csrf::Csrf;
pub use ctx_set::CtxSet;
pub use fault_injection::FaultInjection;
pub use forward_auth::ForwardAuth;
pub use global_access_control::GlobalAccessControl;
pub use header_cert_auth::HeaderCertAuth;
pub use hmac_auth::HmacAuth;
pub use ip_restriction::IpRestriction;
pub use json_schema_validation::JsonSchemaValidation;
pub use jwe_decrypt::JweDecrypt;
pub use jwt_auth::JwtAuth;
pub use key_auth::KeyAuth;
pub use ldap_auth::LdapAuth;
pub use mock::Mock;
pub use openid_connect::OpenidConnect;
pub use plugin_store::{compile_plugins, get_global_plugin_store, global_plugin_store, CompiledPlugins, PluginStore};
pub use proxy_rewrite::ProxyRewrite;
pub use rate_limit::RateLimit;
pub use rate_limit_redis::RateLimitRedis;
pub use real_ip::RealIp;
pub use region_route::RegionRoute;
pub use request_body_buffer::RequestBodyBuffer;
pub use request_mirror::RequestMirrorPlugin;
pub use request_restriction::RequestRestriction;
pub use response_rewrite::ResponseRewrite;
pub use wasm::WasmPlugin;

// Re-export plugin configs from types
pub use edgion_resources::resources::edgion_plugins::{
    AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CorsConfig, CsrfConfig, CtxSetConfig, DslConfig,
    DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig, FaultInjectionConfig, ForwardAuthConfig,
    HeaderCertAuthConfig, HmacAuthConfig, IpRestrictionConfig, JsonSchemaValidationConfig, JweDecryptConfig,
    JwtAuthConfig, KeyAuthConfig, LdapAuthConfig, MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitConfig,
    RateLimitRedisConfig, RealIpConfig, RegionRouteConfig, RequestRestrictionConfig, ResponseRewriteConfig,
};
