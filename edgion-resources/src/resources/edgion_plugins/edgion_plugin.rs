//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use edgion_dsl::bytecode::BuiltinId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::dye::DyeOutcome;
use super::plugin_configs::{
    AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CanaryConfig, CorsConfig, CsrfConfig, CtxSetConfig,
    DebugAccessLogToHeaderConfig, DirectEndpointConfig, DslConfig, DynamicExternalUpstreamConfig,
    DynamicInternalUpstreamConfig, ExtProcConfig, FaultInjectionConfig, ForwardAuthConfig, GlobalAccessControlConfig,
    HeaderCertAuthConfig, HmacAuthConfig, IpRestrictionConfig, JweDecryptConfig, JwtAuthConfig, KeyAuthConfig,
    LdapAuthConfig, MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitConfig, RateLimitRedisConfig,
    RealIpConfig, RegionRouteConfig, RequestMirrorPluginConfig, RequestRestrictionConfig, ResponseRewriteConfig,
    TraceContextConfig, WasmConfig,
};
use crate::resources::http_route::{HTTPHeaderFilter, HTTPRequestRedirectFilter, HTTPURLRewriteFilter};

const DYE_SELECTION: &[DyeOutcome] = &[DyeOutcome::Success, DyeOutcome::Fallback, DyeOutcome::Reject];
const DYE_SELECTION_FALLBACK_ONLY: &[DyeOutcome] = &[DyeOutcome::Success, DyeOutcome::Fallback];
const DYE_FAIL_BOTH: &[DyeOutcome] = &[DyeOutcome::FailOpen, DyeOutcome::FailClose];
const DYE_FAIL_CLOSE_ONLY: &[DyeOutcome] = &[DyeOutcome::FailClose];

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
    RequestMirror(RequestMirrorPluginConfig),

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
    // Waf(WafConfig),
    // Cache(CacheConfig),
    // Transform(TransformConfig),
    // ...
}

/// Which async plugin stage a body requirement is being asked about.
/// `req.body` needs the request body (request stage); `resp.body` needs the
/// response body (upstream-response stage) — a plugin can consume one, both, or
/// neither depending on stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStage {
    Request,
    Response,
}

impl EdgionPlugin {
    /// The plugin's intrinsic body requirement, derived from its own config, for
    /// the given async body stage.
    ///
    /// This is the single source of truth for "does this plugin consume the
    /// request/response body at this stage (in this configuration)?" — used by
    /// both the controller validator and the gateway runtime, so they always
    /// agree. A plugin becomes body-aware by adding an arm here; everything
    /// else returns `None`, so an operator `body:` on a non-consuming plugin is
    /// rejected at preparse and never wraps it in a body gate.
    pub fn body_requirement(&self, stage: BodyStage) -> Option<super::body_requirement::BodyRequirement> {
        use super::body_requirement::{BodyReadFailureMode, BodyRequirement};
        use EdgionPlugin::*;
        match self {
            // HMAC consumes the REQUEST body only, and only when Digest
            // verification is on. Never at the response stage.
            HmacAuth(cfg) if cfg.validate_request_body && stage == BodyStage::Request => {
                Some(BodyRequirement::fail_close())
            }
            // DSL consumes a body only when its script references the matching
            // builtin at this stage. Fail policy is derived from the script's
            // own errorPolicy (Ignore = fail-open; Deny/DenyWith = fail-close) —
            // do NOT use fail_close() here (DSL defaults to fail-open).
            Dsl(cfg) => {
                let script = cfg.compile_and_validate().ok()?;
                let wanted = match stage {
                    BodyStage::Request => BuiltinId::ReqBody,
                    BodyStage::Response => BuiltinId::RespBody,
                };
                if !script.references_builtin(wanted) {
                    return None;
                }
                let mode = match cfg.error_policy {
                    Some(super::plugin_configs::DslErrorPolicy::Deny)
                    | Some(super::plugin_configs::DslErrorPolicy::DenyWith { .. }) => BodyReadFailureMode::FailClose,
                    // None defaults to Ignore (fail-open).
                    _ => BodyReadFailureMode::FailOpen,
                };
                Some(BodyRequirement::with_failure_mode(mode))
            }
            // Exhaustive (no `_`, matching `type_name`/`validation_error`): a new
            // body-consuming plugin MUST add itself above, or this fails to
            // compile — it can never silently default to "no body".
            HmacAuth(_)
            | RequestHeaderModifier(_)
            | ResponseHeaderModifier(_)
            | RequestRedirect(_)
            | UrlRewrite(_)
            | RequestMirror(_)
            | BasicAuth(_)
            | Cors(_)
            | Csrf(_)
            | IpRestriction(_)
            | JwtAuth(_)
            | JweDecrypt(_)
            | HeaderCertAuth(_)
            | KeyAuth(_)
            | LdapAuth(_)
            | Mock(_)
            | DebugAccessLogToHeader(_)
            | ProxyRewrite(_)
            | RequestRestriction(_)
            | ResponseRewrite(_)
            | RateLimit(_)
            | RateLimitRedis(_)
            | CtxSet(_)
            | RealIp(_)
            | FaultInjection(_)
            | ForwardAuth(_)
            | OpenidConnect(_)
            | BandwidthLimit(_)
            | DirectEndpoint(_)
            | AllEndpointStatus(_)
            | DynamicInternalUpstream(_)
            | DynamicExternalUpstream(_)
            | RegionRoute(_)
            | TraceContext(_)
            | ExtProc(_)
            | GlobalAccessControl(_)
            | Canary(_)
            | Wasm(_) => None,
        }
    }

