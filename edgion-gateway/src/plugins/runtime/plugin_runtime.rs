//! Plugin runtime - manages plugin execution across different stages

use edgion_resources::refs::config_data_fields::{request_access_has_external_data, HttpConfigDataEntry};
use edgion_resources::refs::ip_list_fields::{IpListSide, PreparedIpListReferences};
use edgion_resources::refs::waf_fields::WafPluginReferences;
use edgion_resources::resource::meta::ResourceMeta;
use edgion_resources::resources::refs::plugin_fields::{
    BoundManagedHttpConfig, HttpLinkSysConfig, PluginBindings, PluginMirrorRefs, PreparedPluginReferences,
};
use edgion_resources::resources::refs::route_fields::{EffectiveMirrorField, HttpRouteMirrorRefs};
use edgion_resources::EdgionPlugins;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::plugins::runtime::log::la;
use crate::runtime::server::http_to_grpc_status;
use crate::EdgionHttpContext;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::filters::PluginRunningResult::ErrTerminateRequest;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, CorsConfig, PluginValidation, WafConfig};
use edgion_resources::resources::header_modifier::{validate_grpc_filter_list, validate_http_filter_list};
use edgion_resources::resources::http_route::MirrorTuning;
use edgion_resources::resources::refs::extension_ref::ExtensionRefValidationError;
use edgion_resources::resources::refs::route_fields::{
    GrpcRouteExtensionRefs, HttpRouteExtensionRefs, RouteExtensionField,
};
use edgion_resources::resources::{
    EdgionPlugin, GRPCRoute, GRPCRouteFilter, GRPCRouteFilterType, HTTPRoute, HTTPRouteFilter, HTTPRouteFilterType,
};
use edgion_resources::resources::{
    RequestFilterEntry, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry, UpstreamResponseFilterEntry,
};

use super::body_gate::{BodyGatedRequestFilter, BodyGatedUpstreamResponse};
use super::conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseBodyFilter,
    ConditionalUpstreamResponseFilter,
};
use super::entry_bundle::{EntryBundle, EntryCache, EntryOutcome, EntrySlot, EntryStage};
use super::log::PluginLog;
use super::reject_filter::RejectFilter;
use super::session_adapter::PingoraSessionAdapter;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::ai::guards::semantic::Guardrail;
use crate::plugins::http::ai_proxy::AiProxy;
use crate::plugins::http::all_endpoint_status::AllEndpointStatus;
use crate::plugins::http::bandwidth_limit::BandwidthLimit;
use crate::plugins::http::basic_auth::BasicAuth;
use crate::plugins::http::canary::Canary;
use crate::plugins::http::cors::Cors;
use crate::plugins::http::credential_injector::CredentialInjector;
use crate::plugins::http::csrf::Csrf;
use crate::plugins::http::ctx_set::CtxSet;
use crate::plugins::http::direct_endpoint::DirectEndpoint;
use crate::plugins::http::dsl::plugin::DslPlugin;
use crate::plugins::http::dynamic_external_upstream::DynamicExternalUpstream;
use crate::plugins::http::dynamic_internal_upstream::DynamicInternalUpstream;
use crate::plugins::http::ext_proc::ExtProc;
use crate::plugins::http::fault_injection::FaultInjection;
use crate::plugins::http::form_json_transform::FormJsonTransform;
use crate::plugins::http::forward_auth::ForwardAuth;
use crate::plugins::http::geoip_location::GeoIpLocation;
use crate::plugins::http::graphql_guard::GraphqlGuard;
use crate::plugins::http::graphql_operation_router::GraphqlOperationRouter;
use crate::plugins::http::graphql_rate_limit::GraphqlRateLimit;
use crate::plugins::http::graphql_safelist::GraphqlSafelist;
use crate::plugins::http::grpc_json_transcoder::GrpcJsonTranscoder;
use crate::plugins::http::header_cert_auth::HeaderCertAuth;
use crate::plugins::http::hmac_auth::HmacAuth;
use crate::plugins::http::ip_restriction::IpRestriction;
use crate::plugins::http::json_schema_validation::JsonSchemaValidation;
use crate::plugins::http::jwe_decrypt::JweDecrypt;
use crate::plugins::http::jwt_auth::JwtAuth;
use crate::plugins::http::key_auth::KeyAuth;
use crate::plugins::http::ldap_auth::LdapAuth;
use crate::plugins::http::mock::Mock;
use crate::plugins::http::openid_connect::OpenidConnect;
use crate::plugins::http::proxy_rewrite::ProxyRewrite;
use crate::plugins::http::rate_limit_local::RateLimitLocal;
use crate::plugins::http::rate_limit_redis::RateLimitRedis;
use crate::plugins::http::real_ip::RealIp;
use crate::plugins::http::region_route::RegionRoute;
use crate::plugins::http::request_access_policy::RequestAccessPolicy;
use crate::plugins::http::request_body_buffer::RequestBodyBuffer;
use crate::plugins::http::request_id::RequestId;
use crate::plugins::http::request_mirror::RequestMirrorPlugin;
use crate::plugins::http::request_restriction::RequestRestriction;
use crate::plugins::http::response_rewrite::ResponseRewrite;
use crate::plugins::http::rest_to_graphql::RestToGraphql;
use crate::plugins::http::waf::WafPlugin;
use crate::plugins::http::wasm::WasmPlugin;
use crate::plugins::http::webhook_key_get::WebhookKeyGet;
use crate::plugins::http::AiGuard;
use crate::plugins::runtime::gateway_api_filters::{
    DebugAccessLogToHeaderFilter, ExtensionRefFilter, GapiCorsFilter, RequestHeaderModifierFilter,
    RequestRedirectFilter, ResponseHeaderModifierFilter, URLRewriteFilter,
};

#[inline]
fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(Instant::now().saturating_duration_since(started).as_micros()).unwrap_or(u64::MAX)
}

/// Immutable request-election metadata for one logical native WAF.
#[derive(Clone)]
pub(crate) struct WafCandidate {
    owner: Arc<()>,
    priority: u16,
    mode: crate::ctx::WafLogMode,
    /// Owner election across the levels of a delegated route chain.
    delegation_precedence: edgion_resources::resources::edgion_plugins::WafDelegationPrecedence,
}

impl WafCandidate {
    pub(crate) fn owner(&self) -> Arc<()> {
        Arc::clone(&self.owner)
    }

    pub(crate) const fn priority(&self) -> u16 {
        self.priority
    }

    pub(crate) const fn mode(&self) -> crate::ctx::WafLogMode {
        self.mode
    }

    /// Whether this level's WAF wins over every deeper level of a delegated chain.
    #[inline]
    pub(crate) fn takes_parent_precedence(&self) -> bool {
        matches!(
            self.delegation_precedence,
            edgion_resources::resources::edgion_plugins::WafDelegationPrecedence::Parent
        )
    }
}

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(0);

