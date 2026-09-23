//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use edgion_dsl::bytecode::BuiltinId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::dye::{DyeOutcome, DyeStage};
use super::plugin_configs::{
    AiGuardConfig, AiProxyConfig, AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CanaryConfig,
    CorsConfig, CredentialInjectorConfig, CsrfConfig, CtxSetConfig, DebugAccessLogToHeaderConfig, DirectEndpointConfig,
    DslConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig, ExtProcConfig, FaultInjectionConfig,
    FormJsonTransformConfig, ForwardAuthConfig, GeoIpLocationConfig, GuardrailConfig, HeaderCertAuthConfig,
    HmacAuthConfig, IpRestrictionConfig, JsonSchemaValidationConfig, JweDecryptConfig, JwtAuthConfig, KeyAuthConfig,
    LdapAuthConfig, MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitLocalConfig, RateLimitRedisConfig,
    RealIpConfig, RegionRouteConfig, RequestAccessPolicyConfig, RequestBodyBufferConfig, RequestIdConfig,
    RequestMirrorPluginConfig, RequestRestrictionConfig, ResponseRewriteConfig, WasmConfig, WebhookKeyGetConfig,
    DEFAULT_INSTANCE_MAX_BYTES,
};
use crate::resources::http_route::{HTTPHeaderFilter, HTTPRequestRedirectFilter, HTTPURLRewriteFilter};

const DYE_FAIL_BOTH: &[DyeOutcome] = &[DyeOutcome::FailOpen, DyeOutcome::FailClose];
const DYE_FAIL_OPEN_ONLY: &[DyeOutcome] = &[DyeOutcome::FailOpen];
const DYE_FAIL_CLOSE_ONLY: &[DyeOutcome] = &[DyeOutcome::FailClose];

fn selection_dye(success: bool, fallback: bool, reject: bool) -> &'static [DyeOutcome] {
    use DyeOutcome::*;
    match (success, fallback, reject) {
        (true, true, true) => &[Success, Fallback, Reject],
        (true, true, false) => &[Success, Fallback],
        (true, false, true) => &[Success, Reject],
        (true, false, false) => &[Success],
        (false, true, true) => &[Fallback, Reject],
        (false, true, false) => &[Fallback],
        (false, false, true) => &[Reject],
        (false, false, false) => &[],
    }
}

fn selection_policy_dye(success: bool, policies: &[super::UpstreamFallback]) -> &'static [DyeOutcome] {
    selection_dye(
        success,
        policies.iter().any(|p| matches!(p, super::UpstreamFallback::Fallback)),
        policies.iter().any(|p| matches!(p, super::UpstreamFallback::Reject)),
    )
}

