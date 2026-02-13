// EdgionPlugins Test Suite
//
// Tests for EdgionPlugins functionality (independent from Gateway tests)
// Corresponds to conf/EdgionPlugins/ directory structure

mod all_endpoint_status;
mod bandwidth_limit;
mod ctx_set;
mod debug_access_log;
mod direct_endpoint;
mod dsl;
mod dynamic_external_upstream;
mod dynamic_internal_upstream;
mod forward_auth;
mod jwe_decrypt;
mod jwt_auth;
mod key_auth;
mod ldap_auth;
mod openid_connect;
mod plugin_condition;
mod proxy_rewrite;
mod rate_limit;
mod real_ip;
mod request_restriction;
mod response_rewrite;
mod webhook_key_get;

pub use all_endpoint_status::AllEndpointStatusTestSuite;
pub use bandwidth_limit::BandwidthLimitTestSuite;
pub use ctx_set::CtxSetTestSuite;
pub use debug_access_log::PluginLogsTestSuite;
pub use direct_endpoint::DirectEndpointTestSuite;
pub use dsl::DslTestSuite;
pub use dynamic_external_upstream::DynamicExternalUpstreamTestSuite;
pub use dynamic_internal_upstream::DynamicInternalUpstreamTestSuite;
pub use forward_auth::ForwardAuthTestSuite;
pub use jwe_decrypt::JweDecryptTestSuite;
pub use jwt_auth::JwtAuthTestSuite;
pub use key_auth::KeyAuthTestSuite;
pub use ldap_auth::LdapAuthTestSuite;
pub use openid_connect::OpenidConnectTestSuite;
pub use plugin_condition::{AllConditionsTestSuite, PluginConditionTestSuite};
pub use proxy_rewrite::ProxyRewriteTestSuite;
pub use rate_limit::RateLimitTestSuite;
pub use real_ip::RealIpPluginTestSuite;
pub use request_restriction::RequestRestrictionTestSuite;
pub use response_rewrite::ResponseRewriteTestSuite;
pub use webhook_key_get::WebhookKeyGetTestSuite;
