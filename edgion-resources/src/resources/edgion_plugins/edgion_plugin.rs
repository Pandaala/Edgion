//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::plugin_configs::{
    AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CanaryConfig, CorsConfig, CsrfConfig, CtxSetConfig,
    DebugAccessLogToHeaderConfig, DirectEndpointConfig, DslConfig, DynamicExternalUpstreamConfig,
    DynamicInternalUpstreamConfig, ExtProcConfig, FaultInjectionConfig, ForwardAuthConfig, GlobalAccessControlConfig,
    HeaderCertAuthConfig, HmacAuthConfig, IpRestrictionConfig, JweDecryptConfig, JwtAuthConfig, KeyAuthConfig,
    LdapAuthConfig, MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitConfig, RateLimitRedisConfig,
    RealIpConfig, RegionRouteConfig, RequestRestrictionConfig, ResponseRewriteConfig, TraceContextConfig, WasmConfig,
};
use crate::resources::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter, HTTPURLRewriteFilter,
};

/// Plugin enum for all supported plugin types
///
/// Naming convention:
/// - Variant names are the user-facing `type:` values (serde `tag = "type"`), so
///   they carry NO `Edgion` prefix — e.g. `RateLimit`, `DynamicInternalUpstream`.
/// - Gateway API standard plugins keep their spec names (RequestHeaderModifier, etc.);
///   custom Edgion plugins use a plain PascalCase name (RateLimit, CtxSet, etc.).
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
    /// JWE Decrypt filter (decrypt compact JWE from request header)
    JweDecrypt(JweDecryptConfig),
    /// HMAC Authentication filter (HTTP Signature with HMAC-SHA2)
    HmacAuth(HmacAuthConfig),
    /// Header/Connection certificate authentication filter
    HeaderCertAuth(HeaderCertAuthConfig),
    /// Key Authentication filter (API Key in header/query)
    KeyAuth(KeyAuthConfig),
    /// LDAP Authentication filter (username/password bind to LDAP server)
    LdapAuth(LdapAuthConfig),
    /// Mock filter (return predefined responses for testing/prototyping)
    Mock(MockConfig),
    /// Fault Injection (chaos): probabilistic delay and/or abort.
    FaultInjection(FaultInjectionConfig),
    /// Debug Access Log to Header filter (for debugging)
    DebugAccessLogToHeader(DebugAccessLogToHeaderConfig),
    /// Proxy Rewrite filter (rewrite URI, Host, Method, Headers before forwarding to upstream)
    ProxyRewrite(ProxyRewriteConfig),
    /// Request Restriction filter (restrict access based on headers, cookies, query, path, method, referer)
    RequestRestriction(RequestRestrictionConfig),
    /// Response Rewrite filter (rewrite status code and headers before returning to client)
    ResponseRewrite(ResponseRewriteConfig),
    /// RateLimit filter (CMS algorithm for high-performance rate limiting)
    RateLimit(RateLimitConfig),
    /// RateLimitRedis filter (Redis-based precise cluster-wide rate limiting)
    RateLimitRedis(RateLimitRedisConfig),
    /// CtxSet filter (set context variables from various sources with extraction, transformation, and mapping)
    CtxSet(CtxSetConfig),
    /// RealIp filter (extract real client IP from headers with trusted proxy support)
    RealIp(RealIpConfig),
    /// ForwardAuth filter (forward request to external auth service for authentication)
    ForwardAuth(ForwardAuthConfig),
    /// OpenID Connect filter (OIDC / OAuth 2.0 authentication)
    OpenidConnect(Box<OpenidConnectConfig>),
    /// BandwidthLimit filter (limit downstream response bandwidth per second)
    BandwidthLimit(BandwidthLimitConfig),
    /// DirectEndpoint filter (route to specific endpoint, bypassing LB)
    DirectEndpoint(DirectEndpointConfig),
    /// AllEndpointStatus filter (query all backend endpoints and return aggregated status)
    AllEndpointStatus(AllEndpointStatusConfig),
    /// DynamicInternalUpstream filter (route to specific BackendRef, bypass weighted selection)
    DynamicInternalUpstream(DynamicInternalUpstreamConfig),
    /// DynamicExternalUpstream filter (route to external domain via domainMap)
    DynamicExternalUpstream(DynamicExternalUpstreamConfig),
    /// DSL plugin — custom inline scripting with sandboxed VM execution
    Dsl(DslConfig),
    /// RegionRoute filter (consistent hash-range multi-region routing with optional failover)
    RegionRoute(RegionRouteConfig),
    /// TraceContext filter (W3C trace-context propagation)
    TraceContext(TraceContextConfig),
    /// ExtProc filter (external processing over a bidirectional gRPC stream)
    ExtProc(ExtProcConfig),
    /// GlobalAccessControl filter (L7 allow-list: IP/CIDR and header/cookie/... composite, OR groups)
    GlobalAccessControl(GlobalAccessControlConfig),
    /// Canary filter (100% traffic to one named backend, switchable via a Selector EdgionConfigData)
    Canary(CanaryConfig),
    /// Wasm filter (proxy-wasm ABI guest module executed via wasmtime)
    Wasm(WasmConfig),
    // TODO: Add more custom Edgion plugins here (no `Edgion` prefix on the variant name)
    // CircuitBreaker(CircuitBreakerConfig),
    // Waf(WafConfig),
    // Cache(CacheConfig),
    // Transform(TransformConfig),
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
            EdgionPlugin::BasicAuth(_) => "BasicAuth",
            EdgionPlugin::Cors(_) => "Cors",
            EdgionPlugin::Csrf(_) => "Csrf",
            EdgionPlugin::IpRestriction(_) => "IpRestriction",
            EdgionPlugin::JwtAuth(_) => "JwtAuth",
            EdgionPlugin::JweDecrypt(_) => "JweDecrypt",
            EdgionPlugin::HmacAuth(_) => "HmacAuth",
            EdgionPlugin::HeaderCertAuth(_) => "HeaderCertAuth",
            EdgionPlugin::KeyAuth(_) => "KeyAuth",
            EdgionPlugin::LdapAuth(_) => "LdapAuth",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::DebugAccessLogToHeader(_) => "DebugAccessLogToHeader",
            EdgionPlugin::ProxyRewrite(_) => "ProxyRewrite",
            EdgionPlugin::RequestRestriction(_) => "RequestRestriction",
            EdgionPlugin::ResponseRewrite(_) => "ResponseRewrite",
            EdgionPlugin::RateLimit(_) => "RateLimit",
            EdgionPlugin::RateLimitRedis(_) => "RateLimitRedis",
            EdgionPlugin::CtxSet(_) => "CtxSet",
            EdgionPlugin::RealIp(_) => "RealIp",
            EdgionPlugin::FaultInjection(_) => "FaultInjection",
            EdgionPlugin::ForwardAuth(_) => "ForwardAuth",
            EdgionPlugin::OpenidConnect(_) => "OpenidConnect",
            EdgionPlugin::BandwidthLimit(_) => "BandwidthLimit",
            EdgionPlugin::DirectEndpoint(_) => "DirectEndpoint",
            EdgionPlugin::AllEndpointStatus(_) => "AllEndpointStatus",
            EdgionPlugin::DynamicInternalUpstream(_) => "DynamicInternalUpstream",
            EdgionPlugin::DynamicExternalUpstream(_) => "DynamicExternalUpstream",
            EdgionPlugin::Dsl(_) => "Dsl",
            EdgionPlugin::RegionRoute(_) => "RegionRoute",
            EdgionPlugin::TraceContext(_) => "TraceContext",
            EdgionPlugin::ExtProc(_) => "ExtProc",
            EdgionPlugin::GlobalAccessControl(_) => "GlobalAccessControl",
            EdgionPlugin::Canary(_) => "Canary",
            EdgionPlugin::Wasm(_) => "Wasm",
        }
    }

    /// Stateless validation entry point for any plugin variant.
    ///
    /// This is the canonical preparse-time validation accessor. New variants
    /// MUST be added here; the exhaustive match guarantees compile-time coverage.
    pub fn validation_error(&self) -> Option<String> {
        use crate::resources::edgion_plugins::PluginValidation;
        match self {
            EdgionPlugin::CtxSet(c) => c.validation_error(),
            EdgionPlugin::DirectEndpoint(c) => c.validation_error(),
            EdgionPlugin::RegionRoute(c) => c.validation_error(),
            EdgionPlugin::DynamicInternalUpstream(c) => c.validation_error(),
            EdgionPlugin::DynamicExternalUpstream(c) => c.validation_error(),
            EdgionPlugin::KeyAuth(c) => c.validation_error(),
            EdgionPlugin::ResponseRewrite(c) => c.validation_error(),
            EdgionPlugin::ProxyRewrite(c) => c.validation_error(),
            EdgionPlugin::RequestRestriction(c) => c.validation_error(),
            EdgionPlugin::RateLimit(c) => c.validation_error(),
            EdgionPlugin::RateLimitRedis(c) => c.validation_error(),
            EdgionPlugin::Mock(c) => c.validation_error(),
            EdgionPlugin::Csrf(c) => c.validation_error(),
            EdgionPlugin::BasicAuth(c) => c.validation_error(),
            EdgionPlugin::JweDecrypt(c) => c.validation_error(),
            EdgionPlugin::HmacAuth(c) => c.validation_error(),
            EdgionPlugin::HeaderCertAuth(c) => c.validation_error(),
            EdgionPlugin::FaultInjection(c) => c.validation_error(),
            EdgionPlugin::ForwardAuth(c) => c.validation_error(),
            EdgionPlugin::OpenidConnect(c) => c.validation_error(),
            EdgionPlugin::BandwidthLimit(c) => c.validation_error(),
            EdgionPlugin::LdapAuth(c) => c.validation_error(),
            EdgionPlugin::AllEndpointStatus(c) => c.validation_error(),
            EdgionPlugin::Dsl(c) => c.validation_error(),
            EdgionPlugin::Cors(c) => c.validation_error(),
            EdgionPlugin::IpRestriction(c) => c.validation_error(),
            EdgionPlugin::RealIp(c) => c.validation_error(),
            EdgionPlugin::JwtAuth(c) => c.validation_error(),
            EdgionPlugin::DebugAccessLogToHeader(c) => c.validation_error(),
            EdgionPlugin::TraceContext(c) => c.validation_error(),
            EdgionPlugin::ExtProc(c) => c.validation_error(),
            EdgionPlugin::GlobalAccessControl(c) => c.validation_error(),
            EdgionPlugin::Canary(c) => c.validation_error(),
            EdgionPlugin::Wasm(c) => c.validation_error(),
            // Gateway API filter variants (foreign types): no PluginValidation impl.
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_) => None,
        }
    }
}