pub struct PluginRuntime {
    cache_identity: u64,
    cache_capabilities: super::entry_bundle::CacheCapabilities,
    required_ip: Vec<Arc<super::conditions::ip_refs::BoundIpPredicate>>,
    ip_view_required: bool,
    ip_unavailable_gate: Option<Arc<dyn RequestFilter>>,
    extension_checks: Vec<Arc<ExtensionRefFilter>>,
    /// Conservative construction-time capability, including conditional/nested consumers.
    requires_complete_request_body: bool,
    /// Effective executable membership, published only by successful typed construction.
    has_ai_conversion_owner: bool,
    has_unary_body_consumer: bool,
    rate_limit_local_outcome_slots: usize,
    has_upstream_jump_plugin: bool,
    has_grpc_transcoder: bool,
    has_graphql_operation_router: bool,
    backend_ext_ref_keys: Vec<String>,
    /// Plugins for request stage (async)
    request_plugins: Vec<Arc<dyn RequestFilter>>,
    /// The same native HTTP backend modifier held by `request_plugins`.
    /// AI dispatch borrows its prepared operations without running the async chain.
    backend_request_header_modifier: Option<Arc<RequestHeaderModifierFilter>>,
    /// Plugins for upstream_response_filter stage (sync)
    upstream_response_plugins: Vec<Arc<dyn UpstreamResponseFilter>>,
    /// Plugins for upstream_response_body_filter stage (sync, bandwidth throttling)
    upstream_response_body_plugins: Vec<Arc<dyn UpstreamResponseBodyFilter>>,
    /// Plugins for response_filter stage (async)
    upstream_response_async_plugins: Vec<Arc<dyn UpstreamResponse>>,
    /// Optional validated complete key of each ExtensionRef, in build order.
    /// Position == index assigned via `ExtensionRefFilter::set_index`. Empty
    /// when this runtime is built via the backend-scope path
    /// (`from_httproute_filters_for_backend` / `from_grpcroute_filters_for_backend`),
    /// because backend-scope ExtensionRefFilters keep `index = None`; their
    /// first result from the request-captured class scope, including a miss, is
    /// pinned in the logical request context instead.
    ext_ref_keys: Vec<Option<String>>,
    invalid_extension_gate: Option<Arc<dyn RequestFilter>>,
    /// Pre-resolved Arc to the first enabled `EdgionPlugin::Cors` config in
    /// `request_plugins`. Built once at preparse so the global CORS preflight
    /// handler (`pg_request_filter::extract_cors_config_from_route_filters`)
    /// can detect that the normal route plugin pipeline owns the preflight
    /// response. Hot-reload semantics are preserved: every
    /// `EdgionPlugins` update rebuilds this runtime, producing a new Arc that
    /// the plugin store swaps in atomically.
    cors_config: Option<Arc<CorsConfig>>,
    /// Names of plugins whose config failed `validation_error()` while building
    /// this runtime. Such plugins are NOT constructed (an invalid config must
    /// never run in the request path). `seal_fail_closed` consumes this list to
    /// prepend a single request-stage `RejectFilter`, so the whole route fails
    /// closed with 500 instead of silently dropping the misconfigured plugin.
    invalid_plugins: InvalidPlugins,
    /// Persistent construction outcome after the error list moves into RejectFilter.
    invalid_config_sealed: bool,
    /// Qualified names + status of policy-blocked entries, in spec order across
    /// all four stages (builders run request → urf → urbf → ur). The first
    /// element's status is the one the gate returns.
    policy_blocks: Vec<(String, u16)>,
    /// Qualified names of policy-bypassed entries.
    policy_bypassed: Vec<String>,
    /// Present only when this runtime owns one compiled logical WAF.
    waf_candidate: Option<WafCandidate>,
    has_request_id: bool,
    invalid_early_request_id: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestIdPhase {
    Normal,
    Early,
    AfterEarly,
}

/// Result of attempting to build a stage-specific filter from an EdgionPlugin.
///
/// Distinguishes "plugin variant does not belong to this stage" (a configuration
/// error surfaced to status.conditions) from "plugin is applicable but the
/// constructor declined to build it". The declined case is NOT assumed to be
/// covered by `EdgionPlugin::validation_error` — the builders record it
/// independently so the route still seals fail-closed.
enum CreateResult<T> {
    /// Plugin variant is applicable to this stage. `Some` on success, `None` if
    /// the inner constructor declined to build a filter (e.g. DSL VM init
    /// failure). The builders treat `None` as fail-closed and record the plugin
    /// in `invalid_plugins` regardless of whether `EdgionPlugin::validation_error`
    /// also caught the same config.
    Ok(Option<T>),
    /// Plugin variant does not belong to this stage (config error).
    WrongStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvalidUpdatePolicy {
    ReplaceFailClosed,
    RetainPrevious,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct InvalidPlugins {
    names: Vec<&'static str>,
    update_policy: Option<InvalidUpdatePolicy>,
}

impl InvalidPlugins {
    fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn record(&mut self, plugin: &EdgionPlugin) {
        let update_policy = match plugin {
            EdgionPlugin::Dsl(_) => InvalidUpdatePolicy::RetainPrevious,
            _ => InvalidUpdatePolicy::ReplaceFailClosed,
        };
        self.record_with_policy(PluginRuntime::get_plugin_name(plugin), update_policy);
    }

    fn record_non_retainable(&mut self, plugin_name: &'static str) {
        self.record_with_policy(plugin_name, InvalidUpdatePolicy::ReplaceFailClosed);
    }

    fn record_with_policy(&mut self, plugin_name: &'static str, update_policy: InvalidUpdatePolicy) {
        self.names.push(plugin_name);
        self.update_policy = Some(match (self.update_policy, update_policy) {
            (None, policy) => policy,
            (Some(InvalidUpdatePolicy::RetainPrevious), InvalidUpdatePolicy::RetainPrevious) => {
                InvalidUpdatePolicy::RetainPrevious
            }
            _ => InvalidUpdatePolicy::ReplaceFailClosed,
        });
    }

    fn update_policy(&self) -> Option<InvalidUpdatePolicy> {
        self.update_policy
    }
}

// Intentionally no `impl Clone for PluginRuntime`: a Clone impl would have to
// either deep-clone trait-object plugin chains (impossible without a per-plugin
// Clone bound) or silently return an empty runtime — the latter is a footgun
// that a misuse like `runtime_arc.as_ref().clone()` would silently strip the
// entire plugin chain from the request path with no compile-time or runtime
// signal. Sharing is done via `Arc<PluginRuntime>`; rebuild via the `from_*`
// constructors.
impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("request_plugins_count", &self.request_plugins.len())
            .field("upstream_response_plugins_count", &self.upstream_response_plugins.len())
            .field(
                "upstream_response_body_plugins_count",
                &self.upstream_response_body_plugins.len(),
            )
            .field(
                "upstream_response_async_plugins_count",
                &self.upstream_response_async_plugins.len(),
            )
            .field("invalid_plugins", &self.invalid_plugins)
            .field("has_waf", &self.waf_candidate.is_some())
            .finish()
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRuntime {
    pub fn new() -> Self {
        Self {
            cache_identity: NEXT_RUNTIME_ID
                .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
                .ok()
                .map_or(0, |id| id + 1),
            cache_capabilities: super::entry_bundle::CacheCapabilities::default(),
            has_upstream_jump_plugin: false,
            has_grpc_transcoder: false,
            has_graphql_operation_router: false,
            backend_ext_ref_keys: Vec::new(),
            request_plugins: vec![],
            backend_request_header_modifier: None,
            upstream_response_plugins: vec![],
            upstream_response_body_plugins: vec![],
            upstream_response_async_plugins: vec![],
            ext_ref_keys: Vec::new(),
            invalid_extension_gate: None,
            cors_config: None,
            invalid_plugins: InvalidPlugins::default(),
            invalid_config_sealed: false,
            policy_blocks: Vec::new(),
            policy_bypassed: Vec::new(),
            requires_complete_request_body: false,
            has_ai_conversion_owner: false,
            required_ip: Vec::new(),
            ip_view_required: false,
            ip_unavailable_gate: None,
            extension_checks: Vec::new(),
            has_unary_body_consumer: false,
            waf_candidate: None,
            has_request_id: false,
            invalid_early_request_id: false,
            rate_limit_local_outcome_slots: 0,
        }
    }

    fn into_entry_outcome(self, errors: Vec<String>) -> EntryOutcome {
        let failed = !self.invalid_plugins.is_empty();
        let bundle = EntryBundle {
            cache_capabilities: if failed || !errors.is_empty() {
                super::entry_bundle::CacheCapabilities {
                    safe: false,
                    credential_headers: Vec::new(),
                }
            } else {
                self.cache_capabilities
            },
            requires_complete_request_body: self.requires_complete_request_body,
            has_ai_conversion_owner: self.has_ai_conversion_owner,
            has_unary_body_consumer: self.has_unary_body_consumer,
            request: self.request_plugins,
            response_filter: self.upstream_response_plugins,
            response_body: self.upstream_response_body_plugins,
            response: self.upstream_response_async_plugins,
            cors: self.cors_config,
            waf: self.waf_candidate,
            invalid: self.invalid_plugins,
            errors,
        };
        if failed {
            EntryOutcome::Failed(bundle)
        } else {
            EntryOutcome::Ready(bundle)
        }
    }

    fn append_entry_bundle(&mut self, bundle: &EntryBundle) {
        self.cache_capabilities.combine(&bundle.cache_capabilities);
        self.requires_complete_request_body |= bundle.requires_complete_request_body;
        self.has_ai_conversion_owner |= bundle.has_ai_conversion_owner;
        self.has_unary_body_consumer |= bundle.has_unary_body_consumer;
        self.has_graphql_operation_router |= bundle.request.iter().any(|f| f.name() == "GraphqlOperationRouter");
        self.has_grpc_transcoder |= bundle
            .request
            .iter()
            .any(|filter| filter.name() == "GrpcJsonTranscoder");
        self.has_upstream_jump_plugin |= bundle.request.iter().any(|filter| {
            edgion_resources::resources::edgion_plugins::edgion_plugin::is_upstream_jump_plugin_type(filter.name())
        });
        self.request_plugins.extend(bundle.request.iter().cloned());
        self.upstream_response_plugins
            .extend(bundle.response_filter.iter().cloned());
        self.upstream_response_body_plugins
            .extend(bundle.response_body.iter().cloned());
        self.upstream_response_async_plugins
            .extend(bundle.response.iter().cloned());
        if self.cors_config.is_none() {
            self.cors_config = bundle.cors.clone();
        }
        if bundle.waf.is_some() {
            self.waf_candidate = bundle.waf.clone();
        }
        if let Some(policy) = bundle.invalid.update_policy() {
            for name in &bundle.invalid.names {
                self.invalid_plugins.record_with_policy(name, policy);
            }
        }
    }

    pub(crate) fn add_waf_with_cache(&mut self, owner: &EdgionPlugins, cache: &mut EntryCache) -> Vec<String> {
        let config = owner.spec.waf.as_ref().expect("WAF owner selected");
        // Inline rules are a non-serialized local inspection fixture. Resource
        // generations validate their actual policy field before cache access.
        if config.resolved_policy.is_some() || config.rules.trim().is_empty() {
            if let Err(error) = WafPluginReferences::from_plugins(owner).and_then(|fields| fields.policy().map(|_| ()))
            {
                self.invalid_plugins
                    .record_with_policy("Waf", InvalidUpdatePolicy::RetainPrevious);
                return vec![format!("waf: {error}")];
            }
        }
        let slot = EntrySlot {
            stage: EntryStage::Waf,
            index: 0,
        };
        if let Some(outcome) = cache.get(slot) {
            self.append_entry_bundle(outcome.bundle());
            return outcome.bundle().errors.clone();
        }
        let mut runtime = Self::new();
        let errors = runtime
            .add_waf(config)
            .err()
            .into_iter()
            .map(|error| format!("waf: {error}"))
            .collect::<Vec<_>>();
        let outcome = runtime.into_entry_outcome(errors.clone());
        self.append_entry_bundle(outcome.bundle());
        cache.insert(slot, outcome);
        errors
    }

    pub(crate) fn record_policy_action(
        &mut self,
        name: &str,
        action: edgion_resources::resources::plugin_policy::PluginPolicyAction,
    ) {
        use edgion_resources::resources::plugin_policy::PluginPolicyAction;
        let qualified = format!("http/{name}");
        match action {
            PluginPolicyAction::Block { status } => self.policy_blocks.push((qualified, status)),
            PluginPolicyAction::Bypass => self.policy_bypassed.push(qualified),
        }
    }

    /// Compile and attach one logical native WAF across the request-header,
    /// request-body, response-header, response-window and logging phases.
    /// Invalid updates retain the last known-good generation.
    pub fn add_waf(&mut self, config: &WafConfig) -> Result<(), String> {
        self.cache_capabilities.safe = false;
        match WafPlugin::try_new(config) {
            Ok(plugin) => {
                self.waf_candidate = Some(WafCandidate {
                    owner: plugin.owner(),
                    priority: plugin.priority(),
                    mode: match config.mode {
                        edgion_resources::resources::edgion_plugins::WafMode::On => crate::ctx::WafLogMode::On,
                        edgion_resources::resources::edgion_plugins::WafMode::DetectionOnly => {
                            crate::ctx::WafLogMode::DetectionOnly
                        }
                    },
                    delegation_precedence: config.delegation_precedence,
                });
                // The selected WAF owns its inspection policy; these hooks are
                // not an independent complete-body consumer.
                self.request_plugins.push(Arc::new(plugin.clone()));
                self.upstream_response_plugins.push(Arc::new(plugin.clone()));
                self.upstream_response_async_plugins.push(Arc::new(plugin));
                Ok(())
            }
            Err(error) => {
                self.invalid_plugins
                    .record_with_policy("Waf", InvalidUpdatePolicy::RetainPrevious);
                Err(error)
            }
        }
    }

    pub(crate) fn waf_candidate(&self) -> Option<&WafCandidate> {
        self.waf_candidate.as_ref()
    }

    pub(crate) fn configure_request_id_scope(&mut self, has_request_id: bool, invalid_early: bool) {
        self.has_request_id = has_request_id;
        self.invalid_early_request_id = invalid_early;
    }

    pub(crate) fn has_request_id(&self) -> bool {
        self.has_request_id
    }

    pub(crate) fn has_waf(&self) -> bool {
        self.waf_candidate.is_some()
    }

    pub(crate) fn has_graphql_operation_router(&self) -> bool {
        self.has_graphql_operation_router
    }

    pub(crate) fn requires_http_route_rule_scope(&self) -> bool {
        self.has_grpc_transcoder || self.has_graphql_operation_router
    }

    pub(crate) fn has_grpc_transcoder(&self) -> bool {
        self.has_grpc_transcoder
    }

    pub(crate) fn has_unary_body_consumer(&self) -> bool {
        self.has_unary_body_consumer
    }

    /// Inspect the same pinned target that ExtensionRef executes, before any
    /// selected-backend plugin or origin observes a converted or routed request.
    pub(crate) fn backend_compatible(&self, ctx: &mut EdgionHttpContext) -> bool {
        let plan = ctx.grpc_transcoding.clone();
        if plan.is_none() && !ctx.has_strict_jump() {
            return true;
        }
        let incompatible = |runtime: &Self| {
            runtime.has_ai_conversion_owner()
                || runtime.has_grpc_transcoder()
                || plan
                    .as_ref()
                    .is_some_and(|plan| !plan.streaming() && runtime.has_unary_body_consumer())
        };
        if incompatible(self) {
            if let Some(plan) = plan {
                plan.reject_mutation();
            }
            return false;
        }
        for key in &self.backend_ext_ref_keys {
            if ctx
                .resolve_pinned_backend_ext_ref(key)
                .is_some_and(|runtime| incompatible(&runtime))
            {
                if let Some(plan) = plan {
                    plan.reject_mutation();
                }
                return false;
            }
        }
        true
    }

    pub(crate) fn has_ai_conversion_owner(&self) -> bool {
        self.has_ai_conversion_owner
    }

    pub(crate) fn has_upstream_jump_plugin(&self) -> bool {
        self.has_upstream_jump_plugin
    }

    /// Whether runtime construction found a plugin that cannot be activated.
    ///
    /// This is queried once by the EdgionPlugins store before
    /// [`Self::seal_fail_closed`] consumes the list. It is not a request-path
    /// check.
    pub(crate) fn has_invalid_plugins(&self) -> bool {
        !self.invalid_plugins.is_empty()
    }

    /// Whether publication installed the invalid-configuration request gate.
    /// Direct consumers such as AI backend header dispatch do not run that chain.
    pub(crate) fn rejects_invalid_config(&self) -> bool {
        self.invalid_config_sealed
    }

    pub(crate) fn backend_request_header_modifier(&self) -> Option<&RequestHeaderModifierFilter> {
        self.backend_request_header_modifier.as_deref()
    }

    /// Record an active plugin entry whose conditions cannot be activated.
    ///
    /// The plugin store detects condition compile/validation failures and
    /// load-bearing unresolved references before runtime construction. These
    /// failures always replace the generation with the canonical fail-closed
    /// request gate, including when the wrapped plugin is DSL.
    pub(crate) fn record_invalid_condition(&mut self, plugin: &EdgionPlugin) {
        self.invalid_plugins
            .record_non_retainable(Self::get_plugin_name(plugin));
    }

    /// Record the shared validator's resource-level AiGuard/AiProxy ordering
    /// failure once, regardless of the number of offending entries. Ordering
    /// failures replace the generation even alongside invalid DSL/WAF programs.
    pub(crate) fn record_invalid_ai_guard_ordering(&mut self) {
        self.invalid_plugins.record_non_retainable("AiGuardOrdering");
    }

    /// Unknown source types must not become executable through enable or policy masks.
    pub(crate) fn record_unsupported_plugin(&mut self) {
        self.invalid_plugins.record_non_retainable("Unsupported");
    }

    /// Publication policy for an update that failed runtime construction.
    pub(crate) fn invalid_update_policy(&self) -> Option<InvalidUpdatePolicy> {
        self.invalid_plugins.update_policy()
    }

    /// A collision owns no executable candidate and cannot be policy-bypassed.
    pub(crate) fn ambiguous_reference() -> Self {
        let mut runtime = Self::default();
        runtime.invalid_plugins.record_non_retainable("AmbiguousEdgionPlugins");
        runtime.seal_fail_closed();
        runtime
    }

    /// Prepend a fail-closed `RejectFilter` to the request stage when any plugin
    /// in this runtime had an invalid config (recorded in `invalid_plugins`).
    ///
    /// Called once by `EdgionPlugins::preparse` after all four `add_from_*`
    /// builders have run. A single request-stage gate covers invalid plugins
    /// from *every* stage: the request stage always runs before upstream is
    /// contacted, so rejecting there also pre-empts a broken response/body-stage
    /// plugin. An in-slot reject per stage is not viable — the response/body
    /// stages cannot emit a clean 500 (see `check_response_stage_terminate`).
    /// The reject runs before any other plugin, so a request through a
    /// misconfigured EdgionPlugins is rejected with 500 before reaching upstream.
    pub fn seal_fail_closed(&mut self) {
        if self.invalid_plugins.is_empty() {
            return;
        }
        self.invalid_config_sealed = true;
        let reject = RejectFilter::new(std::mem::take(&mut self.invalid_plugins.names));
        self.request_plugins.insert(0, Arc::new(reject));
    }

    /// Prepend policy enforcement filters. MUST be called AFTER
    /// `seal_fail_closed` so the block gate lands ahead of the fail-closed
    /// RejectFilter (policy status code wins; spec §5 precedence).
    pub fn seal_plugin_policy(&mut self) {
        if !self.policy_bypassed.is_empty() {
            let note = super::policy_filter::PolicyBypassNote::new(std::mem::take(&mut self.policy_bypassed));
            // A bypass note is observational and must not hide the fail-closed
            // gate from the preflight fast path. Keep RejectInvalidConfig first
            // when sealing found an independent configuration error.
            let index = usize::from(self.invalid_config_sealed);
            self.request_plugins.insert(index, Arc::new(note));
        }
        if let Some((plugin, status)) = self.policy_blocks.first().cloned() {
            self.policy_blocks.clear();
            let gate = super::policy_filter::PolicyBlockFilter::new(plugin, status);
            self.request_plugins.insert(0, Arc::new(gate));
        }
    }

    /// Static list of (namespace, name) for every ExtensionRef in this runtime,
    /// in the order they were added. Position == `ExtensionRefFilter.index`.
    /// Populated only by `from_httproute_filters_for_rule` /
    /// `from_grpcroute_filters_for_rule`. Returns an empty slice for backend-scope.
    pub fn ext_ref_keys(&self) -> &[Option<String>] {
        &self.ext_ref_keys
    }

    /// Pre-resolved CORS config for the global preflight handler hot path.
    ///
    /// Returns `Some(Arc::clone)` of the first enabled `EdgionPlugin::Cors`
    /// entry in `request_plugins` if any, populated at preparse via
    /// `add_from_request_filters`. Cloning the returned `Arc` is cheap; the
    /// underlying `CorsConfig` is not deep-cloned per call.
    pub fn cors_config_arc(&self) -> Option<Arc<CorsConfig>> {
        self.cors_config.clone()
    }

    /// Build a runtime for a route rule. Assigns ExtensionRef indices for
    /// positional resolution from the request-captured class scope and records
    /// optional complete keys in `ext_ref_keys`.
    /// `mirror_tuning` carries the route-level `edgion.io/mirror-*` annotation
    /// values (pre-parsed onto the rule) applied to every RequestMirror filter.
    pub fn from_httproute_filters_for_rule(route: &HTTPRoute, rule: usize, mirror_tuning: &MirrorTuning) -> Self {
        Self::from_http_route_slot(route, rule, None, mirror_tuning)
    }

    /// Build selected-backend filters from their actual owning fields.
    pub fn from_httproute_filters_for_backend(
        route: &HTTPRoute,
        rule: usize,
        backend: usize,
        mirror_tuning: &MirrorTuning,
    ) -> Self {
        Self::from_http_route_slot(route, rule, Some(backend), mirror_tuning)
    }

    fn from_http_route_slot(
        route: &HTTPRoute,
        rule: usize,
        backend: Option<usize>,
        mirror_tuning: &MirrorTuning,
    ) -> Self {
        let mut runtime = Self::new();
        let owned = HttpRouteExtensionRefs::from_route(route).ok().and_then(|references| {
            let value = route.spec.rules.as_ref()?.get(rule)?;
            let filters = match backend {
                Some(index) => value
                    .backend_refs
                    .as_ref()?
                    .get(index)?
                    .filters
                    .as_deref()
                    .unwrap_or_default(),
                None => value.filters.as_deref().unwrap_or_default(),
            };
            Some((references, filters))
        });
        if let Some((references, filters)) = owned {
            runtime.add_from_httproute_filters_inner(
                filters,
                backend.is_none(),
                mirror_tuning,
                |filter| references.effective_at(rule, backend, filter),
                |filter| {
                    HttpRouteMirrorRefs::from_route(route)
                        .ok()?
                        .effective_at(rule, backend, filter)
                },
                |filter| {
                    edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs::from_route(route)?
                        .effective_at(rule, backend, filter)
                },
            );
        } else {
            runtime.invalid_plugins.record_non_retainable("ExtensionRef");
        }
        runtime.seal_fail_closed();
        runtime
    }

    fn add_from_httproute_filters_inner<'a>(
        &mut self,
        filters: &[HTTPRouteFilter],
        assign_indices: bool,
        mirror_tuning: &MirrorTuning,
        field: impl Fn(usize) -> Option<RouteExtensionField<'a>>,
        mirror_field: impl Fn(usize) -> Option<EffectiveMirrorField<'a>>,
        auth_field: impl Fn(
            usize,
        ) -> Result<
            edgion_resources::resources::refs::route_auth_fields::EffectiveRouteAuthField<'a>,
            edgion_resources::resources::refs::ReferenceError,
        >,
    ) {
        if filters.iter().enumerate().any(|(index, _)| {
            matches!(
                field(index).map(|field| field.validated()),
                None | Some(Err(ExtensionRefValidationError::MissingConfiguration))
                    | Some(Err(ExtensionRefValidationError::UnexpectedConfiguration))
            )
        }) {
            self.invalid_plugins.record_non_retainable("ExtensionRef");
            return;
        }
        if filters.iter().any(|filter| {
            filter.filter_type != HTTPRouteFilterType::ExternalAuth
                && (filter.external_auth.is_some() || filter.resolved_external_auth.is_some())
        }) {
            self.invalid_plugins.record_non_retainable("ForwardAuth");
            return;
        }
        if validate_http_filter_list(filters).is_err() {
            self.invalid_plugins.record_non_retainable("HeaderModifier");
            return;
        }
        if !matches!(
            edgion_resources::request_transform_filter_list_violations(filters),
            Ok(violations) if violations.is_empty()
        ) {
            self.invalid_plugins.record_non_retainable("RequestTransform");
            return;
        }
        for (filter_index, filter) in filters.iter().enumerate() {
            self.cache_capabilities.safe &= matches!(
                filter.filter_type,
                HTTPRouteFilterType::RequestHeaderModifier
                    | HTTPRouteFilterType::ResponseHeaderModifier
                    | HTTPRouteFilterType::RequestRedirect
                    | HTTPRouteFilterType::URLRewrite
                    | HTTPRouteFilterType::CORS
                    | HTTPRouteFilterType::ExtensionRef
            );
            match filter.filter_type {
                HTTPRouteFilterType::RequestHeaderModifier => {
                    if let Some(config) = &filter.request_header_modifier {
                        match RequestHeaderModifierFilter::new(config.clone()) {
                            Ok(filter) => {
                                let filter = Arc::new(filter);
                                if !assign_indices {
                                    self.backend_request_header_modifier = Some(Arc::clone(&filter));
                                }
                                self.requires_complete_request_body = true;
                                self.request_plugins.push(filter);
                            }
                            Err(_) => self.invalid_plugins.record_non_retainable("RequestHeaderModifier"),
                        }
                    }
                }
                HTTPRouteFilterType::ResponseHeaderModifier => {
                    if let Some(config) = &filter.response_header_modifier {
                        match ResponseHeaderModifierFilter::new(config.clone()) {
                            Ok(filter) => self.add_upstream_response_filter(Box::new(filter)),
                            Err(_) => self.invalid_plugins.record_non_retainable("ResponseHeaderModifier"),
                        }
                    }
                }
                HTTPRouteFilterType::RequestRedirect => {
                    if let Some(config) = &filter.request_redirect {
                        match RequestRedirectFilter::try_new(config) {
                            Ok(filter) => self.add_request_filter(Box::new(filter)),
                            Err(_) => self.invalid_plugins.record_non_retainable("RequestRedirect"),
                        }
                    }
                }
                HTTPRouteFilterType::URLRewrite => {
                    if let Some(config) = &filter.url_rewrite {
                        match URLRewriteFilter::try_new(config) {
                            Ok(filter) => self.add_request_filter(Box::new(filter)),
                            Err(_) => self.invalid_plugins.record_non_retainable("URLRewrite"),
                        }
                    }
                }
                HTTPRouteFilterType::RequestMirror => {
                    if let Some(plugin) = mirror_field(filter_index)
                        .and_then(|field| RequestMirrorPlugin::new(field, mirror_tuning.clone()))
                    {
                        self.add_request_filter(Box::new(plugin));
                    } else {
                        self.invalid_plugins.record_non_retainable("RequestMirror");
                    }
                }

                HTTPRouteFilterType::CORS => {
                    if let Some(config) = &filter.cors {
                        // Controller status and Gateway route publication both reject
                        // invalid CORS rules before runtime compilation. Keep try_new as
                        // a final defensive boundary for direct construction.
                        if let Ok(cors_filter) = GapiCorsFilter::try_new(config.clone()) {
                            self.add_request_filter(Box::new(cors_filter.clone()));
                            self.add_upstream_response_filter(Box::new(cors_filter));
                        }
                    }
                }
                HTTPRouteFilterType::ExternalAuth => {
                    let result = auth_field(filter_index)
                        .map_err(|error| error.to_string())
                        .and_then(|field| {
                            let defaults = edgion_common::link_sys_core::outbound_tls::outbound_global_default();
                            let input = crate::plugins::http::common::outbound::client::PreparedRouteAuth::new(
                                &field, &defaults,
                            )?;
                            ForwardAuth::from_route(&input)
                        });
                    match result {
                        Ok(auth) => self.add_request_filter(Box::new(auth)),
                        Err(_) => self.invalid_plugins.record_non_retainable("ForwardAuth"),
                    }
                }
                HTTPRouteFilterType::ExtensionRef => {
                    // Unsupported targets retain the existing ExtensionRef
                    // runtime so their kind-specific diagnostics remain
                    // attributable. Missing payloads were rejected above.
                    if let Some(field) = field(filter_index) {
                        self.add_extension_field(
                            field,
                            filter
                                .extension_ref
                                .as_ref()
                                .map_or("", |reference| reference.name.as_str()),
                            assign_indices,
                        );
                    }
                }
            }
        }
    }

    /// Build a runtime for a gRPC route rule. Assigns ExtensionRef indices for
    /// positional resolution from the request-captured class scope and records
    /// optional complete keys in `ext_ref_keys`.
    pub fn from_grpcroute_filters_for_rule(route: &GRPCRoute, rule: usize) -> Self {
        Self::from_grpc_route_slot(route, rule, None)
    }

    /// Build selected-backend filters from their actual owning fields.
    pub fn from_grpcroute_filters_for_backend(route: &GRPCRoute, rule: usize, backend: usize) -> Self {
        Self::from_grpc_route_slot(route, rule, Some(backend))
    }

    fn from_grpc_route_slot(route: &GRPCRoute, rule: usize, backend: Option<usize>) -> Self {
        let mut runtime = Self::new();
        let owned = GrpcRouteExtensionRefs::from_route(route).ok().and_then(|references| {
            let value = route.spec.rules.as_ref()?.get(rule)?;
            let filters = match backend {
                Some(index) => value
                    .backend_refs
                    .as_ref()?
                    .get(index)?
                    .filters
                    .as_deref()
                    .unwrap_or_default(),
                None => value.filters.as_deref().unwrap_or_default(),
            };
            Some((references, filters))
        });
        if let Some((references, filters)) = owned {
            runtime.add_from_grpcroute_filters_inner(filters, backend.is_none(), |filter| {
                references.effective_at(rule, backend, filter)
            });
        } else {
            runtime.invalid_plugins.record_non_retainable("ExtensionRef");
        }
        runtime.seal_fail_closed();
        runtime
    }

    fn add_from_grpcroute_filters_inner<'a>(
        &mut self,
        filters: &[GRPCRouteFilter],
        assign_indices: bool,
        field: impl Fn(usize) -> Option<RouteExtensionField<'a>>,
    ) {
        if filters.iter().enumerate().any(|(index, _)| {
            matches!(
                field(index).map(|field| field.validated()),
                None | Some(Err(ExtensionRefValidationError::MissingConfiguration))
                    | Some(Err(ExtensionRefValidationError::UnexpectedConfiguration))
            )
        }) {
            self.invalid_plugins.record_non_retainable("ExtensionRef");
            return;
        }
        if validate_grpc_filter_list(filters).is_err() {
            self.invalid_plugins.record_non_retainable("HeaderModifier");
            return;
        }
        for (filter_index, filter) in filters.iter().enumerate() {
            match filter.filter_type {
                GRPCRouteFilterType::RequestHeaderModifier => {
                    if let Some(config) = &filter.request_header_modifier {
                        match RequestHeaderModifierFilter::new_from_grpc(config.clone()) {
                            Ok(filter) => self.add_request_filter(Box::new(filter)),
                            Err(_) => self.invalid_plugins.record_non_retainable("RequestHeaderModifier"),
                        }
                    }
                }
                GRPCRouteFilterType::ResponseHeaderModifier => {
                    if let Some(config) = &filter.response_header_modifier {
                        match ResponseHeaderModifierFilter::new_from_grpc(config.clone()) {
                            Ok(filter) => self.add_upstream_response_filter(Box::new(filter)),
                            Err(_) => self.invalid_plugins.record_non_retainable("ResponseHeaderModifier"),
                        }
                    }
                }
                GRPCRouteFilterType::ExtensionRef => {
                    if let Some(field) = field(filter_index) {
                        self.add_extension_field(
                            field,
                            filter
                                .extension_ref
                                .as_ref()
                                .map_or("", |reference| reference.name.as_str()),
                            assign_indices,
                        );
                    }
                }
            }
        }
    }

