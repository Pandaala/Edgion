//! Gateway API standard plugins and custom Edgion plugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter,
    HTTPURLRewriteFilter, LocalObjectReference,
};

/// Plugin enum for all supported plugin types
///
/// Naming convention:
/// - Gateway API standard filters: keep original names (RequestHeaderModifier, etc.)
/// - Custom Edgion filters: use EdgionXxx naming (EdgionRateLimit, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "camelCase")]
pub enum EdgionPlugin {
    // ========== Gateway API standard filters ==========
    /// Request header modifier filter
    RequestHeaderModifier(HTTPHeaderFilter),
    /// Response header modifier filter
    ResponseHeaderModifier(HTTPHeaderFilter),
    /// Request redirect filter
    RequestRedirect(HTTPRequestRedirectFilter),
    /// URL rewrite filter
    UrlRewrite(HTTPURLRewriteFilter),
    /// Request mirror filter
    RequestMirror(HTTPRequestMirrorFilter),
    /// Extension reference filter
    ExtensionRef(LocalObjectReference),

    // ========== Custom Edgion filters ==========
    // TODO: Add custom Edgion filters here
    // EdgionRateLimit(RateLimitConfig),
    // EdgionCircuitBreaker(CircuitBreakerConfig),
    // EdgionAuth(AuthConfig),
    // EdgionWaf(WafConfig),
    // EdgionCache(CacheConfig),
    // EdgionTransform(TransformConfig),
    // ...
}

impl EdgionPlugin {
    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionPlugin::RequestHeaderModifier(_) => "RequestHeaderModifier",
            EdgionPlugin::ResponseHeaderModifier(_) => "ResponseHeaderModifier",
            EdgionPlugin::RequestRedirect(_) => "RequestRedirect",
            EdgionPlugin::UrlRewrite(_) => "UrlRewrite",
            EdgionPlugin::RequestMirror(_) => "RequestMirror",
            EdgionPlugin::ExtensionRef(_) => "ExtensionRef",
        }
    }
}

