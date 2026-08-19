//! EdgionPlugins custom resource definition
//!
//! EdgionPlugins defines reusable plugin configurations that can be referenced by HTTPRoutes

use crate::resources::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod body_requirement;
pub mod conditions;
pub mod custom_plugins;
pub mod dye;
pub mod edgion_plugin;
pub mod entry;

pub mod access_decision;
pub mod plugin_configs;
pub mod plugin_validation;
pub mod validate;

#[cfg(test)]
mod tests;

// Re-exports
pub use access_decision::AccessDecision;
pub use body_requirement::{BodyReadFailureMode, BodyRequirement};
pub use dye::{sanitize_field_value, DyeOutcome, DyeRule, DyeSuite, DENIED_DYE_HEADER_NAMES};
pub use edgion_plugin::{BodyStage, EdgionPlugin};
pub use entry::{
    duplicate_alias_error, dye_set_clobber_error, PluginEntry, RequestFilterEntry, UpstreamResponseBodyFilterEntry,
    UpstreamResponseEntry, UpstreamResponseFilterEntry,
};
pub use plugin_configs::{
    bounded_structured_body_validation_errors, parse_json_instance, profile_json_value, CompiledJsonSchema,
    ParsedJsonInstance,
};
pub use plugin_configs::{
    compile_guard_pattern, A2aGatewayConfig, AccessLogExternField, AiBackendSelection, AiBackendSelectionConfig,
    AiBackendSlotSelector, AiGuardAction, AiGuardConfig, AiGuardEvaluator, AiGuardMatcher, AiGuardPiiDetector,
    AiGuardRejectionMode, AiGuardRole, AiGuardRule, AiModelHeaderMatch, AiModelPattern, AiModelRoute,
    AiModelRouteAction, AiModelRouteCandidate, AiModelRouteMatch, AiProxyConfig, AiTokenQuotaConfig, AiTokenQuotaLimit,
    AiTokenQuotaOnExceeded, AiTokenQuotaScope, AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig,
    BasicAuthSecretGroup, CaseType, CertHeaderFormat, CertSourceMode, CompiledJsonBodyRewrite,
    CompiledJsonPatchOperation, CompiledJsonPatchValue, CompiledTemplate, ConsumerBy, CorsConfig, CsrfConfig,
    CtxSetConfig, CtxVarRule, DebugAccessLogToHeaderConfig, DefaultAction, DirectEndpointConfig,
    DirectEndpointOnInvalid, DirectEndpointOnMissing, DomainTarget, DslConfig, DynUpstreamOnInvalid,
    DynUpstreamOnMissing, DynUpstreamOnNoMatch, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig,
    EndpointAuthMethod, EndpointExtract, ExtProcConfig, ExtUpstreamExtract, ExtUpstreamOnMissing, ExtUpstreamOnNoMatch,
    ExtractConfig, FaultHeaderEntry, FaultInjectionConfig, FormJsonDirection, FormJsonTransformConfig,
    ForwardAuthConfig, ForwardAuthDecision, GuardrailConfig, HashAlgorithm, HashCalcConfig, HeaderActions,
    HeaderCertAuthConfig, HeaderEntry, HeaderRename, HmacAlgorithm, HmacAuthConfig, HmacAuthSecretGroup,
    HmacCredential, HttpMethod, InstanceValidationError, IpRestrictionConfig, IpSource, JsonBodyRewrite,
    JsonLiteralValue, JsonPatchOperation, JsonSchemaValidationConfig, JsonValueProfile, JweContentEncryption,
    JweDecryptConfig, JweKeyManagement, JwtAlgorithm, JwtAuthConfig, JwtIssuerGroup, KeyAuthConfig, KeyMetadata,
    LdapAuthConfig, LimitHeaderNames, LogExtSource, LruCacheConfig, MappingConfig, MockConfig, OnMissing, OnMissingKey,
    OnRedisFailure, OnUnknownModel, OpenidConnectConfig, ProxyRewriteConfig, RateLimitAlgorithm, RateLimitConfig,
    RateLimitPolicy, RateLimitRedisConfig, RealIpConfig, RegexUri, RegionDef, RegionRouteConfig, ReplaceConfig,
    RequestBodyBufferConfig, RequestRestrictionConfig, ResolvedBasicAuthUser, ResolvedJweCredential,
    ResolvedJwtCredential, ResolvedJwtIssuerGroup, ResponseHeaderActions, ResponseHeaderEntry, ResponseRewriteConfig,
    RestrictionMode, RestrictionRule, RestrictionSource, RouteRule, RuleGroup, SanType, SchemaComplexityProfile,
    StrictJsonError, TemplateSegment, TemplateVarType, TransformConfig, TransformType, UnauthAction,
    UpstreamHeaderConfig, VerificationMode, A2A_AGENT_CARD_MAX_BYTES, ABSOLUTE_INSTANCE_MAX_BYTES,
    DEFAULT_INSTANCE_MAX_BYTES, MAX_JSON_INSTANCE_DEPTH, MAX_JSON_INSTANCE_NODES, MAX_JSON_PATCH_OPERATIONS,
    MAX_JSON_PATCH_PLAN_BYTES, MAX_JSON_POINTER_BYTES, MAX_JSON_POINTER_TOKENS,
};
pub use plugin_validation::PluginValidation;
pub use validate::{applicable_stages_by_name, StageSet, HTTP_PLUGIN_STAGE_EXCEPTIONS};

