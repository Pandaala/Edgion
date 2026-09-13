pub mod access_log_extern;
mod ai_guard;
mod ai_proxy;
pub mod all_endpoint_status;
mod auth_header_projection;
mod bandwidth_limit;
mod basic_auth;
mod canary;
mod cors;
mod credential_injector;
mod csrf;
mod ctx_set;
mod debug_access_log;
mod direct_endpoint;
pub mod dsl;
mod dynamic_external_upstream;
mod dynamic_internal_upstream;
mod ext_proc;
mod fault_injection;
mod form_json_transform;
mod forward_auth;
pub mod geo_rule;
mod geoip_location;
mod guardrail;
mod header_cert_auth;
mod hmac_auth;
mod ip_restriction;
mod json_schema_validation;
mod jwe_decrypt;
mod jwt_auth;
mod key_auth;
mod ldap_auth;
mod linksys_ref;
mod lru_cache_config;
mod mock;
mod openid_connect;
mod proxy_rewrite;
mod rate_limit_local;
mod rate_limit_redis;
mod real_ip;
mod region_route;
mod request_access_policy;
mod request_body_buffer;
mod request_id;
mod request_mirror;
mod request_restriction;
mod response_rewrite;
mod tiny_ufo_ttl_cache_config;
mod upstream_fallback;
mod waf;
mod wasm;
mod wasm_oci;
mod webhook_key_get;