    /// Whether this plugin accepts an operator `body:` configuration at the
    /// given stage (i.e. it consumes the body in its current config at that
    /// stage). Equivalent to `body_requirement(stage).is_some()`.
    pub fn accepts_body(&self, stage: BodyStage) -> bool {
        self.body_requirement(stage).is_some()
    }

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

    /// Outcomes this plugin type records (`record_outcome` call sites exist).
    /// The dye capability table: configuring an outcome not listed here is an
    /// admission error (no silent no-op — the `body:` precedent). Grows as
    /// wiring grows.
    pub fn dye_capabilities(&self) -> &'static [DyeOutcome] {
        match self {
            // Selection plugins: success + fallback/reject branches.
            EdgionPlugin::Canary(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::DirectEndpoint(_) => DYE_SELECTION,
            // region_route: no reject capability until the Rejected-silent-GoodNext
            // pre-existing bug is fixed (see spec, wiring notes).
            EdgionPlugin::RegionRoute(_) => DYE_SELECTION_FALLBACK_ONLY,
            // Dependency-failure plugins.
            EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::Wasm(_)
            | EdgionPlugin::Dsl(_)
            | EdgionPlugin::OpenidConnect(_) => DYE_FAIL_BOTH,
            // forward_auth/ldap_auth: fixed fail-close by policy (auth class never fails
            // open — the degrade switch was removed, see failure-policy-freeze design).
            EdgionPlugin::ForwardAuth(_) | EdgionPlugin::LdapAuth(_) => DYE_FAIL_CLOSE_ONLY,
            _ => &[],
        }
    }

