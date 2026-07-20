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
pub use edgion_plugin::EdgionPlugin;
pub use entry::{
    dye_set_clobber_error, PluginEntry, RequestFilterEntry, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry,
    UpstreamResponseFilterEntry,
};
pub use plugin_configs::{
    AccessLogExternField, AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, BasicAuthSecretGroup,
    CaseType, CertHeaderFormat, CertSourceMode, CompiledTemplate, ConsumerBy, CorsConfig, CsrfConfig, CtxSetConfig,
    CtxVarRule, DebugAccessLogToHeaderConfig, DefaultAction, DirectEndpointConfig, DirectEndpointOnInvalid,
    DirectEndpointOnMissing, DomainTarget, DslConfig, DynUpstreamOnInvalid, DynUpstreamOnMissing, DynUpstreamOnNoMatch,
    DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig, EndpointAuthMethod, EndpointExtract, ExtProcConfig,
    ExtUpstreamExtract, ExtUpstreamOnMissing, ExtUpstreamOnNoMatch, ExtractConfig, FaultHeaderEntry,
    FaultInjectionConfig, ForwardAuthConfig, ForwardAuthDecision, HashAlgorithm, HashCalcConfig, HeaderActions,
    HeaderCertAuthConfig, HeaderEntry, HeaderRename, HmacAlgorithm, HmacAuthConfig, HmacAuthSecretGroup,
    HmacCredential, HttpMethod, IpRestrictionConfig, IpSource, JweContentEncryption, JweDecryptConfig,
    JweKeyManagement, JwtAlgorithm, JwtAuthConfig, JwtIssuerGroup, KeyAuthConfig, KeyMetadata, LdapAuthConfig,
    LimitHeaderNames, LogExtSource, LruCacheConfig, MappingConfig, MockConfig, OnMissing, OnMissingKey, OnRedisFailure,
    OpenidConnectConfig, ProxyRewriteConfig, RateLimitAlgorithm, RateLimitConfig, RateLimitPolicy,
    RateLimitRedisConfig, RealIpConfig, RegexUri, RegionDef, RegionRouteConfig, ReplaceConfig,
    RequestRestrictionConfig, ResolvedBasicAuthUser, ResolvedJweCredential, ResolvedJwtCredential,
    ResolvedJwtIssuerGroup, ResponseHeaderActions, ResponseHeaderEntry, ResponseRewriteConfig, RestrictionMode,
    RestrictionRule, RestrictionSource, RouteRule, RuleGroup, SanType, TemplateSegment, TemplateVarType,
    TransformConfig, TransformType, UnauthAction, UpstreamHeaderConfig, VerificationMode,
};
pub use plugin_validation::PluginValidation;

/// EdgionPlugins defines reusable plugin configurations
#[derive(CustomResource, Serialize, Deserialize, Debug, Default, Clone, JsonSchema)]
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