pub use super::conditions::{
    AllOfGroup, AnyOfClause, Condition, ConditionSet, ConditionValidationProfile, IpAccessMatch, SecretMatchCondition,
};
pub use super::mtls_field::SanType;
pub use access_log_extern::{is_blocked, AccessLogExternField, LogExtSource, ACCESS_LOG_BLOCKED_CTX_KEYS};
pub use ai_guard::{
    compile_guard_pattern, AiGuardAction, AiGuardConfig, AiGuardEvaluator, AiGuardMatcher, AiGuardPiiDetector,
    AiGuardRejectionMode, AiGuardRole, AiGuardRule, AI_GUARD_MAX_CONCURRENT, AI_GUARD_MAX_PATTERN_BYTES,
    AI_GUARD_MAX_REPLACEMENT_BYTES, AI_GUARD_MAX_REQUEST_BYTES, AI_GUARD_MAX_RULES,
    AI_GUARD_MAX_RULE_REPLACEMENT_BYTES, AI_GUARD_MAX_SPANS, AI_GUARD_MAX_SPAN_BYTES, AI_GUARD_MAX_TOTAL_SPAN_BYTES,
    AI_GUARD_REGEX_SIZE_LIMIT,
};
pub use ai_proxy::{
    AiBackendSelection, AiBackendSelectionConfig, AiBackendSlotSelector, AiModelHeaderMatch, AiModelPattern,
    AiModelRoute, AiModelRouteAction, AiModelRouteCandidate, AiModelRouteMatch, AiProxyConfig, AiTokenQuotaConfig,
    AiTokenQuotaLimit, AiTokenQuotaOnExceeded, AiTokenQuotaScope, OnUnknownModel, AI_MODEL_ROUTE_MAX_CANDIDATES,
    AI_MODEL_ROUTE_MAX_COUNT, AI_MODEL_ROUTE_MAX_HEADERS, AI_QUOTA_MAX_SAFE_INTEGER,
};
pub use all_endpoint_status::AllEndpointStatusConfig;
pub use auth_header_projection::{
    compile_claim_header_projection, compile_claim_header_projection_with_owned_destinations,
    ClaimHeaderProjectionEntry, ClaimHeaderProjectionPlan, DUPLICATE_HEADER_DESTINATION, INVALID_HEADER_DESTINATION,
};
pub use bandwidth_limit::BandwidthLimitConfig;
pub use basic_auth::{BasicAuthConfig, BasicAuthSecretGroup, ResolvedBasicAuthUser};
pub use canary::CanaryConfig;
pub use cors::CorsConfig;
pub use credential_injector::{
    CredentialInjectionTarget, CredentialInjectorConfig, ResolvedCredentialInjector,
    CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN, CREDENTIAL_INJECTOR_VALUE_PREFIX_MAX_LEN,
};
pub use csrf::CsrfConfig;
pub use ctx_set::{
    parse_secret_ref, secret_ref_var_names, template_has_secret_ref, validate_secret_ref_segments, CaseType,
    CompiledTemplate, CtxSetConfig, CtxVarRule, ExtractConfig, MappingConfig, ReplaceConfig, SecretRefParts,
    TemplateSegment, TemplateVarType, TransformConfig, TransformType,
};
pub use debug_access_log::DebugAccessLogToHeaderConfig;
pub use direct_endpoint::{DirectEndpointConfig, EndpointExtract};
pub use dsl::{DslConfig, DslErrorPolicy};
pub use dynamic_external_upstream::{DomainTarget, DynamicExternalUpstreamConfig, ExtUpstreamExtract};
pub use dynamic_internal_upstream::{
    DynUpstreamExtract, DynUpstreamRule, DynUpstreamTarget, DynamicInternalUpstreamConfig,
};
pub use ext_proc::{BodySendMode, ExtProcConfig, GrpcServiceConfig, HeaderSendMode, ProcessingMode};
pub use fault_injection::{
    AbortConfig, DelayConfig, FaultHeaderEntry, FaultInjectionConfig, MAX_DELAY_MS as FAULT_MAX_DELAY_MS,
};
pub use form_json_transform::{FormJsonDirection, FormJsonTransformConfig};
pub use forward_auth::{ForwardAuthConfig, ForwardAuthDecision};
pub use geo_rule::{GeoDimFacts, GeoRule, GeoRuleContext, GeoRuleGroup, GeoRuleKey, GeoRuleSource};
pub use geoip_location::{GeoDecision, GeoIpLocationConfig};
pub use guardrail::{
    GuardrailConfig, GUARDRAIL_MAX_CONCURRENT, GUARDRAIL_MAX_OVERLAP_BYTES, GUARDRAIL_MAX_REPLACEMENT_BYTES,
    GUARDRAIL_MAX_REQUEST_BYTES, GUARDRAIL_MAX_WINDOW_BYTES,
};
pub use header_cert_auth::{CertHeaderFormat, CertSourceMode, ConsumerBy, HeaderCertAuthConfig, UpstreamHeaderConfig};
pub use hmac_auth::{HmacAlgorithm, HmacAuthConfig, HmacAuthSecretGroup, HmacCredential, HmacMatchConfig};
pub use ip_restriction::{DefaultAction, IpRestrictionConfig, IpSource};
pub use json_schema_validation::{
    bounded_structured_body_validation_errors, parse_json_instance, profile_json_value, CompiledJsonSchema,
    InstanceValidationError, JsonSchemaValidationConfig, JsonValueProfile, ParsedJsonInstance, SchemaComplexityProfile,
    StrictJsonError, ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES, MAX_JSON_INSTANCE_DEPTH,
    MAX_JSON_INSTANCE_NODES,
};
pub use jwe_decrypt::{
    JweContentEncryption, JweDecryptConfig, JweKeyManagement, ResolvedJweCredential, ResolvedJweCredentialError,
};
pub use jwt_auth::{JwtAlgorithm, JwtAuthConfig, JwtIssuerGroup, ResolvedJwtCredential, ResolvedJwtIssuerGroup};
pub use key_auth::{KeyAuthConfig, KeyAuthSecretGroup, KeyMetadata};
pub use ldap_auth::LdapAuthConfig;
pub use linksys_ref::LinkSysRef;
pub use lru_cache_config::{LruCacheConfig, MAX_LRU_CACHE_CAPACITY, MAX_LRU_CACHE_SHARDS};
pub use mock::MockConfig;
pub use openid_connect::{EndpointAuthMethod, OpenidConnectConfig, UnauthAction, VerificationMode};
pub use proxy_rewrite::{
    CompiledJsonBodyRewrite, CompiledJsonPatchOperation, CompiledJsonPatchValue, HeaderActions, HeaderEntry,
    HttpMethod, JsonBodyRewrite, JsonLiteralValue, JsonPatchOperation, ProxyRewriteConfig, RegexUri,
    MAX_JSON_PATCH_OPERATIONS, MAX_JSON_PATCH_PLAN_BYTES, MAX_JSON_POINTER_BYTES, MAX_JSON_POINTER_TOKENS,
};
pub use rate_limit_local::{LimitHeaderNames, OnKeyMiss, OnLimitExceeded, RateLimitLocalConfig, RateLimitLocalScope};
pub use rate_limit_redis::{OnRedisFailure, RateLimitAlgorithm, RateLimitPolicy, RateLimitRedisConfig};
pub use real_ip::RealIpConfig;
pub use region_route::{
    HashAlgorithm, HashCalcConfig, RegionDef, RegionRouteConfig, RouteByKeyConfMatchMeta, RouteRule,
};
pub use request_access_policy::{
    AccessPathMatch, AccessPathMatchType, IpAccessRuleConfig, RequestAccessPolicyConfig, RequestAccessProfile,
    RequestAccessRule, RequestAccessRuleType, RequestAccessUrlAllowListRef, RequiredRuleGroup, UrlAccessCandidate,
    UrlAccessRuleConfig, UrlAllowListRefValidation, DEFAULT_MAX_REFERENCED_IP_RULES, DEFAULT_MAX_REFERENCED_URL_RULES,
    MAX_REFERENCED_IP_RULES, MAX_REFERENCED_URL_RULES,
};
pub use request_body_buffer::RequestBodyBufferConfig;
pub use request_id::*;
pub use request_mirror::RequestMirrorPluginConfig;
pub use request_restriction::{RequestRestrictionConfig, RestrictionMode};
pub use response_rewrite::{HeaderRename, ResponseHeaderActions, ResponseHeaderEntry, ResponseRewriteConfig};
pub use tiny_ufo_ttl_cache_config::{
    validate_tiny_ufo_effective_limits, TinyUfoTtlCacheConfig, MAX_TINY_UFO_CACHE_ENTRIES, MAX_TINY_UFO_LOGICAL_BYTES,
    TINY_UFO_WEIGHT_UNIT_BYTES,
};
pub use upstream_fallback::UpstreamFallback;
pub use waf::{WafConfig, WafMode};
pub use wasm::{
    normalize_callout_origin, normalize_callout_origin_quiet, WasmAuthHeaderSecretRef, WasmConfig, WasmFetchRetry,
    WasmFetchSpec, WasmRawBase64, WasmRawConfig, WasmRawJson, WasmRawString, WasmSourceSpec, INLINE_WARN_DECODED_BYTES,
    MAX_INLINE_DECODED_BYTES, MAX_WASM_RAW_CONFIG_BYTES,
};
pub use wasm_oci::{OciImageRef, OciPullPolicy, WasmOciSpec};
pub use webhook_key_get::{WebhookExtract, WebhookKeyGetConfig, WebhookVar};

/// Validate a security-sensitive template against the restricted variable
/// allow-list: only controller-resolved or non-client-controlled variables.
///
/// Shared by every plugin config whose outbound path/header templates are a
/// security boundary, so the allow-list has exactly one definition.
pub(super) fn validate_restricted_template(template: &str, field: &str) -> Option<String> {
    let compiled = match CompiledTemplate::compile(template) {
        Ok(compiled) => compiled,
        Err(error) => return Some(format!("{field}: {error}")),
    };
    if let Err(error) = validate_secret_ref_segments(&compiled) {
        return Some(format!("{field}: {error}"));
    }
    for segment in compiled.segments() {
        if let TemplateSegment::Variable { var_type, .. } = segment {
            if !matches!(
                var_type,
                TemplateVarType::Ctx
                    | TemplateVarType::RemoteIp
                    | TemplateVarType::MatchedHttpRoute
                    | TemplateVarType::MatchedGrpcRoute
                    | TemplateVarType::SecretRef
            ) {
                return Some(format!(
                    "{field} must not read client-controlled request input; use a trusted ctx, remoteIp, matched-route, or secretRef variable"
                ));
            }
        }
    }
    None
}