    /// Stable snake_case value prefix, derived from `type_name()` once here so
    /// the wire contract cannot drift from the type list.
    pub fn dye_type_prefix(&self) -> &'static str {
        match self {
            EdgionPlugin::Canary(_) => "canary",
            EdgionPlugin::DynamicInternalUpstream(_) => "dynamic_internal_upstream",
            EdgionPlugin::DynamicExternalUpstream(_) => "dynamic_external_upstream",
            EdgionPlugin::DirectEndpoint(_) => "direct_endpoint",
            EdgionPlugin::RegionRoute(_) => "region_route",
            EdgionPlugin::RateLimitRedis(_) => "rate_limit_redis",
            EdgionPlugin::ExtProc(_) => "ext_proc",
            EdgionPlugin::Wasm(_) => "wasm",
            EdgionPlugin::Dsl(_) => "dsl",
            EdgionPlugin::OpenidConnect(_) => "openid_connect",
            EdgionPlugin::ForwardAuth(_) => "forward_auth",
            EdgionPlugin::LdapAuth(_) => "ldap_auth",
            _ => "",
        }
    }

    /// Header-typed routing key sources, for the request-direction dye
    /// collision check (a dye name equal to a key source would strip the
    /// plugin's own routing input). Mirrors each selection plugin's OLD
    /// `key_collision_error` call site (see `key_collision_error` usages in
    /// `plugin_configs/{direct_endpoint,dynamic_external_upstream,
    /// dynamic_internal_upstream,region_route}.rs` before this method
    /// centralized the mapping) — this is a move of existing logic, not new
    /// policy.
    pub fn key_header_names(&self) -> Vec<String> {
        fn headers_of<'a>(keys: impl IntoIterator<Item = &'a crate::common::KeyGet>) -> Vec<String> {
            keys.into_iter()
                .filter_map(|k| match k {
                    crate::common::KeyGet::Header { name } => Some(name.clone()),
                    _ => None,
                })
                .collect()
        }
        match self {
            // DirectEndpoint / DynamicInternalUpstream / DynamicExternalUpstream:
            // a single `from: KeyGet` field.
            EdgionPlugin::DirectEndpoint(c) => headers_of(std::iter::once(&c.from)),
            EdgionPlugin::DynamicInternalUpstream(c) => headers_of(std::iter::once(&c.from)),
            EdgionPlugin::DynamicExternalUpstream(c) => headers_of(std::iter::once(&c.from)),
            // RegionRoute: every header-typed key source it reads — the shared
            // `key_get`, the hash passthrough `hash_key_get`, and each
            // `RouteByDirectRegion` rule's `key_from` — mirroring
            // `RegionRouteConfig::check_fields`.
            EdgionPlugin::RegionRoute(c) => {
                let mut names = headers_of(c.key_get.iter());
                if let Some(kgs) = &c.hash_key_get {
                    names.extend(headers_of(kgs.iter()));
                }
                for rule in &c.route_rules {
                    if let super::plugin_configs::RouteRule::RouteByDirectRegion { key_from } = rule {
                        names.extend(headers_of(key_from.iter()));
                    }
                }
                names
            }
            // Canary has no key-source configuration (profile selection is
            // driven by `activeProfile`/`activeProfileRef`, not a request key)
            // and was never a `key_collision_error` call site.
            _ => Vec::new(),
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
        };
        let p = EdgionPlugin::Canary(cfg);
        assert!(p.validation_error().unwrap().contains("activeProfile"));
    }
}

#[cfg(test)]
mod dye_capability_tests {
    use super::*;
    use crate::resources::edgion_plugins::dye::DyeOutcome;

