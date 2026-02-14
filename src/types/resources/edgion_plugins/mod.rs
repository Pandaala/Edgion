//! EdgionPlugins custom resource definition
//!
//! EdgionPlugins defines reusable plugin configurations that can be referenced by HTTPRoutes

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::plugins::PluginRuntime;

// Submodules
pub mod custom_plugins;
pub mod edgion_plugin;
pub mod entry;

pub mod plugin_configs;

#[cfg(test)]
mod tests;

// Re-exports
pub use edgion_plugin::EdgionPlugin;
pub use entry::{
    PluginEntry, RequestFilterEntry, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry,
    UpstreamResponseFilterEntry,
};
pub use plugin_configs::{
    AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CaseType, CertHeaderFormat, CertSourceMode,
    ConsumerBy, CorsConfig, CsrfConfig, CtxSetConfig, CtxVarRule, DebugAccessLogToHeaderConfig, DefaultAction,
    DirectEndpointConfig, DirectEndpointOnInvalid, DirectEndpointOnMissing, DomainTarget, DslConfig,
    DynUpstreamOnInvalid, DynUpstreamOnMissing, DynUpstreamOnNoMatch, DynamicExternalUpstreamConfig,
    DynamicInternalUpstreamConfig, EndpointAuthMethod, EndpointExtract, ExtUpstreamExtract, ExtUpstreamOnMissing,
    ExtUpstreamOnNoMatch, ExtractConfig, ForwardAuthConfig, HeaderActions, HeaderCertAuthConfig, HeaderEntry,
    HeaderRename, HmacAlgorithm, HmacAuthConfig, HmacCredential, HttpMethod, IpRestrictionConfig, IpSource,
    JweContentEncryption, JweDecryptConfig, JweKeyManagement, JwtAlgorithm, JwtAuthConfig, KeyAuthConfig, KeyMetadata,
    LdapAuthConfig, LimitHeaderNames, MappingConfig, MockConfig, OnMissing, OnMissingKey, OnRedisFailure,
    OpenidConnectConfig, ProxyRewriteConfig, RateLimitAlgorithm, RateLimitConfig, RateLimitPolicy,
    RateLimitRedisConfig, RealIpConfig, RegexUri, ReplaceConfig, RequestRestrictionConfig, ResolvedJweCredential,
    ResponseHeaderActions, ResponseHeaderEntry, ResponseRewriteConfig, RestrictionRule, RestrictionSource,
    RuleMatchMode, TransformConfig, TransformType, UnauthAction, UpstreamHeaderConfig, VerificationMode,
};

/// API group for EdgionPlugins
pub const EDGION_PLUGINS_GROUP: &str = "edgion.io";

/// Kind for EdgionPlugins
pub const EDGION_PLUGINS_KIND: &str = "EdgionPlugins";

/// EdgionPlugins defines reusable plugin configurations
#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
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

    /// Plugin runtime (runtime only, not serialized)
    /// This is computed from edgion_plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Preparse validation errors (runtime only, not serialized)
    /// Collected during preparse() for status reporting
    #[serde(skip)]
    #[schemars(skip)]
    pub preparse_errors: Vec<String>,
}

/// Status of EdgionPlugins
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionPluginsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl Default for EdgionPluginsSpec {
    fn default() -> Self {
        Self {
            request_plugins: None,
            upstream_response_filter_plugins: None,
            upstream_response_body_filter_plugins: None,
            upstream_response_plugins: None,
            plugin_runtime: Arc::new(PluginRuntime::new()),
            preparse_errors: Vec::new(),
        }
    }
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

    /// Preparse after deserialization to populate runtime fields
    ///
    /// This method should be called after deserializing EdgionPlugins from YAML/JSON
    /// to populate the runtime-only plugin_runtime field.
    ///
    /// Validation errors from plugin configs are collected and stored in preparse_errors
    /// for status reporting.
    pub fn preparse(&mut self) {
        let mut runtime = PluginRuntime::new();
        let mut errors = Vec::new();
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");

        if let Some(request_plugins) = &self.spec.request_plugins {
            let request_errors = runtime.add_from_request_filters(request_plugins, namespace);
            errors.extend(request_errors);
        }

        if let Some(upstream_response_filter_plugins) = &self.spec.upstream_response_filter_plugins {
            let filter_errors = runtime.add_from_upstream_response_filters(upstream_response_filter_plugins, namespace);
            errors.extend(filter_errors);
        }

        if let Some(upstream_response_body_filter_plugins) = &self.spec.upstream_response_body_filter_plugins {
            let body_filter_errors =
                runtime.add_from_upstream_response_body_filters(upstream_response_body_filter_plugins, namespace);
            errors.extend(body_filter_errors);
        }

        if let Some(upstream_response_plugins) = &self.spec.upstream_response_plugins {
            let response_errors = runtime.add_from_upstream_responses(upstream_response_plugins, namespace);
            errors.extend(response_errors);
        }

        self.spec.plugin_runtime = Arc::new(runtime);
        self.spec.preparse_errors = errors;
    }

    /// Get preparse validation errors
    pub fn get_preparse_errors(&self) -> &[String] {
        &self.spec.preparse_errors
    }
}