/// Plugin enum for all supported plugin types
///
/// Naming convention:
/// - Variant names are the user-facing `type:` values (serde `tag = "type"`), so
///   they carry NO `Edgion` prefix — e.g. `RateLimitLocal`, `DynamicInternalUpstream`.
/// - Gateway API standard plugins keep their spec names (RequestHeaderModifier, etc.);
///   custom Edgion plugins use a plain PascalCase name (RateLimitLocal, CtxSet, etc.).
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
    /// Inject a Controller-resolved credential into the primary upstream request.
    CredentialInjector(CredentialInjectorConfig),
    /// Route OpenAI-compatible requests to an EdgionBackend AI capability.
    AiProxy(AiProxyConfig),
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
    /// Buffer the proxied request body for later-stage consumers.
    RequestBodyBuffer(RequestBodyBufferConfig),
    /// Validate a bounded JSON request body against an inline JSON Schema.
    JsonSchemaValidation(JsonSchemaValidationConfig),
    /// Transform a bounded request body between form URL encoding and flat JSON.
    FormJsonTransform(FormJsonTransformConfig),
    /// Response Rewrite filter (rewrite status code and headers before returning to client)
    ResponseRewrite(ResponseRewriteConfig),
    /// RateLimitLocal filter (CMS algorithm for high-performance rate limiting)
    RateLimitLocal(RateLimitLocalConfig),
    /// RateLimitRedis filter (Redis-based precise cluster-wide rate limiting)
    RateLimitRedis(RateLimitRedisConfig),
    /// CtxSet filter (set context variables from various sources with extraction, transformation, and mapping)
    CtxSet(CtxSetConfig),
    /// WebhookKeyGet filter (resolve webhook vars into request context once per request)
    WebhookKeyGet(WebhookKeyGetConfig),
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
    /// RequestId filter (request correlation identifiers)
    RequestId(RequestIdConfig),
    /// ExtProc filter (external processing over a bidirectional gRPC stream)
    ExtProc(ExtProcConfig),
    /// Unary Pass/Replace/Reject decisions for canonical semantic SSE windows.
    Guardrail(GuardrailConfig),
    /// Request-side prompt guarding over canonical LLM message spans.
    AiGuard(AiGuardConfig),
    /// L7 access policy with DirectPeerIp/RemoteIp branches and IP-first/URL-first rules.
    RequestAccessPolicy(RequestAccessPolicyConfig),
    /// GeoIpLocation filter (resolve peer geo; optional deny/allow rule groups over IP/country/continent/city/subdivision)
    GeoIpLocation(GeoIpLocationConfig),
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
/// Buffered bodies are supported only at the request stage. Proxied upstream
/// response bodies remain streaming-only.
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
        use super::body_requirement::BodyRequirement;
        use EdgionPlugin::*;
        match self {
            // HMAC consumes the REQUEST body only, and only when Digest
            // verification is on. Never at the response stage.
            HmacAuth(cfg) if cfg.validate_request_body && stage == BodyStage::Request => {
                Some(BodyRequirement::fail_close())
            }
            RequestAccessPolicy(cfg) if cfg.validates_request_body() && stage == BodyStage::Request => {
                Some(BodyRequirement::fail_close())
            }
            RequestRestriction(cfg) if cfg.validates_request_body() && stage == BodyStage::Request => {
                Some(BodyRequirement::fail_close())
            }
            ForwardAuth(cfg) if cfg.forward_body && stage == BodyStage::Request => Some(BodyRequirement::fail_close()),
            ProxyRewrite(cfg) if cfg.json_body.is_some() && stage == BodyStage::Request => Some(BodyRequirement {
                max_body_size: Some(DEFAULT_INSTANCE_MAX_BYTES.to_string()),
                ..BodyRequirement::fail_close()
            }),
            // RequestBodyBuffer is a non-security capture primitive. Unlike
            // auth/validation consumers, operators may narrow its capture
            // scope with `body.conditions`; an empty intrinsic condition set
            // still means "capture whenever the entry runs".
            RequestBodyBuffer(_) if stage == BodyStage::Request => Some(BodyRequirement {
                conditions: Some(Default::default()),
                ..BodyRequirement::fail_close()
            }),
            JsonSchemaValidation(_) if stage == BodyStage::Request => Some(BodyRequirement {
                max_body_size: Some(DEFAULT_INSTANCE_MAX_BYTES.to_string()),
                ..BodyRequirement::fail_close()
            }),
            FormJsonTransform(_) if stage == BodyStage::Request => Some(BodyRequirement {
                max_body_size: Some(DEFAULT_INSTANCE_MAX_BYTES.to_string()),
                ..BodyRequirement::fail_close()
            }),
            AiProxy(_) if stage == BodyStage::Request => Some(BodyRequirement::fail_close()),
            // AiGuard selects spans from, and masks, the buffered canonical
            // request body, so it must be gated exactly like AiProxy.
            AiGuard(_) if stage == BodyStage::Request => Some(BodyRequirement::fail_close()),
            // Deliberately keyed on BUFFERED only: STREAMED must have NO
            // BodyRequirement — the streamed handler is driven per chunk by the
            // transport pump and must never trigger a pre-read capture
            // (pre-reading would starve the pump and break the hook contract).
            ExtProc(cfg) if cfg.processing_mode.request_body_mode.is_buffered() && stage == BodyStage::Request => {
                Some(BodyRequirement::fail_close())
            }
            // DSL consumes a body only when its script references the matching
            // builtin at this stage.
            //
            // The fail policy is NOT derived from `errorPolicy`. That field
            // governs script RUNTIME errors; body availability is a different
            // question. A refusal is unconditionally fail-close by design (R5),
            // and a failed read means the script's body check never ran — for a
            // script written to enforce something, silently passing is the one
            // outcome that must not happen. Hard BodyAccess failures remain
            // fixed fail-close inside the VM; `errorPolicy` still applies to
            // ordinary script runtime errors.
            Dsl(cfg) if stage == BodyStage::Request => {
                let script = cfg.compile_and_validate().ok()?;
                if !script.references_builtin(BuiltinId::ReqBody) {
                    return None;
                }
                Some(BodyRequirement::fail_close())
            }
            // Exhaustive (no `_`, matching `type_name`/`validation_error`): a new
            // body-consuming plugin MUST add itself above, or this fails to
            // compile — it can never silently default to "no body".
            HmacAuth(_)
            | AiProxy(_)
            | RequestHeaderModifier(_)
            | ResponseHeaderModifier(_)
            | RequestRedirect(_)
            | UrlRewrite(_)
            | RequestMirror(_)
            | BasicAuth(_)
            | CredentialInjector(_)
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
            | RequestBodyBuffer(_)
            | JsonSchemaValidation(_)
            | FormJsonTransform(_)
            | RequestRestriction(_)
            | ResponseRewrite(_)
            | RateLimitLocal(_)
            | RateLimitRedis(_)
            | CtxSet(_)
            | WebhookKeyGet(_)
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
            | RequestId(_)
            | ExtProc(_)
            | Guardrail(_)
            | AiGuard(_)
            | Dsl(_)
            | RequestAccessPolicy(_)
            | Canary(_)
            | GeoIpLocation(_)
            | Wasm(_) => None,
        }
    }

    /// Whether this plugin accepts an operator `body:` configuration at the
    /// given stage (i.e. it consumes the body in its current config at that
    /// stage). Equivalent to `body_requirement(stage).is_some()`, with one
    /// exception: `Wasm` opts into request buffer mode via an operator `body:`
    /// block on the plugin entry; its intrinsic `body_requirement()` is (and
    /// must stay) `None` because the guest's exported hooks are only known after
    /// module load, not from the plugin's own config. So `Wasm` accepts a
    /// `body:` block at the request stage only (it has no response buffer mode).
    /// Every other plugin gates solely on its intrinsic declaration.
    pub fn accepts_body(&self, stage: BodyStage) -> bool {
        (matches!(self, EdgionPlugin::Wasm(_)) && stage == BodyStage::Request) || self.body_requirement(stage).is_some()
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
            EdgionPlugin::CredentialInjector(_) => "CredentialInjector",
            EdgionPlugin::AiProxy(_) => "AiProxy",
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
            EdgionPlugin::RequestBodyBuffer(_) => "RequestBodyBuffer",
            EdgionPlugin::JsonSchemaValidation(_) => "JsonSchemaValidation",
            EdgionPlugin::FormJsonTransform(_) => "FormJsonTransform",
            EdgionPlugin::RequestRestriction(_) => "RequestRestriction",
            EdgionPlugin::ResponseRewrite(_) => "ResponseRewrite",
            EdgionPlugin::RateLimitLocal(_) => "RateLimitLocal",
            EdgionPlugin::RateLimitRedis(_) => "RateLimitRedis",
            EdgionPlugin::CtxSet(_) => "CtxSet",
            EdgionPlugin::WebhookKeyGet(_) => "WebhookKeyGet",
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
            EdgionPlugin::RequestId(_) => "RequestId",
            EdgionPlugin::ExtProc(_) => "ExtProc",
            EdgionPlugin::Guardrail(_) => "Guardrail",
            EdgionPlugin::AiGuard(_) => "AiGuard",
            EdgionPlugin::RequestAccessPolicy(_) => "RequestAccessPolicy",
            EdgionPlugin::GeoIpLocation(_) => "GeoIpLocation",
            EdgionPlugin::Canary(_) => "Canary",
            EdgionPlugin::Wasm(_) => "Wasm",
        }
    }

    /// Whether this request plugin can publish an upstream jump.
    pub fn publishes_upstream_jump(&self) -> bool {
        is_upstream_jump_plugin_type(self.type_name())
    }

    /// Renderable outcomes for this entry's stage and effective configuration.
    /// This is the shared Controller/Gateway contract. It includes request-owned
    /// callbacks before response-header commit, never body/trailer response dye.
    /// Runtime input matching is not predicted here; stage and policy exclusions are.
    pub fn dye_capabilities(&self, stage: DyeStage) -> &'static [DyeOutcome] {
        use super::plugin_configs::{DslErrorPolicy, OnRedisFailure, VerificationMode};
        let stages = super::validate::applicable_stages_by_name(self.type_name());
        let applicable = match stage {
            DyeStage::Request => stages.request,
            DyeStage::UpstreamResponseFilter => stages.upstream_response_filter,
            DyeStage::UpstreamResponseBodyFilter => false,
            DyeStage::UpstreamResponse => stages.upstream_response,
        };
        if !applicable {
            return &[];
        }
        match self {
            EdgionPlugin::Canary(c) => {
                // The internal kill switch pauses selection, not the entry's
                // anti-spoof ownership. Keep its valid request dye plan installed.
                selection_policy_dye(true, &[c.on_invalid])
            }
            EdgionPlugin::DynamicInternalUpstream(c) => {
                if c.rules.is_some() {
                    selection_policy_dye(true, &[c.on_missing, c.on_invalid, c.on_no_match])
                } else {
                    selection_policy_dye(true, &[c.on_missing, c.on_invalid])
                }
            }
            EdgionPlugin::DynamicExternalUpstream(c) => {
                selection_policy_dye(!c.domain_map.is_empty(), &[c.on_missing, c.on_no_match])
            }
            EdgionPlugin::DirectEndpoint(c) => selection_policy_dye(true, &[c.on_missing, c.on_invalid]),
            EdgionPlugin::RegionRoute(c) => selection_dye(
                !c.route_rules.is_empty() && (!c.regions.is_empty() || c.override_ref.is_some()),
                c.override_ref.is_some() && (!c.route_rules.is_empty() || !c.regions.is_empty()),
                false,
            ),
            // Dependency-failure plugins.
            EdgionPlugin::RateLimitRedis(c) => match c.on_redis_failure {
                OnRedisFailure::Allow => DYE_FAIL_OPEN_ONLY,
                OnRedisFailure::Deny => DYE_FAIL_CLOSE_ONLY,
            },
            EdgionPlugin::ExtProc(c) => {
                if c.failure_mode_allow {
                    DYE_FAIL_OPEN_ONLY
                } else {
                    DYE_FAIL_CLOSE_ONLY
                }
            }
            EdgionPlugin::Wasm(c) => {
                if c.fail_open {
                    DYE_FAIL_OPEN_ONLY
                } else {
                    DYE_FAIL_CLOSE_ONLY
                }
            }
            EdgionPlugin::Dsl(c) => match c.error_policy.as_ref().unwrap_or(&DslErrorPolicy::Ignore) {
                // BodyAccess (including string regex scanner errors) stays
                // fixed fail-close even under Ignore, in both supported stages.
                // This is language capability, not arbitrary-script reachability.
                DslErrorPolicy::Ignore => DYE_FAIL_BOTH,
                DslErrorPolicy::Deny | DslErrorPolicy::DenyWith { .. } => DYE_FAIL_CLOSE_ONLY,
            },
            EdgionPlugin::OpenidConnect(c) => {
                // Nonce validation on the callback also uses JWKS, independently
                // of bearer verification mode (and before bearer_only dispatch).
                // Session refresh wait timeouts also verify the expired JWT via
                // JWKS before checking whether it is within the stale-token leeway.
                if c.use_nonce
                    || c.verification_mode == VerificationMode::JwksOnly
                    || (c.verification_mode == VerificationMode::Auto && c.use_jwks)
                    || (!c.bearer_only && c.renew_access_token_on_expiry)
                {
                    DYE_FAIL_BOTH
                } else {
                    &[]
                }
            }
            // forward_auth/ldap_auth: fixed fail-close by policy (auth class never fails
            // open — the degrade switch was removed, see failure-policy-freeze design).
            EdgionPlugin::ForwardAuth(_) | EdgionPlugin::LdapAuth(_) => DYE_FAIL_CLOSE_ONLY,
            // Not dye-wired: no `record_outcome` call site exists in these
            // plugins, so configuring any `dye:` outcome on them is an
            // admission error. Exhaustive (no `_`) so a new plugin must make
            // this choice deliberately instead of silently inheriting "no dye".
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::CredentialInjector(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::IpRestriction(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::JweDecrypt(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::RateLimitLocal(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::WebhookKeyGet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::BandwidthLimit(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::Guardrail(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::GeoIpLocation(_) => &[],
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
            // Not dye-wired (empty `dye_capabilities`), so no prefix is ever
            // emitted. Exhaustive (no `_`) so wiring dye onto a new plugin
            // cannot silently ship an unprefixed, collision-prone value.
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::CredentialInjector(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::IpRestriction(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::JweDecrypt(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::RateLimitLocal(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::WebhookKeyGet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::BandwidthLimit(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::Guardrail(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::GeoIpLocation(_) => "",
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
            // No header-typed routing key source to protect. Canary is here
            // deliberately: profile selection is driven by
            // `activeProfile`/`activeProfileRef`, not a request key, and it was
            // never a `key_collision_error` call site. Exhaustive (no `_`) so a
            // new routing-key plugin cannot silently skip the dye/key collision
            // check and have its own routing input stripped by a dye header.
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::CredentialInjector(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::IpRestriction(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::JweDecrypt(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::LdapAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::RateLimitLocal(_)
            | EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::WebhookKeyGet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::ForwardAuth(_)
            | EdgionPlugin::OpenidConnect(_)
            | EdgionPlugin::BandwidthLimit(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::Dsl(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::Guardrail(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::Canary(_)
            | EdgionPlugin::GeoIpLocation(_)
            | EdgionPlugin::Wasm(_) => Vec::new(),
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
            EdgionPlugin::WebhookKeyGet(c) => c.validation_error(),
            EdgionPlugin::AiProxy(c) => c.validation_error(),
            EdgionPlugin::DirectEndpoint(c) => c.validation_error(),
            EdgionPlugin::RegionRoute(c) => c.validation_error(),
            EdgionPlugin::DynamicInternalUpstream(c) => c.validation_error(),
            EdgionPlugin::DynamicExternalUpstream(c) => c.validation_error(),
            EdgionPlugin::KeyAuth(c) => c.validation_error(),
            EdgionPlugin::ResponseRewrite(c) => c.validation_error(),
            EdgionPlugin::ProxyRewrite(c) => c.validation_error(),
            EdgionPlugin::RequestBodyBuffer(c) => c.validation_error(),
            EdgionPlugin::JsonSchemaValidation(c) => c.validation_error(),
            EdgionPlugin::FormJsonTransform(c) => c.validation_error(),
            EdgionPlugin::RequestRestriction(c) => c.validation_error(),
            EdgionPlugin::RateLimitLocal(c) => c.validation_error(),
            EdgionPlugin::RateLimitRedis(c) => c.validation_error(),
            EdgionPlugin::Mock(c) => c.validation_error(),
            EdgionPlugin::Csrf(c) => c.validation_error(),
            EdgionPlugin::BasicAuth(c) => c.validation_error(),
            EdgionPlugin::CredentialInjector(c) => c.validation_error(),
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
            EdgionPlugin::RequestId(c) => c.validation_error(),
            EdgionPlugin::ExtProc(c) => c.validation_error(),
            EdgionPlugin::Guardrail(c) => c.validation_error(),
            EdgionPlugin::AiGuard(c) => c.validation_error(),
            EdgionPlugin::RequestAccessPolicy(c) => c.validation_error(),
            EdgionPlugin::GeoIpLocation(c) => c.validation_error(),
            EdgionPlugin::Canary(c) => c.validation_error(),
            EdgionPlugin::Wasm(c) => c.validation_error(),
            EdgionPlugin::RequestMirror(c) => c.validation_error(),
            EdgionPlugin::RequestHeaderModifier(c) => c.validate().err().map(str::to_owned),
            EdgionPlugin::ResponseHeaderModifier(c) => c.validate_response().err().map(str::to_owned),
            EdgionPlugin::RequestRedirect(config) => config.prepare().err().map(|error| error.to_string()),
            EdgionPlugin::UrlRewrite(config) => config.prepare().err().map(|error| error.to_string()),
        }
    }
}

/// Shared classification for backendRef-scope validation and Gateway dispatch.
pub fn is_upstream_jump_plugin_type(name: &str) -> bool {
    matches!(
        name,
        "DirectEndpoint" | "DynamicInternalUpstream" | "DynamicExternalUpstream" | "RegionRoute" | "Canary"
    )
}

#[cfg(test)]
mod ai_proxy_variant_tests {
    use super::*;
    use crate::resources::edgion_plugins::{
        applicable_stages_by_name, AiBackendSelection, AiBackendSelectionConfig, BodyReadFailureMode, OnUnknownModel,
        PluginValidation,
    };

    fn config(name: &str) -> AiProxyConfig {
        AiProxyConfig {
            backend_selection: AiBackendSelectionConfig {
                selection_type: if name.is_empty() {
                    AiBackendSelection::LowestLatency
                } else {
                    AiBackendSelection::Weighted
                },
            },
            inject_usage_tracking: true,
            on_unknown_model: if name.is_empty() {
                OnUnknownModel::Passthrough
            } else {
                OnUnknownModel::Reject
            },
            model_routes: Vec::new(),
            token_quota: None,
        }
    }

    #[test]
    fn ai_proxy_variant_is_stateless_request_only_and_fail_close_body_gated() {
        let invalid = EdgionPlugin::AiProxy(config(""));
        assert!(invalid.validation_error().is_some());
        assert!(
            invalid.validation_error().is_some(),
            "validation must not depend on a prior call"
        );

        let plugin = EdgionPlugin::AiProxy(config("openai-prod"));
        assert_eq!(plugin.type_name(), "AiProxy");
        assert!(plugin.validation_error().is_none());
        assert!(config("openai-prod").validation_error().is_none());

        let stages = applicable_stages_by_name(plugin.type_name());
        assert!(stages.request);
        assert!(!stages.upstream_response_filter);
        assert!(!stages.upstream_response_body_filter);
        assert!(!stages.upstream_response);

        let requirement = plugin
            .body_requirement(BodyStage::Request)
            .expect("AiProxy intrinsically consumes the request body");
        assert_eq!(requirement.on_read_failure, BodyReadFailureMode::FailClose);
        assert!(plugin.body_requirement(BodyStage::Response).is_none());
    }
}

#[cfg(test)]
mod request_access_policy_variant_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig;

    #[test]
    fn type_name_and_tag_roundtrip() {
        let json = serde_json::json!({
            "type": "RequestAccessPolicy",
            "config": {
                "defaultProfile": "normal",
                "profiles": {
                    "normal": {
                        "requiredRuleGroups": [{
                            "name": "peer",
                            "anyOfRules": [{
                                "name": "internal",
                                "type": "directPeerIp",
                                "config": { "ips": ["10.0.0.0/8"] }
                            }]
                        }]
                    }
                }
            }
        });
        let p: EdgionPlugin = serde_json::from_value(json).unwrap();
        assert_eq!(p.type_name(), "RequestAccessPolicy");
        assert!(matches!(p, EdgionPlugin::RequestAccessPolicy(_)));
        // validation_error delegates to the config.
        assert!(p.validation_error().is_none());
    }

    #[test]
    fn validation_error_surfaces_bad_default_profile() {
        let cfg = RequestAccessPolicyConfig {
            default_profile: "ghost".into(),
            profiles: std::collections::HashMap::from([(
                "normal".to_string(),
                crate::resources::edgion_plugins::plugin_configs::RequestAccessProfile {
                    required_rule_groups: vec![crate::resources::edgion_plugins::plugin_configs::RequiredRuleGroup {
                        name: "peer".to_string(),
                        any_of_rules: vec![crate::resources::edgion_plugins::plugin_configs::RequestAccessRule {
                            name: "internal".to_string(),
                            rule: crate::resources::edgion_plugins::plugin_configs::RequestAccessRuleType::DirectPeerIp(
                                crate::resources::edgion_plugins::plugin_configs::IpAccessRuleConfig {
                                    ip: crate::resources::edgion_plugins::plugin_configs::IpAccessMatch {
                                        ips: vec!["10.0.0.0/8".to_string()],
                                        ..Default::default()
                                    },
                                    conditions: None,
                                },
                            ),
                        }],
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            resolved_profiles: None,
            resolution_warnings: vec![],
        };
        let p = EdgionPlugin::RequestAccessPolicy(cfg);
        assert!(p.validation_error().unwrap().contains("defaultProfile"));
    }
}

#[cfg(test)]
mod canary_variant_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
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
            on_invalid: UpstreamFallback::Reject,
        };
        let p = EdgionPlugin::Canary(cfg);
        assert!(p.validation_error().unwrap().contains("activeProfile"));
    }
}

#[cfg(test)]
mod dye_capability_tests {
    use super::*;
    use crate::resources::edgion_plugins::dye::DyeOutcome;

    const STAGES: [DyeStage; 4] = [
        DyeStage::Request,
        DyeStage::UpstreamResponseFilter,
        DyeStage::UpstreamResponseBodyFilter,
        DyeStage::UpstreamResponse,
    ];

    #[test]
    fn dependency_dye_capabilities_follow_effective_policy_and_stage() {
        use super::super::plugin_configs::OnRedisFailure;
        for allow in [false, true] {
            let expected = [if allow {
                DyeOutcome::FailOpen
            } else {
                DyeOutcome::FailClose
            }];
            let wasm = EdgionPlugin::Wasm(WasmConfig {
                fail_open: allow,
                ..Default::default()
            });
            let ext = EdgionPlugin::ExtProc(ExtProcConfig {
                failure_mode_allow: allow,
                ..Default::default()
            });
            let redis = EdgionPlugin::RateLimitRedis(RateLimitRedisConfig {
                on_redis_failure: if allow {
                    OnRedisFailure::Allow
                } else {
                    OnRedisFailure::Deny
                },
                ..Default::default()
            });
            for stage in STAGES {
                assert_eq!(
                    wasm.dye_capabilities(stage),
                    if matches!(stage, DyeStage::Request | DyeStage::UpstreamResponseFilter) {
                        &expected[..]
                    } else {
                        &[]
                    }
                );
                for plugin in [&ext, &redis] {
                    assert_eq!(
                        plugin.dye_capabilities(stage),
                        if stage == DyeStage::Request { &expected[..] } else { &[] }
                    );
                }
            }
        }
    }

    #[test]
    fn selection_dye_excludes_inactive_configuration_branches() {
        use super::super::UpstreamFallback::{Fallback, Reject};
        for missing in [Fallback, Reject] {
            for invalid in [Fallback, Reject] {
                for no_match in [Fallback, Reject] {
                    let mut internal = DynamicInternalUpstreamConfig {
                        on_missing: missing,
                        on_invalid: invalid,
                        on_no_match: no_match,
                        ..Default::default()
                    };
                    let plugin = EdgionPlugin::DynamicInternalUpstream(internal.clone());
                    assert_eq!(
                        plugin
                            .dye_capabilities(DyeStage::Request)
                            .contains(&DyeOutcome::Fallback),
                        missing == Fallback || invalid == Fallback
                    );
                    assert_eq!(
                        plugin.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject),
                        missing == Reject || invalid == Reject
                    );
                    internal.rules = Some(
                        serde_json::from_value(serde_json::json!([
                            {"value":"key","backendRef":{"name":"app"}}
                        ]))
                        .unwrap(),
                    );
                    // A rules table enables on-no-match; direct mode does not.
                    assert!(EdgionPlugin::DynamicInternalUpstream(internal)
                        .dye_capabilities(DyeStage::Request)
                        .contains(&if no_match == Fallback {
                            DyeOutcome::Fallback
                        } else {
                            DyeOutcome::Reject
                        }));
                }
                let direct = EdgionPlugin::DirectEndpoint(DirectEndpointConfig {
                    on_missing: missing,
                    on_invalid: invalid,
                    ..Default::default()
                });
                assert_eq!(
                    direct
                        .dye_capabilities(DyeStage::Request)
                        .contains(&DyeOutcome::Fallback),
                    missing == Fallback || invalid == Fallback
                );
                assert_eq!(
                    direct.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject),
                    missing == Reject || invalid == Reject
                );
                for stage in &STAGES[1..] {
                    assert!(direct.dye_capabilities(*stage).is_empty());
                }
            }
        }
    }

    #[test]
    fn oidc_nonce_callback_keeps_jwks_dye_independent_of_bearer_mode() {
        use super::super::VerificationMode;
        for mode in [
            VerificationMode::Auto,
            VerificationMode::JwksOnly,
            VerificationMode::IntrospectionOnly,
        ] {
            for use_jwks in [false, true] {
                for use_nonce in [false, true] {
                    let expected =
                        use_nonce || mode == VerificationMode::JwksOnly || (mode == VerificationMode::Auto && use_jwks);
                    for bearer_only in [false, true] {
                        let plugin = EdgionPlugin::OpenidConnect(Box::new(OpenidConnectConfig {
                            verification_mode: mode,
                            use_jwks,
                            use_nonce,
                            bearer_only,
                            renew_access_token_on_expiry: false,
                            ..Default::default()
                        }));
                        assert_eq!(
                            plugin.dye_capabilities(DyeStage::Request),
                            if expected { DYE_FAIL_BOTH } else { &[] }
                        );
                        for stage in &STAGES[1..] {
                            assert!(plugin.dye_capabilities(*stage).is_empty());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn oidc_session_refresh_dye_requires_session_mode_and_renewal() {
        use super::super::{RequestFilterEntry, VerificationMode};
        for mode in [VerificationMode::IntrospectionOnly, VerificationMode::Auto] {
            for (bearer_only, renew, supported) in [
                (false, true, true),
                (false, false, false),
                (true, true, false),
                (true, false, false),
            ] {
                let plugin = EdgionPlugin::OpenidConnect(Box::new(OpenidConnectConfig {
                    verification_mode: mode,
                    use_jwks: false,
                    use_nonce: false,
                    bearer_only,
                    renew_access_token_on_expiry: renew,
                    ..Default::default()
                }));
                let mut entry = RequestFilterEntry::new(plugin);
                entry.dye = Some(
                    serde_json::from_value(serde_json::json!({
                        "response": [{"name": "X-OIDC-Degraded", "on": ["fail-open", "fail-close"]}]
                    }))
                    .unwrap(),
                );
                assert_eq!(
                    entry.validate_dye().is_none(),
                    supported,
                    "{mode:?}/{bearer_only}/{renew}"
                );
                assert_eq!(
                    entry.plugin.dye_capabilities(DyeStage::Request),
                    if supported { DYE_FAIL_BOTH } else { &[] }
                );
                for stage in &STAGES[1..] {
                    assert!(entry.plugin.dye_capabilities(*stage).is_empty());
                }
            }
        }
    }

    #[test]
    fn dsl_ignore_retains_hard_error_capability_in_both_header_stages() {
        use super::super::plugin_configs::DslErrorPolicy;
        for policy in [
            None,
            Some(DslErrorPolicy::Ignore),
            Some(DslErrorPolicy::Deny),
            Some(DslErrorPolicy::DenyWith {
                status: 503,
                body: None,
            }),
        ] {
            let expected = if matches!(policy, None | Some(DslErrorPolicy::Ignore)) {
                DYE_FAIL_BOTH
            } else {
                DYE_FAIL_CLOSE_ONLY
            };
            let plugin = EdgionPlugin::Dsl(DslConfig {
                error_policy: policy,
                ..Default::default()
            });
            for stage in STAGES {
                assert_eq!(
                    plugin.dye_capabilities(stage),
                    if matches!(stage, DyeStage::Request | DyeStage::UpstreamResponseFilter) {
                        expected
                    } else {
                        &[]
                    }
                );
            }
        }
    }

    #[test]
    fn selection_plugins_declare_success() {
        use crate::resources::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
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
            on_invalid: UpstreamFallback::Reject,
        };
        let p = EdgionPlugin::Canary(cfg);
        assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Success));
        assert!(!p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Fallback));
        assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject));
        assert_eq!(p.dye_type_prefix(), "canary");
    }

    #[test]
    fn unwired_plugin_declares_nothing() {
        use crate::resources::plugin_configs::CorsConfig;
        let p = EdgionPlugin::Cors(CorsConfig::default());
        assert!(p.dye_capabilities(DyeStage::Request).is_empty());
    }

    #[test]
    fn type_prefix_is_snake_case_of_type_name() {
        use crate::resources::plugin_configs::RateLimitRedisConfig;
        let p = EdgionPlugin::RateLimitRedis(RateLimitRedisConfig::default());
        assert_eq!(p.dye_type_prefix(), "rate_limit_redis");
        assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::FailOpen));
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
        let direct_ip: DirectEndpointConfig = serde_yaml::from_str("from:\n  type: remoteIp\n").unwrap();
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
            EdgionPlugin::RegionRoute(RegionRouteConfig::default()).dye_capabilities(DyeStage::Request),
            &[]
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
    fn dsl_resp_body_is_rejected_instead_of_gated() {
        let p = dsl("let b = resp.body", None);
        let EdgionPlugin::Dsl(config) = &p else { unreachable!() };
        assert!(config.compile_and_validate().unwrap_err().contains("streaming-only"));
        assert!(p.body_requirement(BodyStage::Response).is_none());
        assert!(p.body_requirement(BodyStage::Request).is_none());
    }

    #[test]
    fn dsl_header_only_never_gated() {
        let p = dsl("let h = req.header(\"X\")", None);
        assert!(p.body_requirement(BodyStage::Request).is_none());
        assert!(p.body_requirement(BodyStage::Response).is_none());
    }

    #[test]
    fn dsl_failure_mode_is_fail_close_regardless_of_error_policy() {
        // `errorPolicy` governs script RUNTIME errors, not body availability. A
        // refusal or a failed read means the script's body check never ran, so
        // the intrinsic body requirement is unconditionally fail-close — this
        // used to vary with `errorPolicy` (None/Ignore => fail-open), which is
        // exactly the silent-pass hole this test now guards against.
        let none_policy = dsl("let b = req.body", None);
        assert_eq!(
            none_policy
                .body_requirement(BodyStage::Request)
                .unwrap()
                .on_read_failure,
            BodyReadFailureMode::FailClose
        );
        let deny = dsl("let b = req.body", Some(DslErrorPolicy::Deny));
        assert_eq!(
            deny.body_requirement(BodyStage::Request).unwrap().on_read_failure,
            BodyReadFailureMode::FailClose
        );
    }

    #[test]
    fn dsl_body_requirement_is_fail_close_regardless_of_error_policy() {
        // `errorPolicy` governs SCRIPT runtime errors. Body availability is not a
        // script error: a refusal or a failed read means the script's body check
        // never ran, and an enforcement script must not silently pass.
        for policy in [r#""ignore""#, r#""deny""#] {
            let json = format!(
                r#"{{ "type": "Dsl", "config": {{ "name": "t", "source": "let b = req.body", "errorPolicy": {policy} }} }}"#
            );
            let p: EdgionPlugin = serde_json::from_str(&json).expect("parses");
            let req = p
                .body_requirement(BodyStage::Request)
                .expect("dsl script referencing req.body declares a body requirement");
            assert_eq!(
                req.on_read_failure,
                BodyReadFailureMode::FailClose,
                "errorPolicy {policy} must not weaken the body requirement"
            );
        }
    }
}

#[cfg(test)]
mod wasm_body_gate_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::WasmConfig;

    /// Wasm has no intrinsic `body_requirement()` (the guest's hooks are only
    /// known after module load), but it MUST still accept an operator `body:`
    /// block at the request stage so the gateway's body gate can wrap it —
    /// otherwise a `body:` entry on a Wasm request plugin is rejected at
    /// admission and the request-buffer-mode feature is unreachable.
    /// `body_requirement()` itself stays `None` for Wasm (it is config-only, no
    /// operator input); the opt-in is expressed solely through `accepts_body()`.
    /// Wasm has no response buffer mode, so it must NOT accept a body: block at
    /// the response stage.
    #[test]
    fn wasm_accepts_operator_body_requirement() {
        let p = EdgionPlugin::Wasm(WasmConfig::default());
        assert!(
            p.body_requirement(BodyStage::Request).is_none(),
            "Wasm has no intrinsic body requirement (config-only signal)"
        );
        assert!(
            p.accepts_body(BodyStage::Request),
            "Wasm must accept an operator body: block at the request stage despite having no intrinsic requirement"
        );
        assert!(
            !p.accepts_body(BodyStage::Response),
            "Wasm has no response buffer mode, so it must not accept a body: block at the response stage"
        );
    }
}

#[cfg(test)]
mod request_body_plugin_requirement_tests {
    use super::*;
    use crate::conditions::PluginConditions;
    use crate::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement};

    #[test]
    fn request_body_features_declare_request_only_requirements() {
        for json in [
            r#"{"type":"ForwardAuth","config":{"target":{"name":"auth","port":80},"forwardBody":true,"decision":{}}}"#,
            r#"{"type":"ProxyRewrite","config":{"jsonBody":{"operations":[{"op":"replace","path":"/name","value":"gateway"}]}}}"#,
            r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"BUFFERED"}}}"#,
            r#"{"type":"RequestBodyBuffer","config":{}}"#,
            r#"{"type":"FormJsonTransform","config":{"direction":"formToJson"}}"#,
        ] {
            let plugin: EdgionPlugin = serde_json::from_str(json).expect("body plugin config parses");
            assert!(plugin.body_requirement(BodyStage::Request).is_some(), "{json}");
            assert!(plugin.body_requirement(BodyStage::Response).is_none(), "{json}");
        }
    }

    #[test]
    fn ext_proc_override_reductions_do_not_change_static_body_acquisition() {
        let buffered: EdgionPlugin = serde_json::from_str(
            r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"BUFFERED"},"allowModeOverride":true,"allowedOverrideModes":[{"requestBodyMode":"NONE"}]}}"#,
        )
        .unwrap();
        assert!(buffered.body_requirement(BodyStage::Request).is_some());

        let streamed: EdgionPlugin = serde_json::from_str(
            r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"STREAMED"},"allowModeOverride":true,"allowedOverrideModes":[{"requestBodyMode":"NONE"}]}}"#,
        )
        .unwrap();
        assert!(streamed.body_requirement(BodyStage::Request).is_none());
    }

    #[test]
    fn proxy_rewrite_json_body_defaults_to_64_kib_fail_close() {
        let plugin: EdgionPlugin = serde_json::from_str(
            r#"{"type":"ProxyRewrite","config":{"jsonBody":{"operations":[{"op":"add","path":"/enabled","value":true}]}}}"#,
        )
        .unwrap();
        let requirement = plugin.body_requirement(BodyStage::Request).unwrap();
        assert_eq!(requirement.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
        assert!(requirement.is_fail_close());
        assert!(requirement.conditions.is_none());
    }

    #[test]
    fn request_body_buffer_preserves_operator_capture_conditions() {
        let plugin: EdgionPlugin = serde_json::from_str(r#"{"type":"RequestBodyBuffer","config":{}}"#).unwrap();
        let operator = BodyRequirement {
            conditions: Some(PluginConditions {
                skip: None,
                run: Some(crate::ConditionSet::all_of(vec![])),
            }),
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        };

        let resolved = BodyRequirement::resolve(Some(operator), plugin.body_requirement(BodyStage::Request)).unwrap();
        assert!(resolved.conditions.is_some());
        assert!(resolved.is_fail_close());
    }
}