    #[test]
    fn selection_plugins_declare_success() {
        use crate::resources::plugin_configs::{CanaryConfig, DynUpstreamTarget};
        use std::collections::HashMap;

        // CanaryConfig has no `Default` impl; construct directly (mirrors the
        // existing `valid_config()` helper in canary.rs's own tests).
        let cfg = CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([(
                "stable".to_string(),
                DynUpstreamTarget {
                    name: "app-v1".to_string(),
                    namespace: None,
                },
            )]),
            active_profile_ref: None,
            on_invalid: Default::default(),
        };
        let p = EdgionPlugin::Canary(cfg);
        assert!(p.dye_capabilities().contains(&DyeOutcome::Success));
        assert!(p.dye_capabilities().contains(&DyeOutcome::Fallback));
        assert!(p.dye_capabilities().contains(&DyeOutcome::Reject));
        assert_eq!(p.dye_type_prefix(), "canary");
    }

    #[test]
    fn unwired_plugin_declares_nothing() {
        use crate::resources::plugin_configs::CorsConfig;
        let p = EdgionPlugin::Cors(CorsConfig::default());
        assert!(p.dye_capabilities().is_empty());
    }

    #[test]
    fn type_prefix_is_snake_case_of_type_name() {
        use crate::resources::plugin_configs::RateLimitRedisConfig;
        let p = EdgionPlugin::RateLimitRedis(RateLimitRedisConfig::default());
        assert_eq!(p.dye_type_prefix(), "rate_limit_redis");
        assert!(p.dye_capabilities().contains(&DyeOutcome::FailOpen));
    }

    #[test]
    fn key_header_names_mirrors_single_from_field_plugins() {
        use crate::resources::plugin_configs::{
            DirectEndpointConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig,
        };

        // `KeyGet` tags are `#[serde(tag = "type", rename_all = "camelCase")]`,
        // so the `Header` variant renders as lowercase `header`.
        let direct: DirectEndpointConfig =
            serde_yaml::from_str("from:\n  type: header\n  name: X-Backend-Target\n").unwrap();
        assert_eq!(
            EdgionPlugin::DirectEndpoint(direct).key_header_names(),
            vec!["X-Backend-Target".to_string()]
        );

        let dyn_internal: DynamicInternalUpstreamConfig =
            serde_yaml::from_str("from:\n  type: header\n  name: X-Upstream\n").unwrap();
        assert_eq!(
            EdgionPlugin::DynamicInternalUpstream(dyn_internal).key_header_names(),
            vec!["X-Upstream".to_string()]
        );

        let dyn_external: DynamicExternalUpstreamConfig =
            serde_yaml::from_str("from:\n  type: header\n  name: X-External\ndomainMap: {}\n").unwrap();
        assert_eq!(
            EdgionPlugin::DynamicExternalUpstream(dyn_external).key_header_names(),
            vec!["X-External".to_string()]
        );

        // Non-header key source contributes nothing.
        let direct_ip: DirectEndpointConfig = serde_yaml::from_str("from:\n  type: clientIp\n").unwrap();
        assert!(EdgionPlugin::DirectEndpoint(direct_ip).key_header_names().is_empty());
    }

    #[test]
    fn key_header_names_collects_all_region_route_sources() {
        use crate::resources::plugin_configs::RegionRouteConfig;

        let cfg: RegionRouteConfig = serde_yaml::from_str(
            "myRegion: us\n\
             keyGet:\n  - type: header\n    name: X-Key\n\
             hashKeyGet:\n  - type: header\n    name: X-Hash\n\
             routeRules:\n  - type: RouteByDirectRegion\n    keyFrom:\n      - type: header\n        name: X-Region\n\
             regions: []\n",
        )
        .unwrap();
        let mut names = EdgionPlugin::RegionRoute(cfg).key_header_names();
        names.sort();
        assert_eq!(
            names,
            vec!["X-Hash".to_string(), "X-Key".to_string(), "X-Region".to_string()]
        );
        assert_eq!(
            EdgionPlugin::RegionRoute(RegionRouteConfig::default()).dye_capabilities(),
            &[DyeOutcome::Success, DyeOutcome::Fallback]
        );
    }
}

#[cfg(test)]
mod dsl_body_requirement_tests {
    use super::*;
    use crate::resources::edgion_plugins::body_requirement::BodyReadFailureMode;
    use crate::resources::edgion_plugins::plugin_configs::{DslConfig, DslErrorPolicy};

    fn dsl(source: &str, policy: Option<DslErrorPolicy>) -> EdgionPlugin {
        EdgionPlugin::Dsl(DslConfig {
            name: "t".into(),
            source: Some(source.into()),
            error_policy: policy,
            ..Default::default()
        })
    }

    #[test]
    fn dsl_req_body_gated_at_request_stage_only() {
        let p = dsl("let b = req.body", None);
        assert!(p.body_requirement(BodyStage::Request).is_some());
        assert!(p.body_requirement(BodyStage::Response).is_none());
    }

    #[test]
    fn dsl_resp_body_gated_at_response_stage_only() {
        let p = dsl("let b = resp.body", None);
        assert!(p.body_requirement(BodyStage::Response).is_some());
        assert!(p.body_requirement(BodyStage::Request).is_none());
    }

    #[test]
    fn dsl_header_only_never_gated() {
        let p = dsl("let h = req.header(\"X\")", None);
        assert!(p.body_requirement(BodyStage::Request).is_none());
        assert!(p.body_requirement(BodyStage::Response).is_none());
    }

    #[test]
    fn dsl_failure_mode_follows_error_policy() {
        // default (None ⇒ Ignore) ⇒ fail-open
        let open = dsl("let b = req.body", None);
        assert_eq!(
            open.body_requirement(BodyStage::Request).unwrap().on_read_failure,
            BodyReadFailureMode::FailOpen
        );
        // Deny ⇒ fail-close
        let close = dsl("let b = req.body", Some(DslErrorPolicy::Deny));
        assert_eq!(
            close.body_requirement(BodyStage::Request).unwrap().on_read_failure,
            BodyReadFailureMode::FailClose
        );
    }
}