    fn add_extension_field(&mut self, field: RouteExtensionField<'_>, diagnostic_name: &str, assign_indices: bool) {
        let mut filter = ExtensionRefFilter::new(field, diagnostic_name);
        if assign_indices {
            filter.set_index(self.ext_ref_keys.len());
            self.ext_ref_keys.push(filter.plugin_key().map(str::to_owned));
        } else if let Some(key) = filter.plugin_key() {
            self.backend_ext_ref_keys.push(key.to_owned());
        }
        let invalid = filter.plugin_key().is_none();
        let filter = Arc::new(filter);
        self.extension_checks.push(filter.clone());
        self.ensure_ip_unavailable_gate();
        let request: Arc<dyn RequestFilter> = filter.clone();
        if assign_indices && invalid && self.invalid_extension_gate.is_none() {
            self.invalid_extension_gate = Some(Arc::clone(&request));
        }
        self.request_plugins.push(request);
        self.upstream_response_plugins.push(filter.clone());
        self.upstream_response_body_plugins.push(filter.clone());
        self.upstream_response_async_plugins.push(filter);
    }

    /// Add request filters from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalRequestFilter to support condition-based execution.
    /// Gateway API filters (via add_from_httproute_filters_inner — invoked through
    /// `from_httproute_filters_for_rule` / `_for_backend`) are NOT wrapped to maintain compatibility.
    ///
    /// # Arguments
    /// * `entries` - The request filter entries
    /// * `namespace` - The namespace of the EdgionPlugins resource
    ///
    /// # Returns
    /// A vector of validation error messages (empty if all plugins are valid)
    pub(crate) fn add_resource_filters_with_cache(
        &mut self,
        owner: &edgion_resources::EdgionPlugins,
        cache: &mut EntryCache,
    ) -> Vec<String> {
        let ip = match super::conditions::ip_refs::IpConditionCompiler::new(owner) {
            Ok(ip) => Some(ip),
            Err(_) => {
                self.invalid_plugins.record_non_retainable("IpList");
                None
            }
        };
        self.ip_view_required = ip.as_ref().is_some_and(|ip| ip.has_refs());
        self.required_ip = ip.as_ref().map(|ip| ip.required()).unwrap_or_default();
        if !self.required_ip.is_empty() {
            self.ensure_ip_unavailable_gate();
        }
        let prepared = PreparedPluginReferences::from_plugins(owner);
        let bindings = prepared.bind(owner).ok();
        let namespace = owner.metadata.namespace.as_deref().unwrap_or("default");
        let mut errors = self.add_request_entries(
            owner.spec.request_plugins.as_deref().unwrap_or_default(),
            namespace,
            &owner.key_name(),
            cache,
            Some(owner),
            bindings.as_ref(),
            ip.as_ref(),
        );
        errors.extend(
            self.add_response_filter_entries(
                owner
                    .spec
                    .upstream_response_filter_plugins
                    .as_deref()
                    .unwrap_or_default(),
                namespace,
                cache,
                bindings.as_ref(),
                ip.as_ref(),
            ),
        );
        errors.extend(
            self.add_from_upstream_response_body_filters_with_cache(
                owner
                    .spec
                    .upstream_response_body_filter_plugins
                    .as_deref()
                    .unwrap_or_default(),
                namespace,
                cache,
                bindings.as_ref(),
                ip.as_ref(),
            ),
        );
        errors.extend(self.add_from_upstream_responses_with_cache(
            owner.spec.upstream_response_plugins.as_deref().unwrap_or_default(),
            namespace,
            cache,
            bindings.as_ref(),
            ip.as_ref(),
        ));
        errors
    }