/// EdgionPlugins defines reusable plugin configurations
#[derive(CustomResource, Serialize, Debug, Default, Clone, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionPlugins",
    plural = "edgionplugins",
    shortname = "eplugins",
    namespaced,
    status = "EdgionPluginsStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionPluginsSpec {
    /// Request stage plugins (async)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_plugins: Option<Vec<RequestFilterEntry>>,

    /// Upstream response filter stage plugins (sync)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_filter_plugins: Option<Vec<UpstreamResponseFilterEntry>>,

    /// Upstream response body filter stage plugins (sync)
    /// These plugins run for each body chunk and can control bandwidth throttling
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_body_filter_plugins: Option<Vec<UpstreamResponseBodyFilterEntry>>,

    /// Upstream response stage plugins (async)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_plugins: Option<Vec<UpstreamResponseEntry>>,

    /// Access log extension declarations.
    ///
    /// **Not an execution stage.** These entries never run as filters; they are
    /// read when the access log entry is serialized. Modeling them as a
    /// request-stage filter would lose them on every short-circuit path (404,
    /// 403 from an earlier Deny, CORS preflight, ACME, redirect) — exactly the
    /// entries an auditor needs — and would race `CtxSet` for `from: ctx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_log_extern: Option<Vec<AccessLogExternField>>,

    /// Unrecognized key paths found in the submitted document. Populated only on
    /// the Kubernetes watcher ingest path (see `schema_check::strict_ingest_enabled`);
    /// never serialized, never in the CRD schema.
    #[serde(skip)]
    #[schemars(skip)]
    pub unknown_fields: Vec<String>,
}

/// Deserialize-only mirror of [`EdgionPluginsSpec`], carrying every field
/// except `unknown_fields` (which is derived by the walk, never present in
/// the wire document). See `tasks/todo/plugin-config-unknown-field-detection/02-implementation.md`
/// §4 for why this shadow exists: it lets `EdgionPluginsSpec` keep a derived
/// `Serialize`/`JsonSchema` while `Deserialize` is hand-written to run the
/// schema walk before the input is consumed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdgionPluginsSpecShadow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_plugins: Option<Vec<RequestFilterEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upstream_response_filter_plugins: Option<Vec<UpstreamResponseFilterEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upstream_response_body_filter_plugins: Option<Vec<UpstreamResponseBodyFilterEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upstream_response_plugins: Option<Vec<UpstreamResponseEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_log_extern: Option<Vec<AccessLogExternField>>,
}

impl From<EdgionPluginsSpecShadow> for EdgionPluginsSpec {
    fn from(shadow: EdgionPluginsSpecShadow) -> Self {
        EdgionPluginsSpec {
            request_plugins: shadow.request_plugins,
            upstream_response_filter_plugins: shadow.upstream_response_filter_plugins,
            upstream_response_body_filter_plugins: shadow.upstream_response_body_filter_plugins,
            upstream_response_plugins: shadow.upstream_response_plugins,
            access_log_extern: shadow.access_log_extern,
            unknown_fields: Vec::new(),
        }
    }
}