#[cfg(test)]
mod global_access_control_variant_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::GlobalAccessControlConfig;

    #[test]
    fn type_name_and_tag_roundtrip() {
        let json = serde_json::json!({
            "type": "GlobalAccessControl",
            "config": {
                "enable": true,
                "activeProfile": "normal",
                "profiles": { "normal": { "defaultAction": "allow", "ruleGroups": [] } }
            }
        });
        let p: EdgionPlugin = serde_json::from_value(json).unwrap();
        assert_eq!(p.type_name(), "GlobalAccessControl");
        assert!(matches!(p, EdgionPlugin::GlobalAccessControl(_)));
        // validation_error delegates to the config.
        assert!(p.validation_error().is_none());
    }

    #[test]
    fn validation_error_surfaces_bad_active_profile() {
        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "ghost".into(),
            profiles: std::collections::HashMap::from([(
                "normal".to_string(),
                crate::resources::edgion_plugins::plugin_configs::AllowProfile {
                    default_action: crate::resources::edgion_plugins::plugin_configs::DefaultAction::Allow,
                    rule_groups: vec![],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: crate::resources::edgion_plugins::plugin_configs::IpSource::RemoteAddr,
        };
        let p = EdgionPlugin::GlobalAccessControl(cfg);
        assert!(p.validation_error().unwrap().contains("activeProfile"));
    }
}

#[cfg(test)]
mod canary_variant_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};
    use std::collections::HashMap;

    fn target(name: &str) -> DynUpstreamTarget {
        DynUpstreamTarget {
            name: name.to_string(),
            namespace: None,
        }
    }

    #[test]
    fn type_name_and_tag_roundtrip() {
        let json = serde_json::json!({
            "type": "Canary",
            "config": {
                "enable": true,
                "activeProfile": "stable",
                "profiles": { "stable": { "name": "app-v1" } }
            }
        });
        let p: EdgionPlugin = serde_json::from_value(json).unwrap();
        assert_eq!(p.type_name(), "Canary");
        assert!(matches!(p, EdgionPlugin::Canary(_)));
        assert!(p.validation_error().is_none());
    }

    #[test]
    fn validation_error_surfaces_bad_active_profile() {
        let cfg = CanaryConfig {
            enable: true,
            active_profile: "ghost".to_string(),
            profiles: HashMap::from([("stable".to_string(), target("app-v1"))]),
            active_profile_ref: None,
            on_invalid: Default::default(),
            dye_headers: None,
        };
        let p = EdgionPlugin::Canary(cfg);
        assert!(p.validation_error().unwrap().contains("activeProfile"));
    }
}
