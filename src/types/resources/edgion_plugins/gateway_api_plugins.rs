//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter,
    HTTPURLRewriteFilter, LocalObjectReference,
};
use super::plugin_configs::{BasicAuthConfig, CorsConfig, CsrfConfig, DebugAccessLogToHeaderConfig, IpRestrictionConfig, MockConfig};

/// Plugin enum for all supported plugin types
///
/// Naming convention:
/// - Gateway API standard plugins: keep original names (RequestHeaderModifier, etc.)
/// - Custom Edgion plugins: use EdgionXxx naming (EdgionRateLimit, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "camelCase")]
pub enum EdgionPlugin {
    // ========== Gateway API standard plugins ==========
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

    // ========== Custom Edgion plugins ==========
    /// Basic Authentication filter
    BasicAuth(BasicAuthConfig),
    /// CORS (Cross-Origin Resource Sharing) filter
    Cors(CorsConfig),
    /// CSRF (Cross-Site Request Forgery) protection filter
    Csrf(CsrfConfig),
    /// IP Restriction filter (allow/deny based on IP address or CIDR)
    IpRestriction(IpRestrictionConfig),
    /// Mock filter (return predefined responses for testing/prototyping)
    Mock(MockConfig),
    /// Debug Access Log to Header filter (for debugging)
    DebugAccessLogToHeader(DebugAccessLogToHeaderConfig),
    // TODO: Add more custom Edgion plugins here
    // EdgionRateLimit(RateLimitConfig),
    // EdgionCircuitBreaker(CircuitBreakerConfig),
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
            EdgionPlugin::BasicAuth(_) => "BasicAuth",
            EdgionPlugin::Cors(_) => "Cors",
            EdgionPlugin::Csrf(_) => "Csrf",
            EdgionPlugin::IpRestriction(_) => "IpRestriction",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::DebugAccessLogToHeader(_) => "DebugAccessLogToHeader",
        }
    }
}
