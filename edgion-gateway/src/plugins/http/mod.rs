//! Plugin store module for EdgionPlugins resources

pub mod ai_guard;
pub mod ai_proxy;
pub mod all_endpoint_status;
pub mod bandwidth_limit;
pub mod basic_auth;
pub mod canary;
pub mod common;
mod conf_handler_impl;
pub mod cors;
pub mod credential_injector;
pub mod csrf;
pub mod ctx_set;
pub mod direct_endpoint;
pub mod dsl;
pub mod dynamic_external_upstream;
pub mod dynamic_internal_upstream;
pub mod ext_proc;
pub mod fault_injection;
pub mod form_json_transform;
pub mod forward_auth;
pub mod geoip_location;
pub mod graphql_operation_router;
pub mod graphql_rate_limit;
pub mod guardrail;
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
pub(crate) mod plugin_generation;
mod plugin_store;
pub mod proxy_rewrite;
pub mod rate_limit_local;
pub mod rate_limit_redis;
pub mod real_ip;
pub mod region_route;
pub mod request_access_policy;
pub mod request_body_buffer;
pub mod request_id;
pub mod request_mirror;
pub mod request_restriction;
pub mod response_rewrite;
pub mod waf;
pub mod wasm;
pub mod webhook_key_get;

pub use ai_guard::AiGuard;
pub use ai_proxy::AiProxy;
pub use all_endpoint_status::AllEndpointStatus;
pub use bandwidth_limit::BandwidthLimit;
pub use basic_auth::BasicAuth;
pub use canary::Canary;
pub use conf_handler_impl::create_plugin_handler;
pub use cors::Cors;
pub use credential_injector::CredentialInjector;
pub use csrf::Csrf;
pub use ctx_set::CtxSet;
pub use fault_injection::FaultInjection;
pub use form_json_transform::FormJsonTransform;
pub use forward_auth::ForwardAuth;
pub use geoip_location::GeoIpLocation;
pub use graphql_rate_limit::GraphqlRateLimit;
pub use guardrail::Guardrail;
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
pub use rate_limit_local::RateLimitLocal;
pub use rate_limit_redis::RateLimitRedis;
pub use real_ip::RealIp;
pub use region_route::RegionRoute;
pub use request_access_policy::RequestAccessPolicy;
pub use request_body_buffer::RequestBodyBuffer;
pub use request_mirror::RequestMirrorPlugin;
pub use request_restriction::RequestRestriction;
pub use response_rewrite::ResponseRewrite;
pub use wasm::WasmPlugin;
pub use webhook_key_get::WebhookKeyGet;

// Re-export plugin configs from types
pub use edgion_resources::resources::edgion_plugins::{
    AiProxyConfig, AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CorsConfig,
    CredentialInjectorConfig, CsrfConfig, CtxSetConfig, DslConfig, DynamicExternalUpstreamConfig,
    DynamicInternalUpstreamConfig, FaultInjectionConfig, ForwardAuthConfig, HeaderCertAuthConfig, HmacAuthConfig,
    IpRestrictionConfig, JsonSchemaValidationConfig, JweDecryptConfig, JwtAuthConfig, KeyAuthConfig, LdapAuthConfig,
    MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitLocalConfig, RateLimitRedisConfig, RealIpConfig,
    RegionRouteConfig, RequestRestrictionConfig, ResponseRewriteConfig,
};

pub mod grpc_json_transcoder;
