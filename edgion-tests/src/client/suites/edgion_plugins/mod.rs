// EdgionPlugins Test Suite
//
// Tests for EdgionPlugins functionality (independent from Gateway tests)
// Corresponds to conf/EdgionPlugins/ directory structure

mod access_log_extern;
mod all_endpoint_status;
mod bandwidth_limit;
mod basic_auth;
mod body_plugins;
mod canary;
mod cors;
mod csrf;
mod ctx_set;
mod debug_access_log;
mod direct_endpoint;
mod dsl;
mod dynamic_external_upstream;
mod dynamic_internal_upstream;
mod edgion_config_data_managed;
mod edgion_config_data_status;
mod ext_proc;
mod fault_injection;
mod form_json_transform;
mod forward_auth;
mod global_access_control;
mod global_connection_ip_restriction;
mod global_plugins_snapshot;
mod header_cert_auth;
mod hmac_auth;
mod ip_restriction;
mod ip_restriction_partial_good;
mod ip_restriction_refs;
mod json_schema_validation;
mod jwe_decrypt;
mod jwt_auth;
mod key_auth;
mod ldap_auth;
mod ldap_auth_tls;
mod matched_route_meta;
mod openid_connect;
mod path_normalization;
mod plugin_condition;
mod plugin_validation_status;
mod proxy_rewrite;
mod rate_limit;
mod rate_limit_redis;
mod real_ip;
mod region_route;
mod request_mirror;
mod request_restriction;
mod resolved_refs;
mod response_rewrite;
mod trace_context;
mod wasm;
mod webhook_key_get;

pub use access_log_extern::AccessLogExternTestSuite;
pub use all_endpoint_status::AllEndpointStatusTestSuite;
pub use bandwidth_limit::BandwidthLimitTestSuite;
pub use basic_auth::{
    BasicAuthDynamicGroupedInitialTestSuite, BasicAuthDynamicGroupedUpdateTestSuite, BasicAuthDynamicInitialTestSuite,
    BasicAuthDynamicUpdateTestSuite, BasicAuthTestSuite,
};
pub use body_plugins::{
    BodyDynamicCappedTestSuite, BodyDynamicInitialTestSuite, BodyDynamicRejectedTestSuite, BodyDynamicUpdateTestSuite,
    BodyPluginsTestSuite,
};
pub use canary::{CanaryHotSwapFinalTestSuite, CanaryHotSwapInitialTestSuite, CanaryTestSuite};
pub use cors::CorsPluginTestSuite;
pub use csrf::CsrfTestSuite;
pub use ctx_set::CtxSetTestSuite;
pub use debug_access_log::PluginLogsTestSuite;
pub use direct_endpoint::DirectEndpointTestSuite;
pub use dsl::{
    DslBodyTestSuite, DslDynamicInitialTestSuite, DslDynamicUpdateTestSuite, DslGrpcErrorTestSuite, DslTestSuite,
};
pub use dynamic_external_upstream::DynamicExternalUpstreamTestSuite;
pub use dynamic_internal_upstream::DynamicInternalUpstreamTestSuite;
pub use edgion_config_data_managed::EdgionConfigDataManagedTestSuite;
pub use edgion_config_data_status::{
    EdgionConfigDataFlapInitialTestSuite, EdgionConfigDataFlapInvalidTestSuite, EdgionConfigDataFlapValidTestSuite,
    EdgionConfigDataStatusTestSuite,
};
pub use ext_proc::ExtProcTestSuite;
pub use fault_injection::FaultInjectionTestSuite;
pub use form_json_transform::{FormJsonTransformDynamicFormTestSuite, FormJsonTransformDynamicJsonTestSuite};
pub use forward_auth::ForwardAuthTestSuite;
pub use global_access_control::GlobalAccessControlTestSuite;
pub use global_connection_ip_restriction::GlobalConnectionIpRestrictionTestSuite;
pub use global_plugins_snapshot::GlobalPluginsSnapshotTestSuite;
pub use header_cert_auth::HeaderCertAuthTestSuite;
pub use hmac_auth::HmacAuthTestSuite;
pub use ip_restriction::IpRestrictionPluginTestSuite;
pub use ip_restriction_partial_good::IpRestrictionPartialGoodPluginTestSuite;
pub use ip_restriction_refs::IpRestrictionRefsPluginTestSuite;
pub use json_schema_validation::{
    JsonSchemaValidationDynamicInvalidTestSuite, JsonSchemaValidationDynamicV1TestSuite,
    JsonSchemaValidationDynamicV2TestSuite, JsonSchemaValidationTestSuite,
};
pub use jwe_decrypt::JweDecryptTestSuite;
pub use jwt_auth::JwtAuthTestSuite;
pub use key_auth::KeyAuthTestSuite;
pub use ldap_auth::LdapAuthTestSuite;
pub use ldap_auth_tls::LdapAuthTlsTestSuite;
pub use matched_route_meta::MatchedRouteMetaTestSuite;
pub use openid_connect::OpenidConnectTestSuite;
pub use path_normalization::{
    PathNormalizationForwardViewTestSuite, PathNormalizationPreserveParamTestSuite, PathNormalizationTestSuite,
};
pub use plugin_condition::{AllConditionsTestSuite, PluginConditionTestSuite};
pub use plugin_validation_status::PluginValidationStatusTestSuite;
pub use proxy_rewrite::ProxyRewriteTestSuite;
pub use rate_limit::{RateLimitDynamicInitialTestSuite, RateLimitDynamicUpdateTestSuite, RateLimitTestSuite};
pub use rate_limit_redis::RateLimitRedisTestSuite;
pub use real_ip::RealIpPluginTestSuite;
pub use region_route::RegionRouteTestSuite;
pub use request_mirror::RequestMirrorTestSuite;
pub use request_restriction::RequestRestrictionTestSuite;
pub use resolved_refs::ResolvedRefsTestSuite;
pub use response_rewrite::ResponseRewriteTestSuite;
pub use trace_context::TraceContextPluginTestSuite;
pub use wasm::WasmPluginTestSuite;
pub use webhook_key_get::WebhookKeyGetTestSuite;