/// Schema used by the unknown-key walk, built once (`schema_for!` is not
/// cheap and this runs on every Kubernetes-mode ingest).
///
/// `pub` (not module-private): the FileSystem/etcd loader seam
/// (`edgion-controller/src/conf_mgr/conf_center/common.rs`) and the Admin API
/// write handlers (`edgion-controller/src/api/common.rs`) run the same walk
/// against this schema outside the Kubernetes-only `strict_ingest_enabled`
/// gate — see Phase 2b in
/// `tasks/todo/plugin-config-unknown-field-detection/02-implementation.md` §5 / R-11.
pub fn edgion_plugins_spec_schema() -> &'static schemars::Schema {
    static SCHEMA: std::sync::OnceLock<schemars::Schema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| schemars::schema_for!(EdgionPluginsSpec))
}

impl<'de> Deserialize<'de> for EdgionPluginsSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Fast path: non-Kubernetes ingest (FileSystem/etcd loaders, the
        // gateway's conf_sync consumer, tests) pays neither the `Value`
        // round-trip nor the walk. See `schema_check::strict_ingest_enabled`
        // for why this is gated to Kubernetes specifically.
        if !crate::schema_check::strict_ingest_enabled() {
            return Ok(EdgionPluginsSpecShadow::deserialize(d)?.into());
        }
        let value = serde_json::Value::deserialize(d)?;
        let unknown = crate::schema_check::unknown_key_paths(&value, edgion_plugins_spec_schema());
        let mut spec: EdgionPluginsSpec = EdgionPluginsSpecShadow::deserialize(value)
            .map_err(serde::de::Error::custom)?
            .into();
        spec.unknown_fields = unknown;
        Ok(spec)
    }
}

/// Status of EdgionPlugins
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionPluginsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl EdgionPlugins {
    /// Get the namespace of this resource
    pub fn namespace(&self) -> Option<&str> {
        self.metadata.namespace.as_deref()
    }

    /// Get the name of this resource
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("")
    }

    /// Check if this plugin has any plugins defined
    pub fn has_plugins(&self) -> bool {
        self.spec.request_plugins.as_ref().is_some_and(|p| !p.is_empty())
            || self
                .spec
                .upstream_response_filter_plugins
                .as_ref()
                .is_some_and(|p| !p.is_empty())
            || self
                .spec
                .upstream_response_body_filter_plugins
                .as_ref()
                .is_some_and(|p| !p.is_empty())
            || self
                .spec
                .upstream_response_plugins
                .as_ref()
                .is_some_and(|p| !p.is_empty())
    }

    /// Get the total number of plugins
    pub fn plugin_count(&self) -> usize {
        let request_count = self.spec.request_plugins.as_ref().map_or(0, |p| p.len());
        let filter_count = self
            .spec
            .upstream_response_filter_plugins
            .as_ref()
            .map_or(0, |p| p.len());
        let body_filter_count = self
            .spec
            .upstream_response_body_filter_plugins
            .as_ref()
            .map_or(0, |p| p.len());
        let response_count = self.spec.upstream_response_plugins.as_ref().map_or(0, |p| p.len());
        request_count + filter_count + body_filter_count + response_count
    }

    /// Get request filter entries as a slice
    pub fn request_filter_entries(&self) -> &[RequestFilterEntry] {
        self.spec.request_plugins.as_deref().unwrap_or(&[])
    }

    /// Get upstream response filter entries as a slice
    pub fn upstream_response_filter_entries(&self) -> &[UpstreamResponseFilterEntry] {
        self.spec.upstream_response_filter_plugins.as_deref().unwrap_or(&[])
    }

    /// Get upstream response body filter entries as a slice
    pub fn upstream_response_body_filter_entries(&self) -> &[UpstreamResponseBodyFilterEntry] {
        self.spec
            .upstream_response_body_filter_plugins
            .as_deref()
            .unwrap_or(&[])
    }

    /// Get upstream response entries as a slice
    pub fn upstream_response_entries(&self) -> &[UpstreamResponseEntry] {
        self.spec.upstream_response_plugins.as_deref().unwrap_or(&[])
    }

    /// Get the access log extension declarations as a slice.
    pub fn access_log_extern_fields(&self) -> &[AccessLogExternField] {
        self.spec.access_log_extern.as_deref().unwrap_or(&[])
    }
}
