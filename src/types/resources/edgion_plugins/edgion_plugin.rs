//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::plugin_configs::{
    BasicAuthConfig, CorsConfig, CsrfConfig, CtxSetterConfig, DebugAccessLogToHeaderConfig, IpRestrictionConfig,
    JwtAuthConfig, MockConfig, ProxyRewriteConfig, RateLimiterConfig, RequestRestrictionConfig, ResponseRewriteConfig,
};
use crate::types::resources::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter, HTTPURLRewriteFilter, LocalObjectReference,
};

/// Plugin enum for all supported plugin types
///
/// Naming convention:
/// - Gateway API standard plugins: keep original names (RequestHeaderModifier, etc.)
/// - Custom Edgion plugins: use EdgionXxx naming (EdgionRateLimit, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
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
    /// JWT Authentication filter (verify JWT in header/query/cookie)
    JwtAuth(JwtAuthConfig),
    /// Mock filter (return predefined responses for testing/prototyping)
    Mock(MockConfig),
    /// Debug Access Log to Header filter (for debugging)
    DebugAccessLogToHeader(DebugAccessLogToHeaderConfig),
    /// Proxy Rewrite filter (rewrite URI, Host, Method, Headers before forwarding to upstream)
    ProxyRewrite(ProxyRewriteConfig),
    /// Request Restriction filter (restrict access based on headers, cookies, query, path, method, referer)
    RequestRestriction(RequestRestrictionConfig),
    /// Response Rewrite filter (rewrite status code and headers before returning to client)
    ResponseRewrite(ResponseRewriteConfig),
    /// RateLimiter filter (CMS algorithm for high-performance rate limiting)
    RateLimiter(RateLimiterConfig),
    /// CtxSetter filter (set context variables from various sources with extraction, transformation, and mapping)
    CtxSetter(CtxSetterConfig),
    // TODO: Add more custom Edgion plugins here
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
            EdgionPlugin::JwtAuth(_) => "JwtAuth",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::DebugAccessLogToHeader(_) => "DebugAccessLogToHeader",
            EdgionPlugin::ProxyRewrite(_) => "ProxyRewrite",
            EdgionPlugin::RequestRestriction(_) => "RequestRestriction",
            EdgionPlugin::ResponseRewrite(_) => "ResponseRewrite",
            EdgionPlugin::RateLimiter(_) => "RateLimiter",
            EdgionPlugin::CtxSetter(_) => "CtxSetter",
        }
    }
}
