//! Gateway API standard edgion_plugins and custom Edgion edgion_plugins

use super::plugin_configs::RestToGraphqlConfig;
use edgion_dsl::bytecode::BuiltinId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::dye::{DyeOutcome, DyeStage};
use super::plugin_configs::{
    AiGuardConfig, AiProxyConfig, AllEndpointStatusConfig, BandwidthLimitConfig, BasicAuthConfig, CanaryConfig,
    CorsConfig, CredentialInjectorConfig, CsrfConfig, CtxSetConfig, DebugAccessLogToHeaderConfig, DirectEndpointConfig,
    DslConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig, ExtProcConfig, FaultInjectionConfig,
    FormJsonTransformConfig, ForwardAuthConfig, GeoIpLocationConfig, GraphqlGuardConfig, GraphqlOperationRouterConfig,
    GraphqlRateLimitConfig, GraphqlSafelistConfig, GuardrailConfig, HeaderCertAuthConfig, HmacAuthConfig,
    IpRestrictionConfig, JsonSchemaValidationConfig, JweDecryptConfig, JwtAuthConfig, KeyAuthConfig, LdapAuthConfig,
    MockConfig, OpenidConnectConfig, ProxyRewriteConfig, RateLimitLocalConfig, RateLimitRedisConfig, RealIpConfig,
    RegionRouteConfig, RequestAccessPolicyConfig, RequestBodyBufferConfig, RequestIdConfig, RequestMirrorPluginConfig,
    RequestRestrictionConfig, ResponseCacheConfig, ResponseRewriteConfig, WasmConfig, WebhookKeyGetConfig,
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
    /// Cache explicitly public finite responses after all request admission.
    ResponseCache(ResponseCacheConfig),
    /// Validate a bounded JSON request body against an inline JSON Schema.
    JsonSchemaValidation(JsonSchemaValidationConfig),
    /// Schema-aware GraphQL query-cost budgets using the canonical Redis limiter.
    RestToGraphql(RestToGraphqlConfig),
    GraphqlGuard(GraphqlGuardConfig),
    GraphqlSafelist(GraphqlSafelistConfig),
    GraphqlRateLimit(GraphqlRateLimitConfig),
    GraphqlOperationRouter(GraphqlOperationRouterConfig),
    /// Convert REST JSON to native upstream gRPC using compiled descriptors.
    GrpcJsonTranscoder(super::plugin_configs::GrpcJsonTranscoderConfig),
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
    /// Non-executable marker for a plugin type this binary does not support.
    /// Validation rejects the containing resource, including disabled entries.
    #[serde(other, deserialize_with = "crate::resources::common::discard_unsupported_config")]
    #[schemars(skip)]
    Unsupported,
    // TODO: Add more custom Edgion plugins here (no `Edgion` prefix on the variant name)
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
    /// Whether this typed plugin conflicts with a separate protocol conversion owner.
    /// Gateway publication additionally requires successful executable construction.
    pub fn is_ai_conversion_owner(&self) -> bool {
        match self {
            Self::AiProxy(_) | Self::AiGuard(_) | Self::Guardrail(_) => true,
            Self::RequestHeaderModifier(_)
            | Self::ResponseHeaderModifier(_)
            | Self::RequestRedirect(_)
            | Self::UrlRewrite(_)
            | Self::RequestMirror(_)
            | Self::BasicAuth(_)
            | Self::CredentialInjector(_)
            | Self::Cors(_)
            | Self::Csrf(_)
            | Self::IpRestriction(_)
            | Self::JwtAuth(_)
            | Self::JweDecrypt(_)
            | Self::HmacAuth(_)
            | Self::HeaderCertAuth(_)
            | Self::KeyAuth(_)
            | Self::LdapAuth(_)
            | Self::Mock(_)
            | Self::FaultInjection(_)
            | Self::DebugAccessLogToHeader(_)
            | Self::ProxyRewrite(_)
            | Self::RequestRestriction(_)
            | Self::ResponseCache(_)
            | Self::RequestBodyBuffer(_)
            | Self::JsonSchemaValidation(_)
            | Self::RestToGraphql(_)
            | Self::GraphqlGuard(_)
            | Self::GraphqlSafelist(_)
            | Self::GraphqlRateLimit(_)
            | Self::GraphqlOperationRouter(_)
            | Self::GrpcJsonTranscoder(_)
            | Self::FormJsonTransform(_)
            | Self::ResponseRewrite(_)
            | Self::RateLimitLocal(_)
            | Self::RateLimitRedis(_)
            | Self::CtxSet(_)
            | Self::WebhookKeyGet(_)
            | Self::RealIp(_)
            | Self::ForwardAuth(_)
            | Self::OpenidConnect(_)
            | Self::BandwidthLimit(_)
            | Self::DirectEndpoint(_)
            | Self::AllEndpointStatus(_)
            | Self::DynamicInternalUpstream(_)
            | Self::DynamicExternalUpstream(_)
            | Self::Dsl(_)
            | Self::RegionRoute(_)
            | Self::RequestId(_)
            | Self::ExtProc(_)
            | Self::RequestAccessPolicy(_)
            | Self::GeoIpLocation(_)
            | Self::Canary(_)
            | Self::Wasm(_)
            | Self::Unsupported => false,
        }
    }

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
            EdgionPlugin::Unsupported => None,
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
            GrpcJsonTranscoder(_) if stage == BodyStage::Request => Some(BodyRequirement {
                max_body_size: Some("1m".into()),
                ..BodyRequirement::fail_close()
            }),
            JsonSchemaValidation(_) if stage == BodyStage::Request => Some(BodyRequirement {
                max_body_size: Some(DEFAULT_INSTANCE_MAX_BYTES.to_string()),
                ..BodyRequirement::fail_close()
            }),
            RestToGraphql(_)
            | GraphqlGuard(_)
            | GraphqlSafelist(_)
            | GraphqlRateLimit(_)
            | GraphqlOperationRouter(_)
                if stage == BodyStage::Request =>
            {
                Some(BodyRequirement {
                    max_body_size: Some(crate::graphql_request::MAX_ENVELOPE_BYTES.to_string()),
                    ..BodyRequirement::fail_close()
                })
            }
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
                if !script.references_builtin(BuiltinId::ReqBody) && !script.references_builtin(BuiltinId::JsonSetBody)
                {
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
            | ResponseCache(_)
            | RequestBodyBuffer(_)
            | GrpcJsonTranscoder(_)
            | JsonSchemaValidation(_)
            | RestToGraphql(_)
            | GraphqlGuard(_)
            | GraphqlSafelist(_)
            | GraphqlRateLimit(_)
            | GraphqlOperationRouter(_)
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

    /// Shared Controller/Gateway validation for bounded structured consumers.
    pub fn structured_body_validation_errors(&self, body: &super::BodyRequirement) -> Vec<String> {
        if matches!(
            self,
            Self::RestToGraphql(_)
                | Self::GraphqlGuard(_)
                | Self::GraphqlSafelist(_)
                | Self::GraphqlRateLimit(_)
                | Self::GraphqlOperationRouter(_)
        ) {
            super::plugin_configs::graphql_body_validation_errors(body)
        } else {
            super::plugin_configs::bounded_structured_body_validation_errors(body)
        }
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionPlugin::Unsupported => "Unsupported",
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
            EdgionPlugin::ResponseCache(_) => "ResponseCache",
            EdgionPlugin::RequestBodyBuffer(_) => "RequestBodyBuffer",
            EdgionPlugin::JsonSchemaValidation(_) => "JsonSchemaValidation",
            EdgionPlugin::RestToGraphql(_) => "RestToGraphql",
            EdgionPlugin::GraphqlGuard(_) => "GraphqlGuard",
            EdgionPlugin::GraphqlSafelist(_) => "GraphqlSafelist",
            EdgionPlugin::GraphqlRateLimit(_) => "GraphqlRateLimit",
            EdgionPlugin::GraphqlOperationRouter(_) => "GraphqlOperationRouter",
            EdgionPlugin::GrpcJsonTranscoder(_) => "GrpcJsonTranscoder",
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

    /// Attachment classification shared by Controller and Gateway.
    pub fn requires_http_route_rule_scope(&self) -> bool {
        matches!(self, Self::GrpcJsonTranscoder(_) | Self::GraphqlOperationRouter(_))
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
            EdgionPlugin::Unsupported => &[],
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
            EdgionPlugin::RestToGraphql(_)
            | EdgionPlugin::GraphqlGuard(_)
            | EdgionPlugin::GraphqlSafelist(_)
            | EdgionPlugin::GraphqlOperationRouter(_) => &[],
            EdgionPlugin::GraphqlRateLimit(c) => match c.on_redis_failure {
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
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
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
            EdgionPlugin::Unsupported => "unsupported",
            EdgionPlugin::Canary(_) => "canary",
            EdgionPlugin::DynamicInternalUpstream(_) => "dynamic_internal_upstream",
            EdgionPlugin::DynamicExternalUpstream(_) => "dynamic_external_upstream",
            EdgionPlugin::DirectEndpoint(_) => "direct_endpoint",
            EdgionPlugin::RegionRoute(_) => "region_route",
            EdgionPlugin::RateLimitRedis(_) => "rate_limit_redis",
            EdgionPlugin::RestToGraphql(_) => "rest_to_graphql",
            EdgionPlugin::GraphqlGuard(_) => "graphql_guard",
            EdgionPlugin::GraphqlSafelist(_) => "graphql_safelist",
            EdgionPlugin::GraphqlRateLimit(_) => "graphql_rate_limit",
            EdgionPlugin::GraphqlOperationRouter(_) => "graphql_operation_router",
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
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
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
            EdgionPlugin::Unsupported => Vec::new(),
            EdgionPlugin::ProxyRewrite(c) => c.host_from_header.iter().map(|source| source.name.clone()).collect(),
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
            EdgionPlugin::GraphqlRateLimit(c) => {
                let mut names = headers_of(c.key.iter());
                for quota in &c.root_quotas {
                    if let Some(keys) = &quota.key {
                        names.extend(headers_of(keys.iter()));
                    }
                }
                names
            }
            EdgionPlugin::RestToGraphql(c) => headers_of(c.variables.iter().filter_map(|v| v.from.as_ref())),
            EdgionPlugin::GraphqlGuard(_)
            | EdgionPlugin::GraphqlSafelist(_)
            | EdgionPlugin::GraphqlOperationRouter(_) => Vec::new(),
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
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
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
            EdgionPlugin::Unsupported => Some("unsupported plugin type".to_string()),
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
            EdgionPlugin::ResponseCache(c) => c.validation_error(),
            EdgionPlugin::RequestBodyBuffer(c) => c.validation_error(),
            EdgionPlugin::JsonSchemaValidation(c) => c.validation_error(),
            EdgionPlugin::RestToGraphql(c) => c.validation_error(),
            EdgionPlugin::GraphqlGuard(c) => c.validation_error(),
            EdgionPlugin::GraphqlSafelist(c) => c.validation_error(),
            EdgionPlugin::GraphqlRateLimit(c) => c.validation_error(),
            EdgionPlugin::GraphqlOperationRouter(c) => c.validation_error(),
            EdgionPlugin::GrpcJsonTranscoder(c) => c.validation_error(),
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
        "DirectEndpoint"
            | "DynamicInternalUpstream"
            | "DynamicExternalUpstream"
            | "RegionRoute"
            | "Canary"
            | "GraphqlOperationRouter"
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/ai_proxy_variant_tests.rs"]
mod ai_proxy_variant_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/request_access_policy_variant_tests.rs"]
mod request_access_policy_variant_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/canary_variant_tests.rs"]
mod canary_variant_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/dye_capability_tests.rs"]
mod dye_capability_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/dsl_body_requirement_tests.rs"]
mod dsl_body_requirement_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/wasm_body_gate_tests.rs"]
mod wasm_body_gate_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/request_body_plugin_requirement_tests.rs"]
mod request_body_plugin_requirement_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/edgion_plugin_test/transport_admission_tests.rs"]
mod transport_admission_tests;