    // Keep independently borrowed owner capabilities explicit at this cold boundary.
    #[allow(clippy::too_many_arguments)]
    fn add_request_entries(
        &mut self,
        entries: &[RequestFilterEntry],
        namespace: &str,
        resource_key: &str,
        cache: &mut EntryCache,
        owner: Option<&edgion_resources::EdgionPlugins>,
        bindings: Option<&PluginBindings<'_>>,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        // Bind once to the actual equal-length policy view. Never synthesize an owner
        // from resource_key; all indexed entries retain their original positions.
        let ip_prepared = owner.map(PreparedIpListReferences::from_plugins);
        let ip_binding = ip_prepared
            .as_ref()
            .zip(owner)
            .and_then(|(prepared, owner)| prepared.bind_plugins(owner).ok());
        let descriptor_entries: Vec<_> = bindings
            .into_iter()
            .flat_map(|bindings| bindings.request_entries())
            .collect();
        let descriptor_errors: Vec<_> = entries.iter().enumerate().map(|(index, entry)| {
            if !entry.is_enabled() || !matches!(&entry.plugin, EdgionPlugin::GrpcJsonTranscoder(config)
                if matches!(config.descriptor, edgion_resources::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef { .. })) {
                return None;
            }
            descriptor_entries.get(index).ok_or_else(|| "invalid owning descriptor reference".to_string())
                .and_then(GrpcJsonTranscoder::bound_config).err()
        }).collect();
        // Keep source aliases for dependency semantics even when an invalid or
        // duplicate display identity has been sanitized by the generation.
        let outcome_aliases_owned = cache
            .prepare_request_outcomes(entries, self.rate_limit_local_outcome_slots)
            .clone();
        let source_aliases = cache.request_source_aliases().to_vec();
        edgion_resources::resources::edgion_plugins::validate::collect_rate_limit_dependency_errors_with_aliases(
            entries,
            |index| source_aliases[index].as_deref(),
            &mut errors,
        );
        if !errors.is_empty() {
            self.invalid_plugins.record_non_retainable("RateLimitRedis");
        }
        // Binding belongs to the immutable source, not the first policy view
        // that happens to construct a shared producer or consumer.
        let outcome_aliases: std::collections::HashMap<&str, usize> = outcome_aliases_owned
            .iter()
            .map(|(alias, slot)| (alias.as_str(), *slot))
            .collect();
        for (index, entry) in entries.iter().enumerate().filter(|(_, entry)| entry.is_enabled()) {
            if matches!(entry.plugin, EdgionPlugin::RateLimitLocal(_)) {
                if let Some(slot) = source_aliases[index]
                    .as_deref()
                    .and_then(|alias| outcome_aliases.get(alias))
                {
                    self.rate_limit_local_outcome_slots = self.rate_limit_local_outcome_slots.max(slot + 1);
                }
            }
        }
        let transcoding_errors =
            edgion_resources::resources::edgion_plugins::validate::collect_grpc_transcoding_errors(entries);
        if !transcoding_errors.is_empty() {
            self.invalid_plugins.record_non_retainable("GrpcJsonTranscoder");
            errors.extend(transcoding_errors);
        }
        self.has_graphql_operation_router |= entries
            .iter()
            .any(|e| e.is_enabled() && matches!(e.plugin, EdgionPlugin::GraphqlOperationRouter(_)));
        self.has_grpc_transcoder |= entries
            .iter()
            .any(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::GrpcJsonTranscoder(_)));
        let mut invalid_bounded_structured_entries = vec![false; entries.len()];
        // Freeze config validation before the bounded-body prepass. A config
        // error owns the entry and suppresses later body-policy errors, matching
        // `EdgionPlugins::validate_plugins` and avoiding Controller/Gateway
        // error-set drift when both parts are invalid.
        let config_validation_errors: Vec<Option<String>> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                if entry.is_enabled() {
                    descriptor_errors[index]
                        .clone()
                        .or_else(|| Self::plugin_validation_error(&entry.plugin))
                } else {
                    None
                }
            })
            .collect();

        // Mirror the schema-layer JsonSchemaValidation list/entry invariants
        // before constructing any filters. The Gateway must remain fail-closed
        // even if an invalid resource reaches it without Controller admission.
        // Disabled entries in the effective view are inert and do not
        // participate.
        let json_schema_count = entries
            .iter()
            .filter(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::JsonSchemaValidation(_)))
            .count();
        if json_schema_count > 1 {
            errors.push(format!(
                "requestPlugins: at most one JsonSchemaValidation plugin is allowed, found {json_schema_count}"
            ));
            self.invalid_plugins.record_non_retainable("JsonSchemaValidation");
        }

        for (index, entry) in entries.iter().enumerate() {
            if !entry.is_enabled()
                || config_validation_errors[index].is_some()
                || !matches!(
                    entry.plugin,
                    EdgionPlugin::FormJsonTransform(_)
                        | EdgionPlugin::GrpcJsonTranscoder(_)
                        | EdgionPlugin::JsonSchemaValidation(_)
                        | EdgionPlugin::RestToGraphql(_)
                        | EdgionPlugin::GraphqlGuard(_)
                        | EdgionPlugin::GraphqlSafelist(_)
                        | EdgionPlugin::GraphqlRateLimit(_)
                        | EdgionPlugin::GraphqlOperationRouter(_)
                        | EdgionPlugin::ProxyRewrite(edgion_resources::resources::edgion_plugins::ProxyRewriteConfig {
                            json_body: Some(_),
                            ..
                        })
                )
            {
                continue;
            }
            let Some(body) = entry.body.as_ref() else {
                continue;
            };
            let body_errors = entry.plugin.structured_body_validation_errors(body);
            if body_errors.is_empty() {
                continue;
            }
            invalid_bounded_structured_entries[index] = true;
            let plugin_name = Self::get_plugin_name(&entry.plugin);
            self.invalid_plugins.record(&entry.plugin);
            errors.extend(
                body_errors
                    .into_iter()
                    .map(|error| format!("requestPlugins[{}] ({}): {}", index, plugin_name, error)),
            );
        }

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                let link_sys_config = if HttpLinkSysConfig::is_reference_bearing(&entry.plugin) {
                    match descriptor_entries
                        .get(index)
                        .and_then(|entry| entry.link_sys_config().ok())
                        .flatten()
                    {
                        Some(config) => Some(config),
                        None => {
                            let name = Self::get_plugin_name(&entry.plugin);
                            self.invalid_plugins.record_non_retainable(name);
                            errors.push(format!(
                                "requestPlugins[{index}] ({name}): invalid owning LinkSys reference"
                            ));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let oidc_defaults = matches!(entry.plugin, EdgionPlugin::OpenidConnect(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = oidc_defaults.as_deref() {
                    let result = descriptor_entries
                        .get(index)
                        .ok_or_else(|| "openid_connect outbound HTTP client build failed".to_string())
                        .and_then(|entry| OpenidConnect::validate_tls_entry(entry, defaults));
                    if let Err(error) = result {
                        self.invalid_plugins.record_non_retainable("OpenidConnect");
                        errors.push(format!(
                            "requestPlugins[{index}] (OpenidConnect): {}",
                            config_validation_errors[index].as_deref().unwrap_or(&error)
                        ));
                        continue;
                    }
                }
                let ldap_defaults = matches!(entry.plugin, EdgionPlugin::LdapAuth(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = ldap_defaults.as_deref() {
                    let result = descriptor_entries
                        .get(index)
                        .ok_or_else(|| "LdapAuth TLS configuration error: invalid owning TLS reference".to_string())
                        .and_then(|entry| LdapAuth::validate_tls_entry(entry, defaults));
                    if let Err(error) = result {
                        self.invalid_plugins.record_non_retainable("LdapAuth");
                        errors.push(format!(
                            "requestPlugins[{index}] (LdapAuth): {}",
                            config_validation_errors[index].as_deref().unwrap_or(&error)
                        ));
                        continue;
                    }
                }
                let wasm_defaults = matches!(entry.plugin, EdgionPlugin::Wasm(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = wasm_defaults.as_deref() {
                    if descriptor_entries
                        .get(index)
                        .ok_or_else(|| "wasm TLS configuration is invalid".to_string())
                        .and_then(|entry| WasmPlugin::validate_tls_entry(entry, defaults))
                        .is_err()
                    {
                        self.invalid_plugins.record_non_retainable("Wasm");
                        errors.push(format!(
                            "requestPlugins[{index}] (Wasm): {}",
                            config_validation_errors[index]
                                .as_deref()
                                .unwrap_or("plugin construction failed")
                        ));
                        continue;
                    }
                }
                let ext_proc_defaults = matches!(entry.plugin, EdgionPlugin::ExtProc(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = ext_proc_defaults.as_deref() {
                    let result = descriptor_entries
                        .get(index)
                        .ok_or_else(|| "ext_proc TLS configuration is invalid".to_string())
                        .and_then(|entry| ExtProc::validate_tls_entry(entry, defaults));
                    if result.is_err() {
                        self.invalid_plugins.record_non_retainable("ExtProc");
                        errors.push(format!(
                            "requestPlugins[{index}] (ExtProc): {}",
                            config_validation_errors[index]
                                .as_deref()
                                .unwrap_or("plugin construction failed")
                        ));
                        continue;
                    }
                }
                let header_ca_field = if matches!(entry.plugin, EdgionPlugin::HeaderCertAuth(_)) {
                    match descriptor_entries
                        .get(index)
                        .and_then(|entry| entry.header_cert_auth().ok())
                        .flatten()
                    {
                        Some(field) => Some(field),
                        None => {
                            self.invalid_plugins.record_non_retainable("HeaderCertAuth");
                            errors.push(format!(
                                "requestPlugins[{index}] (HeaderCertAuth): {}",
                                config_validation_errors[index]
                                    .as_deref()
                                    .unwrap_or("invalid owning CA reference")
                            ));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let managed_field = if BoundManagedHttpConfig::is_reference_bearing(&entry.plugin) {
                    match descriptor_entries
                        .get(index)
                        .and_then(|entry| entry.managed_http_config().ok())
                        .flatten()
                    {
                        Some(field) => Some(field),
                        None => {
                            let name = Self::get_plugin_name(&entry.plugin);
                            self.invalid_plugins.record_non_retainable(name);
                            errors.push(format!(
                                "requestPlugins[{index}] ({name}): invalid owning HTTP reference"
                            ));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let managed_defaults = managed_field
                    .as_ref()
                    .map(|_| edgion_common::link_sys_core::outbound_tls::outbound_global_default());
                let managed_input = match managed_field.as_ref().zip(managed_defaults.as_deref()) {
                    Some((field, defaults)) => {
                        match crate::plugins::http::common::outbound::client::PreparedManagedHttp::new(field, defaults)
                        {
                            Ok(input) => Some(input),
                            Err(_) => {
                                let name = Self::get_plugin_name(&entry.plugin);
                                self.invalid_plugins.record_non_retainable(name);
                                errors.push(format!(
                                    "requestPlugins[{index}] ({name}): invalid owning HTTP configuration"
                                ));
                                continue;
                            }
                        }
                    }
                    None => None,
                };
                if invalid_bounded_structured_entries[index] {
                    continue;
                }

                let slot = EntrySlot {
                    stage: EntryStage::Request,
                    index,
                };
                let mirror_field = owner.and_then(|owner| PluginMirrorRefs::from_plugins(owner).effective_at(index));
                if matches!(entry.plugin, EdgionPlugin::RequestMirror(_))
                    && !matches!(mirror_field.map(|field| field.validated()), Some(Ok(Some(_))))
                {
                    self.invalid_plugins.record_non_retainable("RequestMirror");
                    errors.push(format!(
                        "requestPlugins[{index}] (RequestMirror): {}",
                        config_validation_errors[index]
                            .as_deref()
                            .unwrap_or("invalid owning mirror reference")
                    ));
                    continue;
                }
                if matches!(entry.plugin, EdgionPlugin::IpRestriction(_))
                    && owner.is_some()
                    && ip_binding
                        .as_ref()
                        .and_then(|binding| binding.http_entry(index))
                        .is_none_or(|entry| {
                            let refs = entry.references();
                            entry.config().validation_error().is_some()
                                || refs.validate_owner().is_err()
                                || refs.read_scopes(IpListSide::Allow).is_err()
                                || refs.read_scopes(IpListSide::Deny).is_err()
                                || ip
                                    .as_ref()
                                    .is_none_or(|compiler| compiler.bind_acl(&entry.config().refs).is_none())
                        })
                {
                    self.invalid_plugins.record_non_retainable("IpRestriction");
                    errors.push(format!(
                        "requestPlugins[{index}] (IpRestriction): {}",
                        config_validation_errors[index]
                            .as_deref()
                            .unwrap_or("invalid owning IP reference")
                    ));
                    continue;
                }
                if let Some(error) = &descriptor_errors[index] {
                    self.invalid_plugins.record_non_retainable("GrpcJsonTranscoder");
                    errors.push(format!("requestPlugins[{index}] (GrpcJsonTranscoder): {error}"));
                    continue;
                }
                let data_entry = owner.and_then(|owner| HttpConfigDataEntry::from_plugins(owner, index));
                let data_ref_bearing = match &entry.plugin {
                    EdgionPlugin::RequestAccessPolicy(config) => request_access_has_external_data(config),
                    EdgionPlugin::Canary(config) => config.active_profile_ref.is_some(),
                    EdgionPlugin::RegionRoute(_) => true,
                    _ => false,
                };
                if data_ref_bearing && data_entry.is_none_or(|entry| entry.validate_owner().is_err()) {
                    let name = Self::get_plugin_name(&entry.plugin);
                    self.invalid_plugins.record_non_retainable(name);
                    errors.push(format!(
                        "requestPlugins[{index}] ({name}): invalid owning ConfigData reference"
                    ));
                    continue;
                }
                // An invalid/denied target must rebuild its consumer-specific
                // deny/base result rather than reuse a cached external key.
                let data_cache_reusable = data_entry
                    .and_then(|entry| entry.field())
                    .is_none_or(|field| matches!(field.runtime_key(), Ok(Some(_))));
                if data_cache_reusable && header_ca_field.as_ref().is_none_or(|field| field.cache_reusable()) {
                    if let Some(outcome) = cache.get(slot) {
                        self.append_entry_bundle(outcome.bundle());
                        errors.extend(outcome.bundle().errors.iter().cloned());
                        continue;
                    }
                }
                let mut entry_runtime = Self::new();
                let errors_before = errors.len();
                'build_entry: {
                    // Reject plugins whose config fails validation: record the name,
                    // surface the error, and skip construction so an invalid config
                    // can never run in the request path. `seal_fail_closed` later
                    // turns a non-empty `invalid_plugins` into a fail-closed 500.
                    if let Some(error) = &config_validation_errors[index] {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, error));
                        entry_runtime.invalid_plugins.record(&entry.plugin);
                        break 'build_entry;
                    }

                    if let EdgionPlugin::RegionRoute(config) = &entry.plugin {
                        if let Some(error) = config.operational_validation_error() {
                            errors.push(format!("requestPlugins[{index}] (RegionRoute): {error}"));
                        }
                    }

                    // A Deny restriction with any unresolved condition reference
                    // is invalid as a whole. Record it before entry conditions are
                    // wrapped so an outer skip can never bypass the generic 500.
                    if let EdgionPlugin::RequestRestriction(config) = &entry.plugin {
                        if RequestRestriction::would_fail_construction(config) {
                            errors.push(format!(
                                "requestPlugins[{}] (RequestRestriction): restriction config invalid",
                                index
                            ));
                            entry_runtime
                                .invalid_plugins
                                .record_non_retainable("RequestRestriction");
                            break 'build_entry;
                        }
                    }

                    // Cache the first enabled CORS config so the global preflight
                    // handler can delegate to the normal route plugin pipeline.
                    // Initialize the `#[serde(skip)]` runtime fields (origins_cache /
                    // compiled_origins_regex / compiled_timing_regex) here: the
                    // cached representation stays ready for any config inspection,
                    // even though the preflight handler currently uses only its
                    // presence. On failure keep the uncompiled (deny-safe) config.
                    if entry_runtime.cors_config.is_none() {
                        if let EdgionPlugin::Cors(cfg) = &entry.plugin {
                            let mut cfg = cfg.clone();
                            if let Err(e) = cfg.validate_and_init() {
                                tracing::warn!(
                                    error = %e,
                                    label = "HTTP plugin",
                                    "Failed to initialize cached CorsConfig for preflight handler; \
                                     regex/cache origins will be ignored (deny-safe)",
                                );
                            }
                            entry_runtime.cors_config = Some(Arc::new(cfg));
                        }
                    }

                    // Validate optional entry-level alias. On failure, surface
                    // the error and drop the alias so an unvalidated value never
                    // reaches the access log.
                    let mut entry_alias = match entry.validate_alias() {
                        None => entry.alias.clone(),
                        Some(err) => {
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, err));
                            None
                        }
                    };

                    // Alias uniqueness within this stage array: the alias is the
                    // discriminator in the entry identity that namespaces state kept
                    // outside the plugin instance (RateLimitRedis buckets), so a
                    // duplicate must not survive into the runtime.
                    if let Some(err) = edgion_resources::resources::edgion_plugins::duplicate_alias_error(
                        &mut seen_aliases,
                        entry_alias.as_deref(),
                    ) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, err));
                        // Drop it exactly like an invalid alias, so the entry falls back
                        // to the `#index` form of its scope. Reporting alone would not be
                        // enough: lenient admission keeps an `Accepted=False` resource
                        // synced and serving, and the two entries would still share one
                        // bucket namespace and one access-log label.
                        entry_alias = None;
                    }

                    // Validate the optional entry-level dye suite. Same severity as
                    // the alias check above: non-fatal, surfaced as a preparse error.
                    // Only compile a runtime plan when the schema-level check passed
                    // (see `compile_entry_dye_plan` doc comment for the severity split).
                    let dye_validation_err = entry.validate_dye();
                    if let Some(err) = &dye_validation_err {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, err));
                    }
                    let dye_plan = if dye_validation_err.is_none() {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        Self::compile_entry_dye_plan(
                            entry.dye.as_ref(),
                            &entry.plugin,
                            &format!("requestPlugins[{}] ({})", index, plugin_name),
                            &mut errors,
                            &mut entry_runtime.invalid_plugins,
                            ip,
                        )
                    } else {
                        None
                    };

                    // Validate the optional operator body requirement. `body:` only
                    // applies to plugins that consume the body in their current config;
                    // reject it elsewhere so it is never a silent no-op. Non-fatal: the
                    // plugin still runs (ungated), but the error is surfaced.
                    if entry.body.is_some() {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        if !entry
                            .plugin
                            .accepts_body(edgion_resources::resources::edgion_plugins::BodyStage::Request)
                        {
                            errors.push(format!(
                                "requestPlugins[{}] ({}): body: plugin does not consume a body",
                                index, plugin_name
                            ));
                        } else if let Some(err) = entry.body.as_ref().and_then(|b| b.validation_error()) {
                            errors.push(format!("requestPlugins[{}] ({}): body: {}", index, plugin_name, err));
                        }
                    }

                    let body_req = Self::resolve_body_requirement(
                        &entry.plugin,
                        entry.body.clone(),
                        edgion_resources::resources::edgion_plugins::BodyStage::Request,
                    );

                    let entry_scope = crate::plugins::common::entry_scope(resource_key, entry_alias.as_deref(), index);

                    if matches!(&entry.plugin, EdgionPlugin::Wasm(config) if config.response.enabled()) {
                        match descriptor_entries
                            .get(index)
                            .zip(wasm_defaults.as_deref())
                            .and_then(|(entry, defaults)| WasmPlugin::create_attached(entry, defaults))
                        {
                            Some((request, response)) => {
                                entry_runtime.has_unary_body_consumer =
                                    matches!(&entry.plugin, EdgionPlugin::Wasm(config) if config.response.body);
                                let gated: Box<dyn RequestFilter> = match body_req {
                                    Some(req) => Box::new(BodyGatedRequestFilter::new(request, req)),
                                    None => request,
                                };
                                entry_runtime.add_request_filter(Box::new(
                                    ConditionalRequestFilter::with_alias_and_dye(
                                        gated,
                                        entry.conditions.as_ref().map(|conditions| {
                                            super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                        }),
                                        entry_alias.clone(),
                                        dye_plan.clone(),
                                    ),
                                ));
                                // Conditions are request-owned. The companion only
                                // observes the exact attached identity installed by
                                // the matched request wrapper.
                                entry_runtime.add_upstream_response(Box::new(
                                    ConditionalUpstreamResponse::with_alias_and_dye(
                                        response,
                                        None,
                                        entry_alias,
                                        dye_plan,
                                    ),
                                ));
                            }
                            None => {
                                errors.push(format!("requestPlugins[{}] (Wasm): plugin construction failed", index));
                                entry_runtime.invalid_plugins.record(&entry.plugin);
                            }
                        }
                        break 'build_entry;
                    }

                    if let EdgionPlugin::ExtProc(_) = &entry.plugin {
                        let identity = format!(
                            "{resource_key}/requestPlugins/{}",
                            entry_alias
                                .as_deref()
                                .map_or_else(|| format!("#{index}"), str::to_string)
                        );
                        match descriptor_entries
                            .get(index)
                            .zip(ext_proc_defaults.as_deref())
                            .and_then(|(entry, defaults)| ExtProc::create_attached(entry, defaults, identity))
                        {
                            Some((request, response)) => {
                                let gated: Box<dyn RequestFilter> = match body_req {
                                    Some(req) => Box::new(BodyGatedRequestFilter::new(request, req)),
                                    None => request,
                                };
                                entry_runtime.add_request_filter(Box::new(
                                    ConditionalRequestFilter::with_alias_and_dye(
                                        gated,
                                        entry.conditions.as_ref().map(|conditions| {
                                            super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                        }),
                                        entry_alias.clone(),
                                        dye_plan.clone(),
                                    ),
                                ));
                                // Conditions are request-owned. The companion only
                                // observes the exact attached identity installed by
                                // the matched request wrapper.
                                entry_runtime.add_upstream_response(Box::new(
                                    ConditionalUpstreamResponse::with_alias_and_dye(
                                        response,
                                        None,
                                        entry_alias,
                                        dye_plan,
                                    ),
                                ));
                            }
                            None => {
                                errors.push(format!(
                                    "requestPlugins[{}] (ExtProc): plugin construction failed",
                                    index
                                ));
                                entry_runtime.invalid_plugins.record(&entry.plugin);
                            }
                        }
                        break 'build_entry;
                    }

                    if matches!(entry.plugin, EdgionPlugin::Guardrail(_)) {
                        let identity = format!(
                            "{resource_key}/requestPlugins/{}",
                            entry_alias
                                .as_deref()
                                .map_or_else(|| format!("#{index}"), str::to_string)
                        );
                        match managed_input
                            .as_ref()
                            .and_then(|input| Guardrail::create_attached(input, identity))
                        {
                            Some((request, response)) => {
                                entry_runtime.has_ai_conversion_owner = entry.plugin.is_ai_conversion_owner();
                                entry_runtime.add_request_filter(Box::new(
                                    ConditionalRequestFilter::with_alias_and_dye(
                                        request,
                                        entry.conditions.as_ref().map(|conditions| {
                                            super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                        }),
                                        entry_alias.clone(),
                                        dye_plan.clone(),
                                    ),
                                ));
                                entry_runtime.add_upstream_response(Box::new(
                                    ConditionalUpstreamResponse::with_alias_and_dye(
                                        response,
                                        None,
                                        entry_alias,
                                        dye_plan,
                                    ),
                                ));
                            }
                            None => {
                                errors.push(format!(
                                    "requestPlugins[{}] (Guardrail): plugin construction failed",
                                    index
                                ));
                                entry_runtime.invalid_plugins.record(&entry.plugin);
                            }
                        }
                        break 'build_entry;
                    }

                    let created =
                        match &entry.plugin {
                            EdgionPlugin::Wasm(_) => CreateResult::Ok(
                                descriptor_entries
                                    .get(index)
                                    .zip(wasm_defaults.as_deref())
                                    .and_then(|(entry, defaults)| WasmPlugin::create_request(entry, defaults)),
                            ),
                            EdgionPlugin::OpenidConnect(_) => CreateResult::Ok(
                                descriptor_entries
                                    .get(index)
                                    .zip(oidc_defaults.as_deref())
                                    .and_then(|(entry, defaults)| OpenidConnect::try_new(entry, defaults).ok())
                                    .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>),
                            ),
                            EdgionPlugin::LdapAuth(_) => CreateResult::Ok(
                                descriptor_entries
                                    .get(index)
                                    .zip(ldap_defaults.as_deref())
                                    .and_then(|(entry, defaults)| LdapAuth::create(entry, defaults)),
                            ),
                            EdgionPlugin::HeaderCertAuth(_) => CreateResult::Ok(
                                header_ca_field
                                    .as_ref()
                                    .map(|field| Box::new(HeaderCertAuth::new(field)) as Box<dyn RequestFilter>),
                            ),
                            EdgionPlugin::ForwardAuth(_) => CreateResult::Ok(
                                managed_input
                                    .as_ref()
                                    .and_then(|input| ForwardAuth::from_owned(input).ok())
                                    .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>),
                            ),
                            EdgionPlugin::AiGuard(config) if config.evaluator.is_some() => CreateResult::Ok(
                                managed_input
                                    .as_ref()
                                    .and_then(|input| AiGuard::create_owned(input, &entry_scope)),
                            ),
                            EdgionPlugin::IpRestriction(_) => {
                                let bound = ip_binding.as_ref().and_then(|binding| binding.http_entry(index));
                                CreateResult::Ok(bound.and_then(|entry| {
                                    ip.and_then(|compiler| IpRestriction::create_bound(entry, compiler))
                                }))
                            }

                            EdgionPlugin::RequestAccessPolicy(config) => CreateResult::Ok(match data_entry {
                                Some(entry) => RequestAccessPolicy::create_bound(entry, body_req.clone(), ip),
                                None => RequestAccessPolicy::create_inline(config, body_req.clone(), ip),
                            }),
                            EdgionPlugin::Canary(config) => CreateResult::Ok(match data_entry {
                                Some(entry) => Canary::create_bound(entry),
                                None => Canary::create_inline(config),
                            }),
                            EdgionPlugin::RegionRoute(config) => CreateResult::Ok(match data_entry {
                                Some(entry) => RegionRoute::create_bound(entry),
                                None => RegionRoute::create_inline(config),
                            }),
                            EdgionPlugin::GrpcJsonTranscoder(config) => {
                                CreateResult::Ok(descriptor_entries.get(index).map_or_else(
                                    || {
                                        GrpcJsonTranscoder::create_inline(
                                            config,
                                            body_req.clone().unwrap_or_else(BodyRequirement::fail_close),
                                        )
                                    },
                                    |entry| {
                                        GrpcJsonTranscoder::create(
                                            entry,
                                            body_req.clone().unwrap_or_else(BodyRequirement::fail_close),
                                        )
                                    },
                                ))
                            }
                            EdgionPlugin::RequestMirror(config) => CreateResult::Ok(
                                mirror_field
                                    .and_then(|field| RequestMirrorPlugin::new(field, config.tuning.clone()))
                                    .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>),
                            ),
                            EdgionPlugin::RateLimitLocal(config) => {
                                CreateResult::Ok(Some(RateLimitLocal::create_with_outcome_slot(
                                    config,
                                    source_aliases[index]
                                        .as_deref()
                                        .and_then(|alias| outcome_aliases.get(alias).copied()),
                                )))
                            }
                            _ if link_sys_config.is_some() => Self::create_link_sys_request_filter(
                                link_sys_config.expect("owning config checked above"),
                                namespace,
                                body_req.clone(),
                                &entry_scope,
                            ),
                            _ => Self::create_request_filter_from_edgion(
                                &entry.plugin,
                                namespace,
                                body_req.clone(),
                                &entry_scope,
                                ip,
                            ),
                        };
                    match created {
                        CreateResult::Ok(Some(filter)) => {
                            // Condition-owned body HMAC on RequestRestriction and
                            // RequestAccessPolicy uses the generic plugin-level gate,
                            // so capture happens before Boolean branch evaluation.
                            // The dedicated HmacAuth plugin remains the sole exception:
                            // it performs cheap header authentication before capture.
                            let generic_body_gate = Self::use_generic_body_gate(&entry.plugin);
                            let gated: Box<dyn RequestFilter> = match body_req {
                                Some(req) if generic_body_gate => {
                                    Box::new(BodyGatedRequestFilter::with_conditions(
                                        filter,
                                        req,
                                        entry.body.as_ref().and_then(|body| body.conditions.as_ref()).map(
                                            |conditions| {
                                                super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                            },
                                        ),
                                    ))
                                }
                                None => filter,
                                Some(_) => filter,
                            };
                            // Wrap with ConditionalRequestFilter to support condition evaluation
                            let mut conditional_filter = ConditionalRequestFilter::with_alias_and_dye(
                                gated,
                                entry.conditions.as_ref().map(|conditions| {
                                    super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                }),
                                entry_alias,
                                dye_plan,
                            );
                            conditional_filter.bind_rate_limit_dependencies(&outcome_aliases);
                            entry_runtime.add_request_filter(Box::new(conditional_filter));
                            entry_runtime.has_ai_conversion_owner = entry.plugin.is_ai_conversion_owner();
                        }
                        CreateResult::Ok(None) => {
                            // Applicable stage but the constructor declined to build the
                            // plugin (e.g. DSL VM init failure). `Ok(None)` means
                            // "should have run but did not" — always fail-closed.
                            // Record it so `seal_fail_closed` fires.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "requestPlugins[{}] ({}): plugin construction failed",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                        CreateResult::WrongStage => {
                            // A plugin placed in the wrong stage array is a misconfiguration:
                            // fail closed alongside validation_error failures rather than
                            // silently no-op'ing.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "requestPlugins[{}] ({}): plugin is not valid for request stage",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                    }
                }
                entry_runtime.cache_capabilities =
                    super::entry_bundle::CacheCapabilities::for_constructed(&entry.plugin, slot.stage);
                let outcome = entry_runtime.into_entry_outcome(errors[errors_before..].to_vec());
                self.append_entry_bundle(outcome.bundle());
                cache.insert(slot, outcome);
            }
        }

        errors
    }

    /// Add upstream response filters from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalUpstreamResponseFilter to support condition-based execution.
    ///
    /// # Returns
    /// A vector of validation error messages (empty if all plugins are valid)
    pub fn add_from_upstream_response_filters(
        &mut self,
        entries: &[UpstreamResponseFilterEntry],
        namespace: &str,
    ) -> Vec<String> {
        self.add_response_filter_entries(entries, namespace, &mut EntryCache::default(), None, None)
    }

    fn add_response_filter_entries(
        &mut self,
        entries: &[UpstreamResponseFilterEntry],
        namespace: &str,
        cache: &mut EntryCache,
        bindings: Option<&PluginBindings<'_>>,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        let bound_entries: Vec<_> = bindings
            .into_iter()
            .flat_map(|binding| binding.response_filter_entries())
            .collect();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                let wasm_defaults = matches!(entry.plugin, EdgionPlugin::Wasm(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = wasm_defaults.as_deref() {
                    if bound_entries
                        .get(index)
                        .ok_or_else(|| "wasm TLS configuration is invalid".to_string())
                        .and_then(|entry| WasmPlugin::validate_tls_entry(entry, defaults))
                        .is_err()
                    {
                        self.invalid_plugins.record_non_retainable("Wasm");
                        errors.push(format!(
                            "upstreamResponseFilterPlugins[{index}] (Wasm): {}",
                            Self::response_plugin_validation_error(&entry.plugin)
                                .as_deref()
                                .unwrap_or("plugin construction failed")
                        ));
                        continue;
                    }
                }
                let slot = EntrySlot {
                    stage: EntryStage::ResponseFilter,
                    index,
                };
                if let Some(outcome) = cache.get(slot) {
                    self.append_entry_bundle(outcome.bundle());
                    errors.extend(outcome.bundle().errors.iter().cloned());
                    continue;
                }
                let mut entry_runtime = Self::new();
                let errors_before = errors.len();
                'build_entry: {
                    // Reject plugins whose config fails validation: record the name,
                    // surface the error, and skip construction so an invalid config
                    // can never run in the request path. `seal_fail_closed` later
                    // turns a non-empty `invalid_plugins` into a fail-closed 500.
                    if let Some(error) = Self::response_plugin_validation_error(&entry.plugin) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseFilterPlugins[{}] ({}): {}",
                            index, plugin_name, error
                        ));
                        entry_runtime.invalid_plugins.record(&entry.plugin);
                        break 'build_entry;
                    }

                    // Validate optional entry-level alias. On failure, surface
                    // the error and drop the alias so an unvalidated value never
                    // reaches the access log.
                    let mut entry_alias = match entry.validate_alias() {
                        None => entry.alias.clone(),
                        Some(err) => {
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponseFilterPlugins[{}] ({}): {}",
                                index, plugin_name, err
                            ));
                            None
                        }
                    };

                    // Alias uniqueness within this stage array: the alias is the
                    // discriminator in the entry identity that namespaces state kept
                    // outside the plugin instance (RateLimitRedis buckets), so a
                    // duplicate must not survive into the runtime.
                    if let Some(err) = edgion_resources::resources::edgion_plugins::duplicate_alias_error(
                        &mut seen_aliases,
                        entry_alias.as_deref(),
                    ) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseFilterPlugins[{}] ({}): {}",
                            index, plugin_name, err
                        ));
                        // Drop it exactly like an invalid alias, so the entry falls back
                        // to the `#index` form of its scope. Reporting alone would not be
                        // enough: lenient admission keeps an `Accepted=False` resource
                        // synced and serving, and the two entries would still share one
                        // bucket namespace and one access-log label.
                        entry_alias = None;
                    }

                    // Validate the optional entry-level dye suite. Same severity as
                    // the alias check above: non-fatal, surfaced as a preparse error.
                    // Only compile a runtime plan when the schema-level check passed
                    // (see `compile_entry_dye_plan` doc comment for the severity split).
                    let dye_validation_err = entry.validate_dye();
                    if let Some(err) = &dye_validation_err {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseFilterPlugins[{}] ({}): {}",
                            index, plugin_name, err
                        ));
                    }
                    let dye_plan = if dye_validation_err.is_none() {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        Self::compile_entry_dye_plan(
                            entry.dye.as_ref(),
                            &entry.plugin,
                            &format!("upstreamResponseFilterPlugins[{}] ({})", index, plugin_name),
                            &mut errors,
                            &mut entry_runtime.invalid_plugins,
                            ip,
                        )
                    } else {
                        None
                    };

                    let created = if matches!(entry.plugin, EdgionPlugin::Wasm(_)) {
                        CreateResult::Ok(
                            bound_entries
                                .get(index)
                                .zip(wasm_defaults.as_deref())
                                .and_then(|(entry, defaults)| WasmPlugin::create_response_filter(entry, defaults)),
                        )
                    } else {
                        Self::create_upstream_response_filter_from_edgion(&entry.plugin, namespace)
                    };
                    match created {
                        CreateResult::Ok(Some(filter)) => {
                            // Wrap with ConditionalUpstreamResponseFilter to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponseFilter::with_alias_and_dye(
                                filter,
                                entry.conditions.as_ref().map(|conditions| {
                                    super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                }),
                                entry_alias,
                                dye_plan,
                            );
                            entry_runtime.add_upstream_response_filter(Box::new(conditional_filter));
                        }
                        CreateResult::Ok(None) => {
                            // Applicable stage but the constructor declined to build the
                            // plugin (e.g. DSL VM init failure). `Ok(None)` means
                            // "should have run but did not" — always fail-closed.
                            // Record it so `seal_fail_closed` fires.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponseFilterPlugins[{}] ({}): plugin construction failed",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                        CreateResult::WrongStage => {
                            // A plugin placed in the wrong stage array is a misconfiguration:
                            // fail closed alongside validation_error failures rather than
                            // silently no-op'ing.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                            "upstreamResponseFilterPlugins[{}] ({}): plugin is not valid for upstream response filter stage",
                            index, plugin_name
                        ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                    }
                }
                entry_runtime.cache_capabilities =
                    super::entry_bundle::CacheCapabilities::for_constructed(&entry.plugin, slot.stage);
                let outcome = entry_runtime.into_entry_outcome(errors[errors_before..].to_vec());
                self.append_entry_bundle(outcome.bundle());
                cache.insert(slot, outcome);
            }
        }

        errors
    }

    /// Add upstream response handlers from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalUpstreamResponse to support condition-based execution.
    ///
    /// # Returns
    /// A vector of validation error messages (empty if all plugins are valid)
    pub fn add_from_upstream_responses(&mut self, entries: &[UpstreamResponseEntry], namespace: &str) -> Vec<String> {
        self.add_from_upstream_responses_with_cache(entries, namespace, &mut EntryCache::default(), None, None)
    }

    pub(crate) fn add_from_upstream_responses_with_cache(
        &mut self,
        entries: &[UpstreamResponseEntry],
        namespace: &str,
        cache: &mut EntryCache,
        bindings: Option<&PluginBindings<'_>>,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        let bound_entries: Vec<_> = bindings
            .into_iter()
            .flat_map(|binding| binding.response_entries())
            .collect();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                // A DSL script with LinkSys references is built only from its
                // validated owning binding, never from the raw entry.
                let link_sys_config = if matches!(&entry.plugin, EdgionPlugin::Dsl(config) if !config.linksys_refs.is_empty())
                {
                    match bound_entries
                        .get(index)
                        .and_then(|entry| entry.link_sys_config().ok())
                        .flatten()
                    {
                        Some(HttpLinkSysConfig::Dsl(config)) => Some(config),
                        _ => {
                            self.invalid_plugins.record_non_retainable("Dsl");
                            errors.push(format!(
                                "upstreamResponsePlugins[{index}] (Dsl): invalid owning LinkSys reference"
                            ));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let slot = EntrySlot {
                    stage: EntryStage::Response,
                    index,
                };
                if let Some(outcome) = cache.get(slot) {
                    self.append_entry_bundle(outcome.bundle());
                    errors.extend(outcome.bundle().errors.iter().cloned());
                    continue;
                }
                let mut entry_runtime = Self::new();
                let errors_before = errors.len();
                'build_entry: {
                    // Reject plugins whose config fails validation: record the name,
                    // surface the error, and skip construction so an invalid config
                    // can never run in the request path. `seal_fail_closed` later
                    // turns a non-empty `invalid_plugins` into a fail-closed 500.
                    if let Some(error) = Self::response_plugin_validation_error(&entry.plugin) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponsePlugins[{}] ({}): {}",
                            index, plugin_name, error
                        ));
                        entry_runtime.invalid_plugins.record(&entry.plugin);
                        break 'build_entry;
                    }

                    // Validate the optional operator body requirement. `body:` only
                    // applies to plugins that consume the body in their current config;
                    // reject it elsewhere so it is never a silent no-op. Non-fatal.
                    if entry.body.is_some()
                        && !entry
                            .plugin
                            .accepts_body(edgion_resources::resources::edgion_plugins::BodyStage::Response)
                    {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponsePlugins[{}] ({}): body: plugin does not consume a body",
                            index, plugin_name
                        ));
                    } else if let Some(err) = entry.body.as_ref().and_then(|b| b.validation_error()) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponsePlugins[{}] ({}): body: {}",
                            index, plugin_name, err
                        ));
                    }

                    let mut entry_alias = match entry.validate_alias() {
                        None => entry.alias.clone(),
                        Some(err) => {
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!("upstreamResponsePlugins[{}] ({}): {}", index, plugin_name, err));
                            None
                        }
                    };

                    // Alias uniqueness within this stage array: the alias is the
                    // discriminator in the entry identity that namespaces state kept
                    // outside the plugin instance (RateLimitRedis buckets), so a
                    // duplicate must not survive into the runtime.
                    if let Some(err) = edgion_resources::resources::edgion_plugins::duplicate_alias_error(
                        &mut seen_aliases,
                        entry_alias.as_deref(),
                    ) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("upstreamResponsePlugins[{}] ({}): {}", index, plugin_name, err));
                        // Drop it exactly like an invalid alias, so the entry falls back
                        // to the `#index` form of its scope. Reporting alone would not be
                        // enough: lenient admission keeps an `Accepted=False` resource
                        // synced and serving, and the two entries would still share one
                        // bucket namespace and one access-log label.
                        entry_alias = None;
                    }

                    // Validate the optional entry-level dye suite. Same severity as
                    // the alias check above: non-fatal, surfaced as a preparse error.
                    // Only compile a runtime plan when the schema-level check passed
                    // (see `compile_entry_dye_plan` doc comment for the severity split).
                    let dye_validation_err = entry.validate_dye();
                    if let Some(err) = &dye_validation_err {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("upstreamResponsePlugins[{}] ({}): {}", index, plugin_name, err));
                    }
                    let dye_plan = if dye_validation_err.is_none() {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        Self::compile_entry_dye_plan(
                            entry.dye.as_ref(),
                            &entry.plugin,
                            &format!("upstreamResponsePlugins[{}] ({})", index, plugin_name),
                            &mut errors,
                            &mut entry_runtime.invalid_plugins,
                            ip,
                        )
                    } else {
                        None
                    };

                    let created = match link_sys_config {
                        Some(config) => CreateResult::Ok(DslPlugin::from_config_response(config)),
                        None => Self::create_upstream_response_from_edgion(&entry.plugin, namespace),
                    };
                    match created {
                        CreateResult::Ok(Some(filter)) => {
                            // Resolve the effective body requirement (operator tunes
                            // size/conditions but cannot downgrade an intrinsic
                            // fail-close) and wrap in a response body gate inside the
                            // Conditional wrapper.
                            let body_req = Self::resolve_body_requirement(
                                &entry.plugin,
                                entry.body.clone(),
                                edgion_resources::resources::edgion_plugins::BodyStage::Response,
                            );
                            let gated: Box<dyn UpstreamResponse> =
                                match body_req {
                                    Some(req) => Box::new(BodyGatedUpstreamResponse::with_conditions(
                                        filter,
                                        req,
                                        entry.body.as_ref().and_then(|body| body.conditions.as_ref()).map(
                                            |conditions| {
                                                super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                            },
                                        ),
                                    )),
                                    None => filter,
                                };
                            // Wrap with ConditionalUpstreamResponse to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponse::with_alias_and_dye(
                                gated,
                                entry.conditions.as_ref().map(|conditions| {
                                    super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                }),
                                entry_alias,
                                dye_plan,
                            );
                            entry_runtime.add_upstream_response(Box::new(conditional_filter));
                        }
                        CreateResult::Ok(None) => {
                            // Applicable stage but the constructor declined to build the
                            // plugin (e.g. DSL VM init failure). `Ok(None)` means
                            // "should have run but did not" — always fail-closed.
                            // Record it so `seal_fail_closed` fires.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponsePlugins[{}] ({}): plugin construction failed",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                        CreateResult::WrongStage => {
                            // A plugin placed in the wrong stage array is a misconfiguration:
                            // fail closed alongside validation_error failures rather than
                            // silently no-op'ing.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponsePlugins[{}] ({}): plugin is not valid for upstream response stage",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                    }
                }
                entry_runtime.cache_capabilities =
                    super::entry_bundle::CacheCapabilities::for_constructed(&entry.plugin, slot.stage);
                let outcome = entry_runtime.into_entry_outcome(errors[errors_before..].to_vec());
                self.append_entry_bundle(outcome.bundle());
                cache.insert(slot, outcome);
            }
        }

        errors
    }

    /// Add upstream response body filters from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalUpstreamResponseBodyFilter to support condition-based execution.
    ///
    /// # Returns
    /// A vector of validation error messages (empty if all plugins are valid)
    pub fn add_from_upstream_response_body_filters(
        &mut self,
        entries: &[UpstreamResponseBodyFilterEntry],
        namespace: &str,
    ) -> Vec<String> {
        self.add_from_upstream_response_body_filters_with_cache(
            entries,
            namespace,
            &mut EntryCache::default(),
            None,
            None,
        )
    }

    pub(crate) fn add_from_upstream_response_body_filters_with_cache(
        &mut self,
        entries: &[UpstreamResponseBodyFilterEntry],
        namespace: &str,
        cache: &mut EntryCache,
        bindings: Option<&PluginBindings<'_>>,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        let bound_entries: Vec<_> = bindings
            .into_iter()
            .flat_map(|binding| binding.response_body_filter_entries())
            .collect();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                let wasm_defaults = matches!(entry.plugin, EdgionPlugin::Wasm(_))
                    .then(edgion_common::link_sys_core::outbound_tls::outbound_global_default);
                if let Some(defaults) = wasm_defaults.as_deref() {
                    if bound_entries
                        .get(index)
                        .ok_or_else(|| "wasm TLS configuration is invalid".to_string())
                        .and_then(|entry| WasmPlugin::validate_tls_entry(entry, defaults))
                        .is_err()
                    {
                        self.invalid_plugins.record_non_retainable("Wasm");
                        errors.push(format!(
                            "upstreamResponseBodyFilterPlugins[{index}] (Wasm): {}",
                            Self::response_plugin_validation_error(&entry.plugin)
                                .as_deref()
                                .unwrap_or("plugin construction failed")
                        ));
                        continue;
                    }
                }
                let slot = EntrySlot {
                    stage: EntryStage::ResponseBody,
                    index,
                };
                if let Some(outcome) = cache.get(slot) {
                    self.append_entry_bundle(outcome.bundle());
                    errors.extend(outcome.bundle().errors.iter().cloned());
                    continue;
                }
                let mut entry_runtime = Self::new();
                let errors_before = errors.len();
                'build_entry: {
                    // Reject plugins whose config fails validation: record the name,
                    // surface the error, and skip construction so an invalid config
                    // can never run in the request path. `seal_fail_closed` later
                    // turns a non-empty `invalid_plugins` into a fail-closed 500.
                    if let Some(error) = Self::response_plugin_validation_error(&entry.plugin) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseBodyFilterPlugins[{}] ({}): {}",
                            index, plugin_name, error
                        ));
                        entry_runtime.invalid_plugins.record(&entry.plugin);
                        break 'build_entry;
                    }

                    // Validate optional entry-level alias. On failure, surface
                    // the error and drop the alias so an unvalidated value never
                    // reaches the access log.
                    let mut entry_alias = match entry.validate_alias() {
                        None => entry.alias.clone(),
                        Some(err) => {
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponseBodyFilterPlugins[{}] ({}): {}",
                                index, plugin_name, err
                            ));
                            None
                        }
                    };

                    // Alias uniqueness within this stage array: the alias is the
                    // discriminator in the entry identity that namespaces state kept
                    // outside the plugin instance (RateLimitRedis buckets), so a
                    // duplicate must not survive into the runtime.
                    if let Some(err) = edgion_resources::resources::edgion_plugins::duplicate_alias_error(
                        &mut seen_aliases,
                        entry_alias.as_deref(),
                    ) {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseBodyFilterPlugins[{}] ({}): {}",
                            index, plugin_name, err
                        ));
                        // Drop it exactly like an invalid alias, so the entry falls back
                        // to the `#index` form of its scope. Reporting alone would not be
                        // enough: lenient admission keeps an `Accepted=False` resource
                        // synced and serving, and the two entries would still share one
                        // bucket namespace and one access-log label.
                        entry_alias = None;
                    }

                    // Validate the optional entry-level dye suite. Same severity as
                    // the alias check above: non-fatal, surfaced as a preparse error.
                    // Only compile a runtime plan when the schema-level check passed
                    // (see `compile_entry_dye_plan` doc comment for the severity split).
                    let dye_validation_err = entry.validate_dye();
                    if let Some(err) = &dye_validation_err {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!(
                            "upstreamResponseBodyFilterPlugins[{}] ({}): {}",
                            index, plugin_name, err
                        ));
                    }
                    let dye_plan = if dye_validation_err.is_none() {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        Self::compile_entry_dye_plan(
                            entry.dye.as_ref(),
                            &entry.plugin,
                            &format!("upstreamResponseBodyFilterPlugins[{}] ({})", index, plugin_name),
                            &mut errors,
                            &mut entry_runtime.invalid_plugins,
                            ip,
                        )
                    } else {
                        None
                    };

                    let created = if matches!(entry.plugin, EdgionPlugin::Wasm(_)) {
                        CreateResult::Ok(
                            bound_entries
                                .get(index)
                                .zip(wasm_defaults.as_deref())
                                .and_then(|(entry, defaults)| WasmPlugin::create_body_filter(entry, defaults))
                                .map(|filter| (filter, true)),
                        )
                    } else {
                        Self::create_upstream_response_body_filter_from_edgion(&entry.plugin, namespace)
                    };
                    match created {
                        CreateResult::Ok(Some((filter, has_unary_body_consumer))) => {
                            // Wrap with ConditionalUpstreamResponseBodyFilter to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponseBodyFilter::with_alias_and_dye(
                                filter,
                                entry.conditions.as_ref().map(|conditions| {
                                    super::conditions::CompiledPluginConditions::with_ip(conditions, ip)
                                }),
                                entry_alias,
                                dye_plan,
                            );
                            entry_runtime.add_upstream_response_body_filter(Box::new(conditional_filter));
                            entry_runtime.has_unary_body_consumer = has_unary_body_consumer;
                        }
                        CreateResult::Ok(None) => {
                            // Applicable stage but the constructor declined to build the
                            // plugin (e.g. DSL VM init failure). `Ok(None)` means
                            // "should have run but did not" — always fail-closed.
                            // Record it so `seal_fail_closed` fires.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                                "upstreamResponseBodyFilterPlugins[{}] ({}): plugin construction failed",
                                index, plugin_name
                            ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                        CreateResult::WrongStage => {
                            // A plugin placed in the wrong stage array is a misconfiguration:
                            // fail closed alongside validation_error failures rather than
                            // silently no-op'ing.
                            let plugin_name = Self::get_plugin_name(&entry.plugin);
                            errors.push(format!(
                            "upstreamResponseBodyFilterPlugins[{}] ({}): plugin is not valid for upstream response body filter stage",
                            index, plugin_name
                        ));
                            entry_runtime.invalid_plugins.record(&entry.plugin);
                        }
                    }
                }
                entry_runtime.cache_capabilities =
                    super::entry_bundle::CacheCapabilities::for_constructed(&entry.plugin, slot.stage);
                let outcome = entry_runtime.into_entry_outcome(errors[errors_before..].to_vec());
                self.append_entry_bundle(outcome.bundle());
                cache.insert(slot, outcome);
            }
        }

        errors
    }

    /// Gateway-side plugin validation gate.
    ///
    /// Runs the schema-layer checks (`EdgionPlugin::validation_error`) — which
    /// since the `edgion-dsl` extraction include the deep DSL static pipeline
    /// (compile / bytecode validation / linkSysRefs / redis link-name scan),
    /// so control plane and data plane literally share one code path — plus
    /// gateway-only checks that need resolved runtime state (e.g. the CSRF
    /// resolved-key guard below). DSL VM init remains covered by
    /// `seal_fail_closed` at construction time.
    fn response_plugin_validation_error(plugin: &EdgionPlugin) -> Option<String> {
        if matches!(
            plugin,
            EdgionPlugin::GrpcJsonTranscoder(_) | EdgionPlugin::RegionRoute(_)
        ) {
            return edgion_resources::resources::edgion_plugins::validate::response_stage_plugin_validation_error(
                plugin,
            );
        }
        Self::plugin_validation_error(plugin)
    }

    fn plugin_validation_error(plugin: &EdgionPlugin) -> Option<String> {
        if let EdgionPlugin::RegionRoute(config) = plugin {
            return config.structural_validation_error();
        }
        if let Some(e) = plugin.validation_error() {
            return Some(e);
        }
        if let EdgionPlugin::Csrf(cfg) = plugin {
            // A CSRF plugin cannot sign/verify without a key. If the secretRef
            // never resolved, refuse to bring it live (seal_fail_closed -> 500)
            // rather than mint forgeable tokens with an empty key.
            if cfg.resolved_key.as_deref().unwrap_or("").is_empty() {
                return Some("CSRF: signing key not resolved (secretRef missing or unresolved Secret)".to_string());
            }
        }
        None
    }

    /// Resolve the effective body requirement for a plugin entry, shared by
    /// the request and response wiring.
    ///
    /// For a plugin with an intrinsic declaration (`plugin.body_requirement()`
    /// is `Some`), this defers to [`BodyRequirement::resolve`] unchanged: the
    /// operator may tune `maxBodySize`/`conditions`, but a stricter intrinsic
    /// fail-close always wins.
    ///
    /// `Wasm` is the one plugin that `accepts_body()` without an intrinsic
    /// declaration (its guest hooks are only known after module load, so
    /// nothing in its own config can express "needs body"). For such a
    /// plugin, `BodyRequirement::resolve` would always return `None` — its
    /// `intr = intrinsic?` short-circuit requires an intrinsic value to
    /// exist, so passing `None` as intrinsic can never produce a gate no
    /// matter what the operator sets. Since the operator's `body:` is the
    /// SOLE source of truth here (there is nothing to "tune" against), it is
    /// used as-is: no forced fail-close floor, no dropped conditions — those
    /// merge rules exist to protect an intrinsic declaration this plugin does
    /// not have.
    fn resolve_body_requirement(
        plugin: &EdgionPlugin,
        operator: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
        stage: edgion_resources::resources::edgion_plugins::BodyStage,
    ) -> Option<edgion_resources::resources::edgion_plugins::BodyRequirement> {
        match plugin.body_requirement(stage) {
            Some(intrinsic) => {
                edgion_resources::resources::edgion_plugins::BodyRequirement::resolve(operator, Some(intrinsic))
            }
            None if plugin.accepts_body(stage) => operator,
            None => None,
        }
    }

    fn use_generic_body_gate(plugin: &EdgionPlugin) -> bool {
        match plugin {
            EdgionPlugin::HmacAuth(_) | EdgionPlugin::GrpcJsonTranscoder(_) => false,
            EdgionPlugin::RequestRestriction(config) => !RequestRestriction::would_fail_construction(config),
            _ => true,
        }
    }

    /// Create a RequestFilter instance from EdgionPlugin enum
    ///
    /// # Arguments
    /// * `plugin` - The EdgionPlugin configuration
    /// * `namespace` - The namespace of the EdgionPlugins resource
    /// * `entry_scope` - `<namespace>/<name>/<alias or #index>` identity of this
    ///   entry, used by plugins that must namespace state kept outside the
    ///   process (RateLimitRedis Redis keys)
    fn create_link_sys_request_filter(
        config: HttpLinkSysConfig<'_>,
        namespace: &str,
        body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
        entry_scope: &str,
    ) -> CreateResult<Box<dyn RequestFilter>> {
        match config {
            HttpLinkSysConfig::Dsl(config) => CreateResult::Ok(DslPlugin::from_config(config)),
            HttpLinkSysConfig::RateLimitRedis(config) => {
                CreateResult::Ok(Some(RateLimitRedis::create(config, entry_scope)))
            }
            HttpLinkSysConfig::GraphqlRateLimit(config) => {
                CreateResult::Ok(GraphqlRateLimit::create(config, entry_scope, body))
            }
            HttpLinkSysConfig::WebhookKeyGet(config) => CreateResult::Ok(Some(WebhookKeyGet::create(config))),
            HttpLinkSysConfig::AiProxy(config) => CreateResult::Ok(AiProxy::create(config, namespace, entry_scope)),
        }
    }

    fn create_request_filter_from_edgion(
        plugin: &EdgionPlugin,
        namespace: &str,
        body_requirement: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
        entry_scope: &str,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> CreateResult<Box<dyn RequestFilter>> {
        if HttpLinkSysConfig::is_reference_bearing(plugin) {
            return CreateResult::WrongStage;
        }
        match plugin {
            EdgionPlugin::Unsupported => CreateResult::WrongStage,
            EdgionPlugin::RequestHeaderModifier(config) => CreateResult::Ok(
                RequestHeaderModifierFilter::new(config.clone())
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn RequestFilter>),
            ),
            EdgionPlugin::RequestRedirect(config) => CreateResult::Ok(
                RequestRedirectFilter::try_new(config)
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn RequestFilter>),
            ),
            EdgionPlugin::UrlRewrite(config) => CreateResult::Ok(
                URLRewriteFilter::try_new(config)
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn RequestFilter>),
            ),
            EdgionPlugin::RequestMirror(_) => CreateResult::WrongStage,
            EdgionPlugin::BasicAuth(config) => CreateResult::Ok(Some(Box::new(BasicAuth::new(config)))),
            EdgionPlugin::AiProxy(config) => CreateResult::Ok(AiProxy::create(config, namespace, entry_scope)),
            EdgionPlugin::AiGuard(config) => CreateResult::Ok(AiGuard::create(config, entry_scope)),
            EdgionPlugin::Cors(config) => CreateResult::Ok(Some(Box::new(Cors::new(config)))),
            EdgionPlugin::Csrf(config) => CreateResult::Ok(Some(Box::new(Csrf::new(config)))),
            EdgionPlugin::CredentialInjector(config) => CreateResult::Ok(CredentialInjector::create(config)),
            EdgionPlugin::IpRestriction(_) => CreateResult::WrongStage,
            EdgionPlugin::RequestAccessPolicy(config) => {
                CreateResult::Ok(RequestAccessPolicy::create_inline(config, body_requirement, ip))
            }
            EdgionPlugin::GeoIpLocation(config) => {
                CreateResult::Ok(ip.and_then(|compiler| GeoIpLocation::create(config, compiler).ok()))
            }
            EdgionPlugin::Canary(config) => CreateResult::Ok(Canary::create_inline(config)),
            EdgionPlugin::JwtAuth(config) => {
                CreateResult::Ok(Some(Box::new(JwtAuth::new(config, namespace.to_string()))))
            }
            EdgionPlugin::JweDecrypt(config) => {
                CreateResult::Ok(Some(Box::new(JweDecrypt::new(config, namespace.to_string()))))
            }
            EdgionPlugin::JsonSchemaValidation(config) => CreateResult::Ok(JsonSchemaValidation::create(config)),
            EdgionPlugin::GraphqlOperationRouter(config) => {
                CreateResult::Ok(GraphqlOperationRouter::create(config, body_requirement))
            }
            EdgionPlugin::RestToGraphql(config) => CreateResult::Ok(RestToGraphql::create(config, body_requirement)),
            EdgionPlugin::GraphqlGuard(config) => CreateResult::Ok(GraphqlGuard::create(config, body_requirement)),
            EdgionPlugin::GraphqlSafelist(config) => {
                CreateResult::Ok(GraphqlSafelist::create(config, body_requirement))
            }
            EdgionPlugin::GraphqlRateLimit(_) => CreateResult::WrongStage,
            EdgionPlugin::GrpcJsonTranscoder(config) => CreateResult::Ok(GrpcJsonTranscoder::create_inline(
                config,
                body_requirement.unwrap_or_else(BodyRequirement::fail_close),
            )),
            EdgionPlugin::FormJsonTransform(config) => {
                CreateResult::Ok(FormJsonTransform::create(config, body_requirement))
            }
            EdgionPlugin::HmacAuth(config) => CreateResult::Ok(Some(HmacAuth::create(config, body_requirement))),
            EdgionPlugin::HeaderCertAuth(_) => CreateResult::WrongStage,
            EdgionPlugin::KeyAuth(config) => CreateResult::Ok(Some(KeyAuth::create(config))),
            EdgionPlugin::LdapAuth(_) => CreateResult::WrongStage,
            EdgionPlugin::FaultInjection(config) => CreateResult::Ok(Some(Box::new(FaultInjection::new(config)))),
            EdgionPlugin::Mock(config) => CreateResult::Ok(Some(Box::new(Mock::new(config)))),
            EdgionPlugin::ProxyRewrite(config) => {
                CreateResult::Ok(ProxyRewrite::create(config, body_requirement).map(|plugin| Box::new(plugin) as _))
            }
            EdgionPlugin::RequestBodyBuffer(_) => CreateResult::Ok(Some(RequestBodyBuffer::create())),
            EdgionPlugin::ResponseCache(config) => {
                CreateResult::Ok(crate::plugins::http::response_cache::ResponseCache::create(config))
            }
            EdgionPlugin::RequestRestriction(config) => {
                CreateResult::Ok(Some(RequestRestriction::with_ip(config, body_requirement, ip)))
            }
            EdgionPlugin::RateLimitLocal(config) => CreateResult::Ok(Some(RateLimitLocal::create(config))),
            EdgionPlugin::RateLimitRedis(_) => CreateResult::WrongStage,
            EdgionPlugin::CtxSet(config) => CreateResult::Ok(Some(CtxSet::create(config))),
            EdgionPlugin::WebhookKeyGet(config) => CreateResult::Ok(Some(WebhookKeyGet::create(config))),
            EdgionPlugin::DirectEndpoint(config) => CreateResult::Ok(Some(Box::new(DirectEndpoint::new(config)))),
            EdgionPlugin::DynamicInternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicInternalUpstream::create(config)))
            }
            EdgionPlugin::DynamicExternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicExternalUpstream::create(config)))
            }
            EdgionPlugin::RealIp(config) => {
                CreateResult::Ok(ip.and_then(|compiler| RealIp::create(config, compiler).ok()))
            }
            EdgionPlugin::RequestId(config) => CreateResult::Ok(RequestId::create(config)),
            EdgionPlugin::ExtProc(_) => CreateResult::WrongStage,
            EdgionPlugin::Guardrail(_) => CreateResult::WrongStage,
            EdgionPlugin::ForwardAuth(_) => CreateResult::WrongStage,
            EdgionPlugin::AllEndpointStatus(config) => CreateResult::Ok(
                AllEndpointStatus::new(config)
                    .ok()
                    .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>),
            ),
            EdgionPlugin::OpenidConnect(_) => CreateResult::WrongStage,
            EdgionPlugin::Dsl(config) => CreateResult::Ok(DslPlugin::from_config(config)),
            EdgionPlugin::RegionRoute(config) => CreateResult::Ok(RegionRoute::create_inline(config)),
            EdgionPlugin::Wasm(_) => CreateResult::WrongStage,
            // Response-stage only — not applicable to request filters
            EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::BandwidthLimit(_) => CreateResult::WrongStage,
        }
    }

    /// Create an UpstreamResponseFilter instance from EdgionPlugin enum
    fn create_upstream_response_filter_from_edgion(
        plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> CreateResult<Box<dyn UpstreamResponseFilter>> {
        if HttpLinkSysConfig::is_reference_bearing(plugin) {
            return CreateResult::WrongStage;
        }
        match plugin {
            EdgionPlugin::Unsupported => CreateResult::WrongStage,
            EdgionPlugin::ResponseHeaderModifier(config) => CreateResult::Ok(
                ResponseHeaderModifierFilter::new(config.clone())
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn UpstreamResponseFilter>),
            ),
            EdgionPlugin::DebugAccessLogToHeader(config) => {
                CreateResult::Ok(Some(Box::new(DebugAccessLogToHeaderFilter::new(config))))
            }
            EdgionPlugin::ResponseRewrite(config) => CreateResult::Ok(Some(Box::new(ResponseRewrite::new(config)))),
            // DSL awaits its VM, so it belongs to the async upstream-response stage.
            EdgionPlugin::Dsl(_) | EdgionPlugin::Wasm(_) => CreateResult::WrongStage,
            // Request-stage or other-stage plugins — not applicable to upstream response filters
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::CredentialInjector(_)
            | EdgionPlugin::IpRestriction(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::JweDecrypt(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::RestToGraphql(_)
            | EdgionPlugin::GraphqlGuard(_)
            | EdgionPlugin::GraphqlSafelist(_)
            | EdgionPlugin::GraphqlRateLimit(_)
            | EdgionPlugin::GraphqlOperationRouter(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::LdapAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::RateLimitLocal(_)
            | EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::WebhookKeyGet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::ForwardAuth(_)
            | EdgionPlugin::OpenidConnect(_)
            | EdgionPlugin::BandwidthLimit(_)
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::Guardrail(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::GeoIpLocation(_)
            | EdgionPlugin::Canary(_) => CreateResult::WrongStage,
        }
    }

    /// Create an UpstreamResponse instance from EdgionPlugin enum
    fn create_upstream_response_from_edgion(
        plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> CreateResult<Box<dyn UpstreamResponse>> {
        // Reference-bearing DSL is constructed from its bound LinkSys view by
        // the caller; reaching here without one is a misconfiguration.
        if HttpLinkSysConfig::is_reference_bearing(plugin) {
            return CreateResult::WrongStage;
        }
        match plugin {
            EdgionPlugin::Unsupported => CreateResult::WrongStage,
            EdgionPlugin::Dsl(config) => CreateResult::Ok(DslPlugin::from_config_response(config)),
            EdgionPlugin::ExtProc(_) | EdgionPlugin::Guardrail(_) => CreateResult::WrongStage,
            // Every other variant belongs to a different stage. Exhaustive
            // (no `_`), matching the other three `create_*_from_edgion`
            // constructors: these four matches are the runtime half of the
            // stage table declared by `applicable_stages` in
            // `edgion-resources/.../edgion_plugins/validate.rs`, and a `_` arm
            // here would let a new async-stage plugin silently resolve to
            // `WrongStage` instead of breaking the build.
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::CredentialInjector(_)
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
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::RestToGraphql(_)
            | EdgionPlugin::GraphqlGuard(_)
            | EdgionPlugin::GraphqlSafelist(_)
            | EdgionPlugin::GraphqlRateLimit(_)
            | EdgionPlugin::GraphqlOperationRouter(_)
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
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::Canary(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::GeoIpLocation(_)
            | EdgionPlugin::Wasm(_) => CreateResult::WrongStage,
        }
    }

    /// Create a body filter together with its explicit unary-response conflict fact.
    fn create_upstream_response_body_filter_from_edgion(
        plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> CreateResult<(Box<dyn UpstreamResponseBodyFilter>, bool)> {
        match plugin {
            EdgionPlugin::Unsupported => CreateResult::WrongStage,
            EdgionPlugin::BandwidthLimit(config) => CreateResult::Ok(Some((BandwidthLimit::create(config), true))),
            EdgionPlugin::Wasm(_) => CreateResult::WrongStage,
            // Not body-filter plugins
            EdgionPlugin::RequestHeaderModifier(_)
            | EdgionPlugin::ResponseHeaderModifier(_)
            | EdgionPlugin::RequestRedirect(_)
            | EdgionPlugin::UrlRewrite(_)
            | EdgionPlugin::RequestMirror(_)
            | EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::AiProxy(_)
            | EdgionPlugin::Cors(_)
            | EdgionPlugin::Csrf(_)
            | EdgionPlugin::CredentialInjector(_)
            | EdgionPlugin::IpRestriction(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::JweDecrypt(_)
            | EdgionPlugin::GrpcJsonTranscoder(_)
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::RestToGraphql(_)
            | EdgionPlugin::GraphqlGuard(_)
            | EdgionPlugin::GraphqlSafelist(_)
            | EdgionPlugin::GraphqlRateLimit(_)
            | EdgionPlugin::GraphqlOperationRouter(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::LdapAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestBodyBuffer(_)
            | EdgionPlugin::ResponseCache(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::RateLimitLocal(_)
            | EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::WebhookKeyGet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::ForwardAuth(_)
            | EdgionPlugin::OpenidConnect(_)
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::Guardrail(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::Canary(_)
            | EdgionPlugin::GeoIpLocation(_)
            | EdgionPlugin::Dsl(_) => CreateResult::WrongStage,
        }
    }

    /// Get plugin type name for error messages
    fn get_plugin_name(plugin: &EdgionPlugin) -> &'static str {
        match plugin {
            EdgionPlugin::Unsupported => "Unsupported",
            EdgionPlugin::RequestHeaderModifier(_) => "RequestHeaderModifier",
            EdgionPlugin::RequestRedirect(_) => "RequestRedirect",
            EdgionPlugin::ResponseHeaderModifier(_) => "ResponseHeaderModifier",
            EdgionPlugin::BasicAuth(_) => "BasicAuth",
            EdgionPlugin::AiProxy(_) => "AiProxy",
            EdgionPlugin::Cors(_) => "Cors",
            EdgionPlugin::Csrf(_) => "Csrf",
            EdgionPlugin::CredentialInjector(_) => "CredentialInjector",
            EdgionPlugin::IpRestriction(_) => "IpRestriction",
            EdgionPlugin::RequestAccessPolicy(_) => "RequestAccessPolicy",
            EdgionPlugin::GeoIpLocation(_) => "GeoIpLocation",
            EdgionPlugin::Canary(_) => "Canary",
            EdgionPlugin::JwtAuth(_) => "JwtAuth",
            EdgionPlugin::JweDecrypt(_) => "JweDecrypt",
            EdgionPlugin::JsonSchemaValidation(_) => "JsonSchemaValidation",
            EdgionPlugin::RestToGraphql(_) => "RestToGraphql",
            EdgionPlugin::GraphqlGuard(_) => "GraphqlGuard",
            EdgionPlugin::GraphqlSafelist(_) => "GraphqlSafelist",
            EdgionPlugin::GraphqlRateLimit(_) => "GraphqlRateLimit",
            EdgionPlugin::GraphqlOperationRouter(_) => "GraphqlOperationRouter",
            EdgionPlugin::GrpcJsonTranscoder(_) => "GrpcJsonTranscoder",
            EdgionPlugin::FormJsonTransform(_) => "FormJsonTransform",
            EdgionPlugin::HmacAuth(_) => "HmacAuth",
            EdgionPlugin::HeaderCertAuth(_) => "HeaderCertAuth",
            EdgionPlugin::KeyAuth(_) => "KeyAuth",
            EdgionPlugin::LdapAuth(_) => "LdapAuth",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::ProxyRewrite(_) => "ProxyRewrite",
            EdgionPlugin::RequestBodyBuffer(_) => "RequestBodyBuffer",
            EdgionPlugin::ResponseCache(_) => "ResponseCache",
            EdgionPlugin::RequestRestriction(_) => "RequestRestriction",
            EdgionPlugin::RateLimitLocal(_) => "RateLimitLocal",
            EdgionPlugin::RateLimitRedis(_) => "RateLimitRedis",
            EdgionPlugin::CtxSet(_) => "CtxSet",
            EdgionPlugin::WebhookKeyGet(_) => "WebhookKeyGet",
            EdgionPlugin::DirectEndpoint(_) => "DirectEndpoint",
            EdgionPlugin::RealIp(_) => "RealIp",
            EdgionPlugin::RequestId(_) => "RequestId",
            EdgionPlugin::ForwardAuth(_) => "ForwardAuth",
            EdgionPlugin::OpenidConnect(_) => "OpenidConnect",
            EdgionPlugin::DebugAccessLogToHeader(_) => "DebugAccessLogToHeader",
            EdgionPlugin::ResponseRewrite(_) => "ResponseRewrite",
            EdgionPlugin::BandwidthLimit(_) => "BandwidthLimit",
            EdgionPlugin::AllEndpointStatus(_) => "AllEndpointStatus",
            EdgionPlugin::DynamicInternalUpstream(_) => "DynamicInternalUpstream",
            EdgionPlugin::DynamicExternalUpstream(_) => "DynamicExternalUpstream",
            EdgionPlugin::FaultInjection(_) => "FaultInjection",
            EdgionPlugin::UrlRewrite(_) => "UrlRewrite",
            EdgionPlugin::RequestMirror(_) => "RequestMirror",
            EdgionPlugin::Dsl(_) => "Dsl",
            EdgionPlugin::RegionRoute(_) => "RegionRoute",
            EdgionPlugin::ExtProc(_) => "ExtProc",
            EdgionPlugin::Guardrail(_) => "Guardrail",
            EdgionPlugin::AiGuard(_) => "AiGuard",
            EdgionPlugin::Wasm(_) => "Wasm",
        }
    }

    /// Compile an entry's dye suite into a runtime plan.
    ///
    /// Only called when `entry.validate_dye()` found no schema-level error
    /// (the caller checks that first) — the schema check already ran
    /// `DyeSuite::validation_error`, which compiles every value template and
    /// guarantees the suite is well-formed. A `CompiledDyePlan::compile`
    /// failure here therefore means the runtime compiler and the schema
    /// validator disagree — a bug, not an operator error — so unlike the
    /// non-fatal schema-level dye error this fails closed
    /// (`invalid_plugins`), the same as every other "should have run but did
    /// not" compile error in this file.
    fn compile_entry_dye_plan(
        dye: Option<&edgion_resources::resources::edgion_plugins::DyeSuite>,
        plugin: &EdgionPlugin,
        context: &str,
        errors: &mut Vec<String>,
        invalid_plugins: &mut InvalidPlugins,
        ip: Option<&super::conditions::ip_refs::IpConditionCompiler<'_>>,
    ) -> Option<Arc<super::dye_plan::CompiledDyePlan>> {
        let suite = dye?;
        match super::dye_plan::CompiledDyePlan::with_ip(suite, plugin.dye_type_prefix(), ip) {
            Ok(plan) => Some(plan),
            Err(e) => {
                errors.push(format!(
                    "{context}: dye plan compile error (bug, already validated): {e}"
                ));
                invalid_plugins.record(plugin);
                None
            }
        }
    }

    fn add_request_filter(&mut self, filter: Box<dyn RequestFilter>) {
        self.requires_complete_request_body = true;
        self.has_upstream_jump_plugin |=
            edgion_resources::resources::edgion_plugins::edgion_plugin::is_upstream_jump_plugin_type(filter.name());
        self.request_plugins.push(Arc::from(filter));
    }

    fn add_upstream_response_filter(&mut self, filter: Box<dyn UpstreamResponseFilter>) {
        self.requires_complete_request_body = true;
        self.upstream_response_plugins.push(Arc::from(filter));
    }

    pub(crate) fn add_upstream_response_body_filter(&mut self, filter: Box<dyn UpstreamResponseBodyFilter>) {
        self.requires_complete_request_body = true;
        self.upstream_response_body_plugins.push(Arc::from(filter));
    }

    fn add_upstream_response(&mut self, filter: Box<dyn UpstreamResponse>) {
        self.requires_complete_request_body = true;
        self.upstream_response_async_plugins.push(Arc::from(filter));
    }

    pub(crate) fn cache_identity(&self) -> u64 {
        self.cache_identity
    }

    pub(crate) fn cache_capabilities(&self) -> &super::entry_bundle::CacheCapabilities {
        &self.cache_capabilities
    }

    pub(crate) fn cache_safe(&self) -> bool {
        self.cache_identity != 0
            && self.cache_capabilities.safe
            && self.invalid_plugins.is_empty()
            && !self.invalid_config_sealed
            && self.policy_blocks.is_empty()
            && self.waf_candidate.is_none()
            && !self.has_ai_conversion_owner
            && !self.has_unary_body_consumer
            && !self.has_upstream_jump_plugin
            && !self.has_grpc_transcoder
            && !self.has_graphql_operation_router
    }

    pub(crate) fn requires_complete_request_body(&self) -> bool {
        self.requires_complete_request_body
    }

    /// Backend dispatch occurs later; any candidate with filters or references
    /// must conservatively preserve complete-body availability.
    pub(crate) fn requires_complete_backend_body(&self) -> bool {
        !self.request_plugins.is_empty()
            || !self.upstream_response_plugins.is_empty()
            || !self.upstream_response_body_plugins.is_empty()
            || !self.upstream_response_async_plugins.is_empty()
            || !self.backend_ext_ref_keys.is_empty()
    }

    /// Get request stage plugin count
    pub fn request_plugins_count(&self) -> usize {
        self.request_plugins.len()
    }

    /// Get upstream_response_filter stage plugin count (sync)
    pub fn upstream_response_plugins_count(&self) -> usize {
        self.upstream_response_plugins.len()
    }

    /// Get upstream_response_body_filter stage plugin count (sync)
    pub fn upstream_response_body_plugins_count(&self) -> usize {
        self.upstream_response_body_plugins.len()
    }

    /// Get upstream_response stage plugin count (async)
    pub fn upstream_response_async_plugins_count(&self) -> usize {
        self.upstream_response_async_plugins.len()
    }

    /// Iterate over request stage filters
    pub fn request_plugins_iter(&self) -> impl Iterator<Item = &Arc<dyn RequestFilter>> {
        self.request_plugins.iter()
    }

    /// Iterate over upstream_response_filter stage filters (sync)
    pub fn upstream_response_plugins_iter(&self) -> impl Iterator<Item = &Arc<dyn UpstreamResponseFilter>> {
        self.upstream_response_plugins.iter()
    }

    /// Iterate over upstream_response_body_filter stage filters (sync)
    pub fn upstream_response_body_plugins_iter(&self) -> impl Iterator<Item = &Arc<dyn UpstreamResponseBodyFilter>> {
        self.upstream_response_body_plugins.iter()
    }

    /// Iterate over response_filter stage filters (async)
    pub fn upstream_response_async_plugins_iter(&self) -> impl Iterator<Item = &Arc<dyn UpstreamResponse>> {
        self.upstream_response_async_plugins.iter()
    }

    fn ensure_ip_unavailable_gate(&mut self) {
        if self.ip_unavailable_gate.is_none() {
            self.ip_unavailable_gate = Some(Arc::new(RejectFilter::new(vec!["IpList"])));
        }
    }
    pub(crate) fn ip_dependencies_available(&self, session: &mut dyn PluginSession) -> bool {
        if self.policy_blocked() || self.invalid_config_sealed {
            return true;
        }
        if self.ip_view_required {
            session.ensure_config_data_view();
        }
        if !self.required_ip.iter().all(|refs| refs.available(session)) {
            return false;
        }
        self.extension_checks
            .iter()
            .all(|extension| extension.ip_dependencies_available(session))
    }
    fn policy_blocked(&self) -> bool {
        self.request_plugins
            .first()
            .is_some_and(|filter| filter.name() == "PluginPolicyBlock")
    }

    /// Run request stage filters (async)
    pub async fn run_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(&self.request_plugins, s, ctx, false, RequestIdPhase::Normal, None)
            .await;
    }

    /// Run request stage filters on behalf of `level` (a delegated-chain
    /// ancestor) or, with `None`, of the matched leaf / ordinary route.
    pub(crate) async fn run_request_plugins_at(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        level: Option<usize>,
    ) {
        self.run_request_plugins_impl(&self.request_plugins, s, ctx, false, RequestIdPhase::Normal, level)
            .await;
    }

    pub(crate) fn rate_limit_local_outcome_slots(&self) -> usize {
        self.rate_limit_local_outcome_slots
    }

    /// Enforce a sealed policy or invalid-config gate before preflight or body work.
    /// The normal runner owns the response, access-log record and metric.
    pub(crate) async fn run_preflight_gate(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_preflight_gate_at(s, ctx, None).await;
    }

    /// `run_preflight_gate` for a delegated-chain ancestor (`None`: the leaf).
    pub(crate) async fn run_preflight_gate_at(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        level: Option<usize>,
    ) {
        if !ctx.plugin_terminated()
            && self
                .request_plugins
                .first()
                .is_some_and(|filter| matches!(filter.name(), "PluginPolicyBlock" | "RejectInvalidConfig"))
        {
            self.run_request_plugins_at(s, ctx, level).await;
        } else if !ctx.plugin_terminated() {
            if let Some(gate) = &self.invalid_extension_gate {
                self.run_request_plugins_impl(std::slice::from_ref(gate), s, ctx, false, RequestIdPhase::Normal, level)
                    .await;
            } else if self.ip_unavailable_gate.is_some() {
                self.run_request_plugins_impl(&[], s, ctx, false, RequestIdPhase::Normal, level)
                    .await;
            }
        }
    }

    /// Dispatch the selected HTTP backend's filters before its body pump starts.
    /// Only a first-attempt read-only mirror tee may be installed after the
    /// request-body policy freeze; capture and mutation remain unavailable.
    pub(crate) async fn run_backend_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(&self.request_plugins, s, ctx, true, RequestIdPhase::Normal, None)
            .await;
    }

    pub(crate) async fn run_early_request_ids(&self, s: &mut Session, ctx: &mut EdgionHttpContext) -> bool {
        if self.invalid_early_request_id || (self.has_request_id && !self.invalid_plugins.is_empty()) {
            return false;
        }
        self.run_request_plugins_impl(&self.request_plugins, s, ctx, false, RequestIdPhase::Early, None)
            .await;
        !ctx.plugin_terminated()
    }

    pub(crate) async fn run_request_plugins_after_early_ids(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(&self.request_plugins, s, ctx, false, RequestIdPhase::AfterEarly, None)
            .await;
    }

    async fn run_request_plugins_impl(
        &self,
        filters: &[Arc<dyn RequestFilter>],
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        backend_request: bool,
        phase: RequestIdPhase,
        level: Option<usize>,
    ) {
        if filters.is_empty() {
            return;
        }

        // Snapshot before `ctx` is borrowed by `session_adapter`. Set in
        // `pg_request_filter` and not mutated during plugin execution.
        let is_grpc_request = ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched;
        let outcome_frame = if phase == RequestIdPhase::Early {
            None
        } else {
            ctx.begin_rate_limit_local_frame(self.rate_limit_local_outcome_slots)
        };
        let mut filter_logs = Vec::new();
        let mut session_adapter = if backend_request {
            PingoraSessionAdapter::for_backend_request(s, ctx)
        } else {
            PingoraSessionAdapter::new(s, ctx).with_level(level)
        };

        let unavailable = !self.policy_blocked() && !self.ip_dependencies_available(&mut session_adapter);
        let filters = if unavailable {
            self.ip_unavailable_gate
                .as_ref()
                .map(std::slice::from_ref)
                .unwrap_or(filters)
        } else {
            filters
        };
        for filter in filters {
            let is_request_id = filter.name() == "RequestId";
            if !unavailable
                && ((phase == RequestIdPhase::Early && !is_request_id)
                    || (phase == RequestIdPhase::AfterEarly && is_request_id))
            {
                continue;
            }
            let previous_identity = session_adapter.replace_plugin_entry_identity(
                crate::ctx::CurrentPluginEntryIdentity::new(filter.name(), filter.alias()),
            );
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Request-direction
            // anti-spoof strip runs here too, before the plugin's kill
            // switch / gating / conditions, so a client-forged value of a
            // name this entry declares can never survive into the plugin.
            session_adapter.install_dye_plan(filter.dye_plan().cloned());
            session_adapter.strip_request_dye_once();

            let suppress_log = filter.suppress_automatic_log();
            let mut plugin_log = if suppress_log {
                PluginLog::disabled()
            } else {
                PluginLog::with_alias(filter.name(), filter.alias())
            };
            let start = (!suppress_log).then(std::time::Instant::now);

            // If this future is cancelled while the plugin is awaiting, the
            // whole request-stage future and adapter are dropped; no later
            // plugin can resume and register an observer under the stale
            // identity. Every completed invocation restores before any result
            // branch below.
            let result = filter.run_request(&mut session_adapter, &mut plugin_log).await;
            session_adapter.restore_plugin_entry_identity(previous_identity);

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if let Some(start) = start.filter(|_| plugin_log.refer_to.is_none()) {
                plugin_log.time_cost = Some(elapsed_micros(start));
            }
            if !suppress_log {
                if filter_logs.is_empty() {
                    filter_logs.reserve(filters.len());
                }
                filter_logs.push(plugin_log);
            }

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                break;
            } else if let PluginRunningResult::ErrResponse { status, body } = result {
                let (status, body) = if edgion_resources::http_status::is_valid_final_http_status(status) {
                    (status, body)
                } else {
                    (500, None)
                };
                if is_grpc_request {
                    // gRPC route: route to the gRPC-native error path. The
                    // HTTP-style resp built below would otherwise be discarded
                    // by `write_response_header`'s gRPC re-route (see
                    // `send_grpc_trailers_only`), losing any queued
                    // response-header ops. `send_grpc_error_direct` drains
                    // those ops itself and re-asserts the gRPC content-type
                    // contract.
                    let (grpc_status, default_msg) = http_to_grpc_status(status);
                    // `grpc-message` is a trailer value, so only a valid UTF-8
                    // body can carry it. A non-UTF-8 body (reachable now that
                    // the body is raw `Bytes`, e.g. from a Wasm guest) falls
                    // back to the mapped default rather than putting invalid
                    // bytes into a header.
                    let body_msg = body.as_deref().and_then(|b| std::str::from_utf8(b).ok());
                    if body.is_some() && body_msg.is_none() {
                        session_adapter.note_grpc_message_body_not_utf8();
                    }
                    let message = body_msg.or(Some(default_msg));
                    let _ = session_adapter.send_grpc_error_direct(grpc_status, message).await;
                } else if let Ok(status_code) = http::StatusCode::from_u16(status) {
                    if let Ok(mut resp) = ResponseHeader::build(status_code, Some(2)) {
                        let _ = resp.insert_header("content-type", "text/plain");
                        let len = body.as_ref().map(|b| b.len()).unwrap_or(0);
                        // Apply queued ops first so plugin-set values (e.g. Mock's
                        // Content-Type) override the runtime defaults, THEN re-assert
                        // framing so a queued Content-Length / Transfer-Encoding (now
                        // reachable via an ext_proc ImmediateResponse's header mutation)
                        // cannot desync the wire from the actual body length.
                        session_adapter.drain_pending_response_header_ops(&mut resp);
                        let _ = resp.remove_header(&http::header::TRANSFER_ENCODING);
                        let _ = resp.insert_header("content-length", len.to_string());

                        if session_adapter
                            .write_response_header(Box::new(resp), body.is_none())
                            .await
                            .is_ok()
                        {
                            if let Some(b) = body {
                                let _ = session_adapter.write_response_body(Some(b), true).await;
                            }
                        }
                    }
                }
                session_adapter.set_terminate();
                break;
            } else if let PluginRunningResult::ErrGrpcError { grpc_status, message } = result {
                // Send gRPC native error: HTTP 200 + grpc-status/grpc-message trailers
                let _ = session_adapter
                    .send_grpc_error_direct(grpc_status, message.as_deref())
                    .await;
                session_adapter.set_terminate();
                break;
            }
        }

        if session_adapter.ctx().plugin_terminated() {
            // A bare backend termination is answered by terminate_with_fallback
            // after this adapter is dropped; set framing before that writer.
            session_adapter.prepare_local_reply_transport();
        }

        ctx.finish_plugin_stage("request_filters", filter_logs);
        ctx.end_rate_limit_local_frame(outcome_frame);
    }

    /// Run upstream_response_filter stage filters (sync)
    pub fn run_upstream_response_plugins_sync(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        self.run_upstream_response_plugins_sync_at(s, ctx, response_header, None);
    }

    /// `run_upstream_response_plugins_sync` for a delegated-chain ancestor.
    pub(crate) fn run_upstream_response_plugins_sync_at(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
        level: Option<usize>,
    ) {
        let mut filter_logs = Vec::new();
        let mut session_adapter =
            PingoraSessionAdapter::with_response_header(s, ctx, response_header).with_level(level);
        if !self.ip_dependencies_available(&mut session_adapter) {
            let mut log = PluginLog::new("IpList");
            check_response_stage_terminate(
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                },
                &mut log,
                &mut session_adapter,
            );
            session_adapter.set_terminate();
            drop(session_adapter);
            ctx.finish_plugin_stage("upstream_response_filters", vec![log]);
            return;
        }

        for filter in &self.upstream_response_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Response-stage
            // loops do not strip: request-direction rules are meaningless
            // here (validation forbids them on non-request-stage entries).
            session_adapter.install_dye_plan(filter.dye_plan().cloned());

            let suppress_log = filter.suppress_automatic_log();
            let mut plugin_log = if suppress_log {
                PluginLog::disabled()
            } else {
                PluginLog::with_alias(filter.name(), filter.alias())
            };
            let start = (!suppress_log).then(std::time::Instant::now);

            let result = filter.run_upstream_response_filter(&mut session_adapter, &mut plugin_log);

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if let Some(start) = start.filter(|_| plugin_log.refer_to.is_none()) {
                plugin_log.time_cost = Some(elapsed_micros(start));
            }

            let should_terminate = check_response_stage_terminate(result, &mut plugin_log, &mut session_adapter);
            if !suppress_log {
                if filter_logs.is_empty() {
                    filter_logs.reserve(self.upstream_response_plugins.len());
                }
                filter_logs.push(plugin_log);
            }

            if should_terminate {
                session_adapter.set_terminate();
                break;
            }
        }

        ctx.finish_plugin_stage("upstream_response_filters", filter_logs);
    }

    /// Run upstream_response_body_filter stage filters (sync)
    ///
    /// Called for each body chunk received from upstream. Each plugin can return
    /// an optional Duration for bandwidth throttling. When multiple plugins return
    /// a duration, the largest (most restrictive) one wins.
    ///
    /// Body filters do not produce per-chunk PluginLog entries (see
    /// `UpstreamResponseBodyFilter` trait docs): a long stream would emit
    /// thousands of identical entries, so the trait omits the log argument and
    /// no PluginLog is allocated here per chunk.
    ///
    /// # Arguments
    /// * `s` - The Pingora session (for condition evaluation)
    /// * `ctx` - The request context
    /// * `body` - The body chunk data (read-only)
    /// * `end_of_stream` - Whether this is the last chunk
    ///
    /// # Returns
    /// * `None` - No throttling
    /// * `Some(duration)` - Delay next chunk by this duration
    pub fn run_upstream_response_body_plugins(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
    ) -> Option<Duration> {
        self.run_upstream_response_body_plugins_at(s, ctx, body, end_of_stream, None)
    }

    /// `run_upstream_response_body_plugins` for a delegated-chain ancestor.
    pub(crate) fn run_upstream_response_body_plugins_at(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        level: Option<usize>,
    ) -> Option<Duration> {
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx).with_level(level);
        if !self.ip_dependencies_available(&mut session_adapter) {
            session_adapter.mark_ip_body_denied();
            return None;
        }
        if self.upstream_response_body_plugins.is_empty() {
            return None;
        }
        let mut max_delay: Option<Duration> = None;

        for filter in &self.upstream_response_body_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Body filters have no
            // PluginLog anchor (see the trait's no-per-chunk-logging doc), so
            // this is the loop-top install rather than the PluginLog line.
            session_adapter.install_dye_plan(filter.dye_plan().cloned());

            if let Some(d) = filter.run_upstream_response_body_filter(body, end_of_stream, &mut session_adapter) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(d),
                    None => d,
                });
            }
            if session_adapter.ctx().ip_body_denied() {
                break;
            }
        }

        max_delay
    }

    pub(crate) fn check_body_ip_dependencies(&self, session: &mut Session, ctx: &mut EdgionHttpContext) {
        self.check_body_ip_dependencies_at(session, ctx, None);
    }

    /// `check_body_ip_dependencies` for a delegated-chain ancestor (`None`:
    /// the leaf). The level makes this runtime's ExtensionRef availability
    /// checks resolve against its own level's ExtensionRef array.
    pub(crate) fn check_body_ip_dependencies_at(
        &self,
        session: &mut Session,
        ctx: &mut EdgionHttpContext,
        level: Option<usize>,
    ) {
        let mut adapter = PingoraSessionAdapter::new(session, ctx).with_level(level);
        if !self.ip_dependencies_available(&mut adapter) {
            adapter.mark_ip_body_denied();
        }
    }

    /// Run response_filter stage filters (async)
    pub async fn run_upstream_response_plugins_async(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        self.run_upstream_response_plugins_async_at(s, ctx, response_header, None)
            .await;
    }

    /// `run_upstream_response_plugins_async` for a delegated-chain ancestor.
    pub(crate) async fn run_upstream_response_plugins_async_at(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
        level: Option<usize>,
    ) {
        let mut filter_logs = Vec::new();
        let mut session_adapter =
            PingoraSessionAdapter::with_response_header(s, ctx, response_header).with_level(level);
        if !self.ip_dependencies_available(&mut session_adapter) {
            let mut log = PluginLog::new("IpList");
            check_response_stage_terminate(
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                },
                &mut log,
                &mut session_adapter,
            );
            session_adapter.set_terminate();
            drop(session_adapter);
            ctx.finish_plugin_stage("upstream_responses", vec![log]);
            return;
        }

        for filter in &self.upstream_response_async_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Response-stage
            // loops do not strip: request-direction rules are meaningless
            // here (validation forbids them on non-request-stage entries).
            session_adapter.install_dye_plan(filter.dye_plan().cloned());

            let suppress_log = filter.suppress_automatic_log();
            let mut plugin_log = if suppress_log {
                PluginLog::disabled()
            } else {
                PluginLog::with_alias(filter.name(), filter.alias())
            };
            let start = (!suppress_log).then(std::time::Instant::now);

            let result = filter
                .run_upstream_response(&mut session_adapter, &mut plugin_log)
                .await;

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if let Some(start) = start.filter(|_| plugin_log.refer_to.is_none()) {
                plugin_log.time_cost = Some(elapsed_micros(start));
            }

            let should_terminate = check_response_stage_terminate(result, &mut plugin_log, &mut session_adapter);
            if !suppress_log {
                if filter_logs.is_empty() {
                    filter_logs.reserve(self.upstream_response_async_plugins.len());
                }
                filter_logs.push(plugin_log);
            }

            if should_terminate {
                session_adapter.set_terminate();
                break;
            }
        }

        ctx.finish_plugin_stage("upstream_responses", filter_logs);
    }
}

/// Check if a plugin result should terminate execution in the response stage,
/// and — for HTTP routes — turn an `ErrResponse { status, body }` into a local
/// reply that fully replaces the upstream response (status + headers + body).
///
/// Building the local reply is synchronous, so this is shared by both the sync
/// (`run_upstream_response_plugins_sync`) and async
/// (`run_upstream_response_plugins_async`) loops: it stashes the built reply in
/// the context's pending response override. `response_filter` then writes
/// that reply to the downstream and returns a sentinel `Err` to abort the
/// upstream forward — this is what makes a body-less upstream (where Pingora
/// never calls the body filter) still emit the replacement body. gRPC routes
/// are not supported: the response is terminated by status only (body not
/// replaced) and a marker is recorded.
// `pub(crate)` for the regression tests in `session_adapter`, which owns the
// only real `pingora_proxy::Session` fixture in the crate.
pub(crate) fn check_response_stage_terminate(
    result: PluginRunningResult,
    plugin_log: &mut PluginLog,
    adapter: &mut PingoraSessionAdapter<'_>,
) -> bool {
    match result {
        PluginRunningResult::ErrTerminateRequest => {
            // The same silent-ALLOW shape as the refused `ErrResponse` below,
            // but unconditional: no precondition is needed for it to leak,
            // because returning `true` here only makes the runner call
            // `set_terminate`, which writes a field no response-stage reader
            // consults. Answer it the way the request stage does — 502 fallback
            // plus the plugin-bug marker.
            plugin_log.push("deny terminate");
            adapter.set_response_terminate_without_reply();
            true
        }
        PluginRunningResult::ErrResponse { status, body } => {
            if adapter.apply_response_local_reply(status, body) {
                plugin_log.push_fmt("replace response {}", la![status.to_string().as_str()]);
            } else {
                // Refused (gRPC route, real 101 / established CONNECT tunnel,
                // interim 1xx, or no header bound): the exchange cannot carry a
                // replacement response, so arm the response-stage abort. Logging
                // alone would forward the upstream response unmodified — the
                // runners' `set_terminate` writes the request plugin marker,
                // which has no response-stage reader. Only tag the gRPC marker
                // for genuine gRPC routes so it does not mislabel 101/1xx.
                plugin_log.push_fmt("deny response {}", la![status.to_string().as_str()]);
                if adapter.is_grpc_request() {
                    adapter.note_response_local_reply_unsupported();
                }
                adapter.set_response_denied(status);
            }
            true
        }
        PluginRunningResult::ErrGrpcError { grpc_status, message } => {
            // Same shape as the refused `ErrResponse` above: the response-stage
            // runners have no gRPC writer (the sync one cannot await
            // `send_grpc_error_direct`, which is how the request stage answers
            // this result), so without arming the abort the denial is a log line
            // and the upstream response is forwarded.
            plugin_log.push_fmt("deny grpc {}", la![grpc_status.to_string().as_str()]);
            adapter.set_response_denied_grpc(grpc_status, message);
            true
        }
        PluginRunningResult::Nothing | PluginRunningResult::GoodNext => false,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test/condition_body_requirement_tests.rs"]
mod condition_body_requirement_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test/header_modifier_validation_tests.rs"]
mod header_modifier_validation_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test/ext_ref_index_tests.rs"]
mod ext_ref_index_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test/transport_admission_tests.rs"]
mod transport_admission_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/plugin_runtime_test/response_cache_tests.rs"]
pub(crate) mod response_cache_tests;
