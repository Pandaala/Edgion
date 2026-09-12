//! Plugin runtime - manages plugin execution across different stages

use std::sync::Arc;
use std::time::{Duration, Instant};

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::plugins::runtime::log::la;
use crate::runtime::server::http_to_grpc_status;
use crate::EdgionHttpContext;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::filters::PluginRunningResult::ErrTerminateRequest;
use edgion_resources::resources::edgion_plugins::{CorsConfig, WafConfig};
use edgion_resources::resources::extension_ref::{validate_extension_ref_filter, ExtensionRefValidationError};
use edgion_resources::resources::header_modifier::{validate_grpc_filter_list, validate_http_filter_list};
use edgion_resources::resources::http_route::MirrorTuning;
use edgion_resources::resources::{
    EdgionPlugin, GRPCRouteFilter, GRPCRouteFilterType, HTTPRouteFilter, HTTPRouteFilterType,
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
use crate::plugins::http::guardrail::Guardrail;
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
}

pub struct PluginRuntime {
    rate_limit_local_outcome_slots: usize,
    /// Plugins for request stage (async)
    request_plugins: Vec<Arc<dyn RequestFilter>>,
    /// Plugins for upstream_response_filter stage (sync)
    upstream_response_plugins: Vec<Arc<dyn UpstreamResponseFilter>>,
    /// Plugins for upstream_response_body_filter stage (sync, bandwidth throttling)
    upstream_response_body_plugins: Vec<Arc<dyn UpstreamResponseBodyFilter>>,
    /// Plugins for response_filter stage (async)
    upstream_response_async_plugins: Vec<Arc<dyn UpstreamResponse>>,
    /// (namespace, name) of every ExtensionRef in this runtime, in build order.
    /// Position == index assigned via `ExtensionRefFilter::set_index`. Empty
    /// when this runtime is built via the backend-scope path
    /// (`from_httproute_filters_for_backend` / `from_grpcroute_filters_for_backend`),
    /// because backend-scope ExtensionRefFilters keep `index = None`; their
    /// first store result is pinned in the logical request context instead.
    ext_ref_keys: Vec<(String, String)>,
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
            request_plugins: vec![],
            upstream_response_plugins: vec![],
            upstream_response_body_plugins: vec![],
            upstream_response_async_plugins: vec![],
            ext_ref_keys: Vec::new(),
            cors_config: None,
            invalid_plugins: InvalidPlugins::default(),
            invalid_config_sealed: false,
            policy_blocks: Vec::new(),
            policy_bypassed: Vec::new(),
            waf_candidate: None,
            has_request_id: false,
            invalid_early_request_id: false,
            rate_limit_local_outcome_slots: 0,
        }
    }

    fn into_entry_outcome(self, errors: Vec<String>) -> EntryOutcome {
        let failed = !self.invalid_plugins.is_empty();
        let bundle = EntryBundle {
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

    pub(crate) fn add_waf_with_cache(&mut self, config: &WafConfig, cache: &mut EntryCache) -> Vec<String> {
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
                });
                self.add_request_filter(Box::new(plugin.clone()));
                self.add_upstream_response_filter(Box::new(plugin.clone()));
                self.add_upstream_response(Box::new(plugin));
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

    /// Publication policy for an update that failed runtime construction.
    pub(crate) fn invalid_update_policy(&self) -> Option<InvalidUpdatePolicy> {
        self.invalid_plugins.update_policy()
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
    pub fn ext_ref_keys(&self) -> &[(String, String)] {
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

    /// Build a runtime for a route rule. Assigns ExtensionRef indices for the
    /// per-request resolve cache and records (ns, name) in `ext_ref_keys`.
    /// `mirror_tuning` carries the route-level `edgion.io/mirror-*` annotation
    /// values (pre-parsed onto the rule) applied to every RequestMirror filter.
    pub fn from_httproute_filters_for_rule(
        filters: &[HTTPRouteFilter],
        namespace: &str,
        mirror_tuning: &MirrorTuning,
    ) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_httproute_filters_inner(filters, namespace, true, mirror_tuning);
        runtime.seal_fail_closed();
        runtime
    }

    /// Build a runtime for a backend reference. ExtensionRef filters retain
    /// `index = None` and fall back to direct global store lookup at exec time
    /// (no per-request resolve cache for backend scope yet).
    pub fn from_httproute_filters_for_backend(
        filters: &[HTTPRouteFilter],
        namespace: &str,
        mirror_tuning: &MirrorTuning,
    ) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_httproute_filters_inner(filters, namespace, false, mirror_tuning);
        runtime.seal_fail_closed();
        runtime
    }

    fn add_from_httproute_filters_inner(
        &mut self,
        filters: &[HTTPRouteFilter],
        namespace: &str,
        assign_indices: bool,
        mirror_tuning: &MirrorTuning,
    ) {
        if filters.iter().any(|filter| {
            matches!(
                validate_extension_ref_filter(
                    filter.filter_type == HTTPRouteFilterType::ExtensionRef,
                    filter.extension_ref.as_ref(),
                ),
                Err(ExtensionRefValidationError::MissingConfiguration)
                    | Err(ExtensionRefValidationError::UnexpectedConfiguration)
            )
        }) {
            self.invalid_plugins.record_non_retainable("ExtensionRef");
            return;
        }
        if validate_http_filter_list(filters).is_err() {
            self.invalid_plugins.record_non_retainable("HeaderModifier");
            return;
        }
        for filter in filters {
            match filter.filter_type {
                HTTPRouteFilterType::RequestHeaderModifier => {
                    if let Some(config) = &filter.request_header_modifier {
                        match RequestHeaderModifierFilter::new(config.clone()) {
                            Ok(filter) => self.add_request_filter(Box::new(filter)),
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
                        self.add_request_filter(Box::new(RequestRedirectFilter::new(config.clone())));
                    }
                }
                HTTPRouteFilterType::URLRewrite => {
                    if let Some(config) = &filter.url_rewrite {
                        self.add_request_filter(Box::new(URLRewriteFilter::new(config.clone())));
                    }
                }
                HTTPRouteFilterType::RequestMirror => {
                    if let Some(config) = &filter.request_mirror {
                        self.add_request_filter(Box::new(RequestMirrorPlugin::new(
                            config.clone(),
                            mirror_tuning.clone(),
                            namespace.to_string(),
                        )));
                    }
                }
                HTTPRouteFilterType::CORS => {
                    if let Some(config) = &filter.cors {
                        // try_new validation failures are reported via the controller:
                        // bad configs flag the rule in HTTPRoute.spec.invalid_rule_indices
                        // and surface as PartiallyInvalid status (see http_route handler's
                        // find_invalid_rule_details). Silently drop here — the affected
                        // rule is excluded from the data-plane route table so this filter
                        // would never be invoked anyway.
                        if let Ok(cors_filter) = GapiCorsFilter::try_new(config.clone()) {
                            self.add_request_filter(Box::new(cors_filter.clone()));
                            self.add_upstream_response_filter(Box::new(cors_filter));
                        }
                    }
                }
                HTTPRouteFilterType::ExternalAuth => {
                    if let Some(config) = &filter.external_auth {
                        match ForwardAuth::new(config) {
                            Ok(fa) => self.add_request_filter(Box::new(fa)),
                            Err(_) => self.invalid_plugins.record_non_retainable("ForwardAuth"),
                        }
                    }
                }
                HTTPRouteFilterType::ExtensionRef => {
                    // Unsupported targets retain the existing ExtensionRef
                    // runtime so their kind-specific diagnostics remain
                    // attributable. Missing payloads were rejected above.
                    if let Some(ext_ref) = &filter.extension_ref {
                        let extension_namespace = ext_ref.resolved_namespace.as_deref().unwrap_or(namespace);
                        let mut ext_filter = ExtensionRefFilter::new(extension_namespace.to_string(), ext_ref.clone());
                        if assign_indices {
                            let idx = self.ext_ref_keys.len();
                            self.ext_ref_keys
                                .push((extension_namespace.to_string(), ext_ref.name.clone()));
                            ext_filter.set_index(idx);
                        }
                        self.add_request_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_body_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response(Box::new(ext_filter));
                    }
                }
            }
        }
    }

    /// Build a runtime for a gRPC route rule. Assigns ExtensionRef indices for
    /// the per-request resolve cache and records (ns, name) in `ext_ref_keys`.
    pub fn from_grpcroute_filters_for_rule(filters: &[GRPCRouteFilter], namespace: &str) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_grpcroute_filters_inner(filters, namespace, true);
        runtime.seal_fail_closed();
        runtime
    }

    /// Build a runtime for a gRPC backend reference. ExtensionRef filters retain
    /// `index = None` and fall back to direct global store lookup at exec time.
    pub fn from_grpcroute_filters_for_backend(filters: &[GRPCRouteFilter], namespace: &str) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_grpcroute_filters_inner(filters, namespace, false);
        runtime.seal_fail_closed();
        runtime
    }

    fn add_from_grpcroute_filters_inner(&mut self, filters: &[GRPCRouteFilter], namespace: &str, assign_indices: bool) {
        if filters.iter().any(|filter| {
            matches!(
                validate_extension_ref_filter(
                    filter.filter_type == GRPCRouteFilterType::ExtensionRef,
                    filter.extension_ref.as_ref(),
                ),
                Err(ExtensionRefValidationError::MissingConfiguration)
                    | Err(ExtensionRefValidationError::UnexpectedConfiguration)
            )
        }) {
            self.invalid_plugins.record_non_retainable("ExtensionRef");
            return;
        }
        if validate_grpc_filter_list(filters).is_err() {
            self.invalid_plugins.record_non_retainable("HeaderModifier");
            return;
        }
        for filter in filters {
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
                    if let Some(ext_ref) = &filter.extension_ref {
                        let extension_namespace = ext_ref.resolved_namespace.as_deref().unwrap_or(namespace);
                        let mut ext_filter = ExtensionRefFilter::new(extension_namespace.to_string(), ext_ref.clone());
                        if assign_indices {
                            let idx = self.ext_ref_keys.len();
                            self.ext_ref_keys
                                .push((extension_namespace.to_string(), ext_ref.name.clone()));
                            ext_filter.set_index(idx);
                        }
                        self.add_request_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_body_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response(Box::new(ext_filter));
                    }
                }
            }
        }
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
    pub fn add_from_request_filters(
        &mut self,
        entries: &[RequestFilterEntry],
        namespace: &str,
        resource_key: &str,
    ) -> Vec<String> {
        self.add_from_request_filters_with_cache(entries, namespace, resource_key, &mut EntryCache::default())
    }

    pub(crate) fn add_from_request_filters_with_cache(
        &mut self,
        entries: &[RequestFilterEntry],
        namespace: &str,
        resource_key: &str,
        cache: &mut EntryCache,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        // Keep source aliases for dependency semantics even when an invalid or
        // duplicate display identity has been sanitized by the generation.
        let outcome_aliases_owned = cache
            .prepare_request_outcomes(entries, self.rate_limit_local_outcome_slots)
            .clone();
        let source_aliases = cache.request_source_aliases().to_vec();
        let mut dependency_entries = entries.to_vec();
        for (entry, alias) in dependency_entries.iter_mut().zip(&source_aliases) {
            entry.alias = alias.clone();
        }
        edgion_resources::resources::edgion_plugins::validate::collect_rate_limit_dependency_errors(
            &dependency_entries,
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
        let mut invalid_bounded_structured_entries = vec![false; entries.len()];
        // Freeze config validation before the bounded-body prepass. A config
        // error owns the entry and suppresses later body-policy errors, matching
        // `EdgionPlugins::validate_plugins` and avoiding Controller/Gateway
        // error-set drift when both parts are invalid.
        let config_validation_errors: Vec<Option<String>> = entries
            .iter()
            .map(|entry| {
                if entry.is_enabled() {
                    Self::plugin_validation_error(&entry.plugin)
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
                        | EdgionPlugin::JsonSchemaValidation(_)
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
            let body_errors =
                edgion_resources::resources::edgion_plugins::bounded_structured_body_validation_errors(body);
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
                if invalid_bounded_structured_entries[index] {
                    continue;
                }

                let slot = EntrySlot {
                    stage: EntryStage::Request,
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
                    if let Some(error) = &config_validation_errors[index] {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, error));
                        entry_runtime.invalid_plugins.record(&entry.plugin);
                        break 'build_entry;
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

                    if let EdgionPlugin::ExtProc(config) = &entry.plugin {
                        let identity = format!(
                            "{resource_key}/requestPlugins/{}",
                            entry_alias
                                .as_deref()
                                .map_or_else(|| format!("#{index}"), str::to_string)
                        );
                        match ExtProc::create_attached(config, identity) {
                            Some((request, response)) => {
                                let gated: Box<dyn RequestFilter> = match body_req {
                                    Some(req) => Box::new(BodyGatedRequestFilter::new(request, req)),
                                    None => request,
                                };
                                entry_runtime.add_request_filter(Box::new(
                                    ConditionalRequestFilter::with_alias_and_dye(
                                        gated,
                                        entry.conditions.clone(),
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

                    if let EdgionPlugin::Guardrail(config) = &entry.plugin {
                        let identity = format!(
                            "{resource_key}/requestPlugins/{}",
                            entry_alias
                                .as_deref()
                                .map_or_else(|| format!("#{index}"), str::to_string)
                        );
                        match Guardrail::create_attached(config, identity) {
                            Some((request, response)) => {
                                entry_runtime.add_request_filter(Box::new(
                                    ConditionalRequestFilter::with_alias_and_dye(
                                        request,
                                        entry.conditions.clone(),
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

                    let created = match &entry.plugin {
                        EdgionPlugin::RateLimitLocal(config) => {
                            CreateResult::Ok(Some(RateLimitLocal::create_with_outcome_slot(
                                config,
                                source_aliases[index]
                                    .as_deref()
                                    .and_then(|alias| outcome_aliases.get(alias).copied()),
                            )))
                        }
                        _ => Self::create_request_filter_from_edgion(
                            &entry.plugin,
                            namespace,
                            body_req.clone(),
                            &entry_scope,
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
                                Some(req) if generic_body_gate => Box::new(BodyGatedRequestFilter::new(filter, req)),
                                None => filter,
                                Some(_) => filter,
                            };
                            // Wrap with ConditionalRequestFilter to support condition evaluation
                            let mut conditional_filter = ConditionalRequestFilter::with_alias_and_dye(
                                gated,
                                entry.conditions.clone(),
                                entry_alias,
                                dye_plan,
                            );
                            conditional_filter.bind_rate_limit_dependencies(&outcome_aliases);
                            entry_runtime.add_request_filter(Box::new(conditional_filter));
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
        self.add_from_upstream_response_filters_with_cache(entries, namespace, &mut EntryCache::default())
    }

    pub(crate) fn add_from_upstream_response_filters_with_cache(
        &mut self,
        entries: &[UpstreamResponseFilterEntry],
        namespace: &str,
        cache: &mut EntryCache,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
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
                    if let Some(error) = Self::plugin_validation_error(&entry.plugin) {
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
                        )
                    } else {
                        None
                    };

                    match Self::create_upstream_response_filter_from_edgion(&entry.plugin, namespace) {
                        CreateResult::Ok(Some(filter)) => {
                            // Wrap with ConditionalUpstreamResponseFilter to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponseFilter::with_alias_and_dye(
                                filter,
                                entry.conditions.clone(),
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
        self.add_from_upstream_responses_with_cache(entries, namespace, &mut EntryCache::default())
    }

    pub(crate) fn add_from_upstream_responses_with_cache(
        &mut self,
        entries: &[UpstreamResponseEntry],
        namespace: &str,
        cache: &mut EntryCache,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
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
                    if let Some(error) = Self::plugin_validation_error(&entry.plugin) {
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
                        )
                    } else {
                        None
                    };

                    match Self::create_upstream_response_from_edgion(&entry.plugin, namespace) {
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
                            let gated: Box<dyn UpstreamResponse> = match body_req {
                                Some(req) => Box::new(BodyGatedUpstreamResponse::new(filter, req)),
                                None => filter,
                            };
                            // Wrap with ConditionalUpstreamResponse to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponse::with_alias_and_dye(
                                gated,
                                entry.conditions.clone(),
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
        self.add_from_upstream_response_body_filters_with_cache(entries, namespace, &mut EntryCache::default())
    }

    pub(crate) fn add_from_upstream_response_body_filters_with_cache(
        &mut self,
        entries: &[UpstreamResponseBodyFilterEntry],
        namespace: &str,
        cache: &mut EntryCache,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        // Per-stage-array alias accumulator; see `duplicate_alias_error`.
        let mut seen_aliases = std::collections::HashSet::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
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
                    if let Some(error) = Self::plugin_validation_error(&entry.plugin) {
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
                        )
                    } else {
                        None
                    };

                    match Self::create_upstream_response_body_filter_from_edgion(&entry.plugin, namespace) {
                        CreateResult::Ok(Some(filter)) => {
                            // Wrap with ConditionalUpstreamResponseBodyFilter to support condition evaluation
                            let conditional_filter = ConditionalUpstreamResponseBodyFilter::with_alias_and_dye(
                                filter,
                                entry.conditions.clone(),
                                entry_alias,
                                dye_plan,
                            );
                            entry_runtime.add_upstream_response_body_filter(Box::new(conditional_filter));
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
    fn plugin_validation_error(plugin: &EdgionPlugin) -> Option<String> {
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
            EdgionPlugin::HmacAuth(_) => false,
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
    fn create_request_filter_from_edgion(
        plugin: &EdgionPlugin,
        namespace: &str,
        body_requirement: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
        entry_scope: &str,
    ) -> CreateResult<Box<dyn RequestFilter>> {
        match plugin {
            EdgionPlugin::RequestHeaderModifier(config) => CreateResult::Ok(
                RequestHeaderModifierFilter::new(config.clone())
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn RequestFilter>),
            ),
            EdgionPlugin::RequestRedirect(config) => {
                CreateResult::Ok(Some(Box::new(RequestRedirectFilter::new(config.clone()))))
            }
            EdgionPlugin::UrlRewrite(config) => CreateResult::Ok(Some(Box::new(URLRewriteFilter::new(config.clone())))),
            EdgionPlugin::RequestMirror(config) => CreateResult::Ok(Some(Box::new(RequestMirrorPlugin::new(
                config.filter.clone(),
                config.tuning.clone(),
                namespace.to_string(),
            )))),
            EdgionPlugin::BasicAuth(config) => CreateResult::Ok(Some(Box::new(BasicAuth::new(config)))),
            EdgionPlugin::AiProxy(config) => CreateResult::Ok(AiProxy::create(config, namespace, entry_scope)),
            EdgionPlugin::AiGuard(config) => CreateResult::Ok(AiGuard::create(config, entry_scope)),
            EdgionPlugin::Cors(config) => CreateResult::Ok(Some(Box::new(Cors::new(config)))),
            EdgionPlugin::Csrf(config) => CreateResult::Ok(Some(Box::new(Csrf::new(config)))),
            EdgionPlugin::CredentialInjector(config) => CreateResult::Ok(CredentialInjector::create(config)),
            EdgionPlugin::IpRestriction(config) => CreateResult::Ok(Some(IpRestriction::create(config, namespace))),
            EdgionPlugin::RequestAccessPolicy(config) => {
                CreateResult::Ok(Some(RequestAccessPolicy::create(config, namespace, body_requirement)))
            }
            EdgionPlugin::GeoIpLocation(config) => CreateResult::Ok(Some(GeoIpLocation::create(config))),
            EdgionPlugin::Canary(config) => CreateResult::Ok(Some(Canary::create(config, namespace))),
            EdgionPlugin::JwtAuth(config) => {
                CreateResult::Ok(Some(Box::new(JwtAuth::new(config, namespace.to_string()))))
            }
            EdgionPlugin::JweDecrypt(config) => {
                CreateResult::Ok(Some(Box::new(JweDecrypt::new(config, namespace.to_string()))))
            }
            EdgionPlugin::JsonSchemaValidation(config) => CreateResult::Ok(JsonSchemaValidation::create(config)),
            EdgionPlugin::FormJsonTransform(config) => {
                CreateResult::Ok(FormJsonTransform::create(config, body_requirement))
            }
            EdgionPlugin::HmacAuth(config) => CreateResult::Ok(Some(HmacAuth::create(config, body_requirement))),
            EdgionPlugin::HeaderCertAuth(config) => {
                CreateResult::Ok(Some(Box::new(HeaderCertAuth::new(config, namespace.to_string()))))
            }
            EdgionPlugin::KeyAuth(config) => CreateResult::Ok(Some(KeyAuth::create(config))),
            EdgionPlugin::LdapAuth(config) => CreateResult::Ok(LdapAuth::create(config)),
            EdgionPlugin::FaultInjection(config) => CreateResult::Ok(Some(Box::new(FaultInjection::new(config)))),
            EdgionPlugin::Mock(config) => CreateResult::Ok(Some(Box::new(Mock::new(config)))),
            EdgionPlugin::ProxyRewrite(config) => {
                CreateResult::Ok(ProxyRewrite::create(config, body_requirement).map(|plugin| Box::new(plugin) as _))
            }
            EdgionPlugin::RequestBodyBuffer(_) => CreateResult::Ok(Some(RequestBodyBuffer::create())),
            EdgionPlugin::RequestRestriction(config) => {
                CreateResult::Ok(Some(RequestRestriction::create_with_body(config, body_requirement)))
            }
            EdgionPlugin::RateLimitLocal(config) => CreateResult::Ok(Some(RateLimitLocal::create(config))),
            EdgionPlugin::RateLimitRedis(config) => CreateResult::Ok(Some(RateLimitRedis::create(config, entry_scope))),
            EdgionPlugin::CtxSet(config) => CreateResult::Ok(Some(CtxSet::create(config))),
            EdgionPlugin::WebhookKeyGet(config) => CreateResult::Ok(Some(WebhookKeyGet::create(config))),
            EdgionPlugin::DirectEndpoint(config) => CreateResult::Ok(Some(Box::new(DirectEndpoint::new(config)))),
            EdgionPlugin::DynamicInternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicInternalUpstream::create(config)))
            }
            EdgionPlugin::DynamicExternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicExternalUpstream::create(config)))
            }
            EdgionPlugin::RealIp(config) => CreateResult::Ok(Some(RealIp::create(config))),
            EdgionPlugin::RequestId(config) => CreateResult::Ok(RequestId::create(config)),
            EdgionPlugin::ExtProc(_) => CreateResult::WrongStage,
            EdgionPlugin::Guardrail(_) => CreateResult::WrongStage,
            EdgionPlugin::ForwardAuth(config) => match ForwardAuth::new(config) {
                Ok(fa) => CreateResult::Ok(Some(Box::new(fa))),
                Err(_) => CreateResult::Ok(None),
            },
            EdgionPlugin::AllEndpointStatus(config) => CreateResult::Ok(Some(Box::new(AllEndpointStatus::new(config)))),
            EdgionPlugin::OpenidConnect(config) => match OpenidConnect::try_new(config, namespace.to_string()) {
                Ok(plugin) => CreateResult::Ok(Some(Box::new(plugin))),
                Err(_) => CreateResult::Ok(None),
            },
            EdgionPlugin::Dsl(config) => CreateResult::Ok(DslPlugin::from_config(config)),
            EdgionPlugin::RegionRoute(config) => CreateResult::Ok(Some(RegionRoute::create(config, namespace))),
            EdgionPlugin::Wasm(config) => CreateResult::Ok(WasmPlugin::create_request(config)),
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
        match plugin {
            EdgionPlugin::ResponseHeaderModifier(config) => CreateResult::Ok(
                ResponseHeaderModifierFilter::new(config.clone())
                    .ok()
                    .map(|filter| Box::new(filter) as Box<dyn UpstreamResponseFilter>),
            ),
            EdgionPlugin::DebugAccessLogToHeader(config) => {
                CreateResult::Ok(Some(Box::new(DebugAccessLogToHeaderFilter::new(config))))
            }
            EdgionPlugin::ResponseRewrite(config) => CreateResult::Ok(Some(Box::new(ResponseRewrite::new(config)))),
            EdgionPlugin::Dsl(config) => CreateResult::Ok(DslPlugin::from_config_response(config)),
            EdgionPlugin::Wasm(config) => CreateResult::Ok(WasmPlugin::create_response_filter(config)),
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
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::LdapAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestBodyBuffer(_)
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
        match plugin {
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
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::Dsl(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::RequestId(_)
            | EdgionPlugin::RequestAccessPolicy(_)
            | EdgionPlugin::Canary(_)
            | EdgionPlugin::AiGuard(_)
            | EdgionPlugin::GeoIpLocation(_)
            | EdgionPlugin::Wasm(_) => CreateResult::WrongStage,
        }
    }

    /// Create an UpstreamResponseBodyFilter instance from EdgionPlugin enum
    fn create_upstream_response_body_filter_from_edgion(
        plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> CreateResult<Box<dyn UpstreamResponseBodyFilter>> {
        match plugin {
            EdgionPlugin::BandwidthLimit(config) => CreateResult::Ok(Some(BandwidthLimit::create(config))),
            EdgionPlugin::Wasm(config) => CreateResult::Ok(WasmPlugin::create_body_filter(config)),
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
            | EdgionPlugin::JsonSchemaValidation(_)
            | EdgionPlugin::FormJsonTransform(_)
            | EdgionPlugin::HmacAuth(_)
            | EdgionPlugin::HeaderCertAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::LdapAuth(_)
            | EdgionPlugin::Mock(_)
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestBodyBuffer(_)
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
            EdgionPlugin::FormJsonTransform(_) => "FormJsonTransform",
            EdgionPlugin::HmacAuth(_) => "HmacAuth",
            EdgionPlugin::HeaderCertAuth(_) => "HeaderCertAuth",
            EdgionPlugin::KeyAuth(_) => "KeyAuth",
            EdgionPlugin::LdapAuth(_) => "LdapAuth",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::ProxyRewrite(_) => "ProxyRewrite",
            EdgionPlugin::RequestBodyBuffer(_) => "RequestBodyBuffer",
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
    ) -> Option<Arc<super::dye_plan::CompiledDyePlan>> {
        let suite = dye?;
        match super::dye_plan::CompiledDyePlan::compile(suite, plugin.dye_type_prefix()) {
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
        self.request_plugins.push(Arc::from(filter));
    }

    fn add_upstream_response_filter(&mut self, filter: Box<dyn UpstreamResponseFilter>) {
        self.upstream_response_plugins.push(Arc::from(filter));
    }

    fn add_upstream_response_body_filter(&mut self, filter: Box<dyn UpstreamResponseBodyFilter>) {
        self.upstream_response_body_plugins.push(Arc::from(filter));
    }

    fn add_upstream_response(&mut self, filter: Box<dyn UpstreamResponse>) {
        self.upstream_response_async_plugins.push(Arc::from(filter));
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

    /// Run request stage filters (async)
    pub async fn run_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(s, ctx, false, RequestIdPhase::Normal)
            .await;
    }

    pub(crate) fn rate_limit_local_outcome_slots(&self) -> usize {
        self.rate_limit_local_outcome_slots
    }

    /// Enforce a sealed policy or invalid-config gate before preflight or body work.
    /// The normal runner owns the response, access-log record and metric.
    pub(crate) async fn run_preflight_gate(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        if !ctx.plugin_terminated()
            && self
                .request_plugins
                .first()
                .is_some_and(|filter| matches!(filter.name(), "PluginPolicyBlock" | "RejectInvalidConfig"))
        {
            self.run_request_plugins(s, ctx).await;
        }
    }

    /// Dispatch the selected HTTP backend's filters before its body pump starts.
    /// Only a first-attempt read-only mirror tee may be installed after the
    /// request-body policy freeze; capture and mutation remain unavailable.
    pub(crate) async fn run_backend_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(s, ctx, true, RequestIdPhase::Normal)
            .await;
    }

    pub(crate) async fn run_early_request_ids(&self, s: &mut Session, ctx: &mut EdgionHttpContext) -> bool {
        if self.invalid_early_request_id || (self.has_request_id && !self.invalid_plugins.is_empty()) {
            return false;
        }
        self.run_request_plugins_impl(s, ctx, false, RequestIdPhase::Early)
            .await;
        !ctx.plugin_terminated()
    }

    pub(crate) async fn run_request_plugins_after_early_ids(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        self.run_request_plugins_impl(s, ctx, false, RequestIdPhase::AfterEarly)
            .await;
    }

    async fn run_request_plugins_impl(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        backend_request: bool,
        phase: RequestIdPhase,
    ) {
        if self.request_plugins.is_empty() {
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
            PingoraSessionAdapter::new(s, ctx)
        };

        for filter in &self.request_plugins {
            let is_request_id = filter.name() == "RequestId";
            if (phase == RequestIdPhase::Early && !is_request_id)
                || (phase == RequestIdPhase::AfterEarly && is_request_id)
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
                    filter_logs.reserve(self.request_plugins.len());
                }
                filter_logs.push(plugin_log);
            }

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                break;
            } else if let PluginRunningResult::ErrResponse { status, body } = result {
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

                        let _ = session_adapter
                            .write_response_header(Box::new(resp), body.is_none())
                            .await;
                        if let Some(b) = body {
                            let _ = session_adapter.write_response_body(Some(b), true).await;
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
        if self.upstream_response_plugins.is_empty() {
            return;
        }

        let mut filter_logs = Vec::new();
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

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

            let should_terminate = check_response_stage_terminate(&result, &mut plugin_log, &mut session_adapter);
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
        if self.upstream_response_body_plugins.is_empty() {
            return None;
        }

        let mut max_delay: Option<Duration> = None;
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

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
        }

        max_delay
    }

    /// Run response_filter stage filters (async)
    pub async fn run_upstream_response_plugins_async(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        if self.upstream_response_async_plugins.is_empty() {
            return;
        }

        let mut filter_logs = Vec::new();
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

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

            let should_terminate = check_response_stage_terminate(&result, &mut plugin_log, &mut session_adapter);
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
    result: &PluginRunningResult,
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
            plugin_log.push("deny terminate; ");
            adapter.set_response_terminate_without_reply();
            true
        }
        PluginRunningResult::ErrResponse { status, body } => {
            if adapter.apply_response_local_reply(*status, body.clone()) {
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
                adapter.set_response_denied(*status);
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
            adapter.set_response_denied_grpc(*grpc_status, message.clone());
            true
        }
        PluginRunningResult::Nothing | PluginRunningResult::GoodNext => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::RequestDiagnostic;
    use async_trait::async_trait;
    use bytes::Bytes;
    use edgion_resources::resources::edgion_plugins::DslConfig;
    use edgion_resources::resources::{EdgionPlugin, KeyAuthConfig, RequestFilterEntry};
    use edgion_resources::EdgionErrorCode;
    use std::sync::Mutex;
    use tokio::io::AsyncWriteExt;

    fn compile_scoped_request_entries(
        entries: Vec<RequestFilterEntry>,
        deny: serde_json::Value,
    ) -> Arc<crate::plugins::http::CompiledPlugins> {
        use crate::plugins::http::plugin_generation::PluginGeneration;
        use edgion_resources::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec};
        use edgion_resources::resources::plugin_policy::PluginPolicyConfig;
        let policy: PluginPolicyConfig = serde_json::from_value(serde_json::json!({"deny": deny})).unwrap();
        PluginGeneration::new(EdgionPlugins::new(
            "scoped-test",
            EdgionPluginsSpec {
                request_plugins: Some(entries),
                ..Default::default()
            },
        ))
        .compile(Some(&policy.compile().unwrap()))
    }

    async fn session_from_request(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client.write_all(raw_request).await.unwrap();
        drop(client);
        assert!(session.read_request().await.unwrap());
        session
    }

    #[test]
    fn elapsed_micros_saturates_a_future_start_to_zero() {
        let started = Instant::now()
            .checked_add(Duration::from_secs(60))
            .expect("adding one minute to the current instant must remain representable");

        assert_eq!(elapsed_micros(started), 0);
    }

    struct FixedResultFilter(PluginRunningResult);

    #[async_trait]
    impl RequestFilter for FixedResultFilter {
        fn name(&self) -> &str {
            "FixedResult"
        }

        async fn run_request(
            &self,
            _session: &mut dyn PluginSession,
            _plugin_log: &mut PluginLog,
        ) -> PluginRunningResult {
            self.0.clone()
        }
    }

    struct SelfWritingFilter {
        client: Mutex<Option<tokio::io::DuplexStream>>,
    }

    #[async_trait]
    impl RequestFilter for SelfWritingFilter {
        fn name(&self) -> &str {
            "SelfWriting"
        }

        async fn run_request(
            &self,
            session: &mut dyn PluginSession,
            _plugin_log: &mut PluginLog,
        ) -> PluginRunningResult {
            let mut response = ResponseHeader::build(401, None).unwrap();
            response.insert_header("content-length", "6").unwrap();
            session
                .write_response_header(Box::new(response), false)
                .await
                .expect("header should be delivered before the test closes the client");
            drop(self.client.lock().unwrap().take());
            let _ = session
                .write_response_body(Some(bytes::Bytes::from_static(b"denied")), true)
                .await;
            PluginRunningResult::ErrTerminateRequest
        }
    }

    async fn plugin_test_session() -> (Session, tokio::io::DuplexStream) {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"GET /plugin-test HTTP/1.1\r\nHost: test\r\n\r\n")
            .await
            .unwrap();
        session.read_request().await.unwrap();
        (session, client)
    }

    fn client_closed_count(ctx: &EdgionHttpContext) -> usize {
        ctx.diagnostics
            .entries()
            .iter()
            .filter(|entry| **entry == RequestDiagnostic::Error(EdgionErrorCode::ClientClosed))
            .count()
    }

    #[tokio::test]
    async fn err_response_writer_failures_record_client_closed_once() {
        let (mut session, client) = plugin_test_session().await;
        drop(client);
        let mut ctx = EdgionHttpContext::default();
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"denied")),
        })));

        runtime.run_request_plugins(&mut session, &mut ctx).await;

        assert_eq!(ctx.final_downstream_status(), Some(403));
        assert_eq!(client_closed_count(&ctx), 1);
    }

    /// Read whatever the gateway wrote to the duplex client end. The gRPC
    /// trailers-only reply is a single HTTP/1 header block, so the raw bytes
    /// are enough to assert what reached the wire.
    async fn read_client_response(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(1), client.read(&mut buf))
            .await
            .expect("gateway wrote a response")
            .expect("read succeeds");
        buf.truncate(n);
        buf
    }

    /// A UTF-8 `ErrResponse` body on a gRPC route is carried verbatim as
    /// `grpc-message`, exactly as before the body became raw `Bytes`.
    #[tokio::test]
    async fn err_response_on_grpc_route_carries_utf8_body_as_message() {
        let (mut session, mut client) = plugin_test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"tenant quota exhausted")),
        })));

        runtime.run_request_plugins(&mut session, &mut ctx).await;
        let raw = String::from_utf8_lossy(&read_client_response(&mut client).await).into_owned();

        assert_eq!(ctx.request_info.grpc_status(), Some(http_to_grpc_status(403).0));
        assert!(
            raw.contains("grpc-message: tenant quota exhausted"),
            "plugin body must reach grpc-message verbatim; got: {raw}"
        );
        assert!(
            !ctx.diagnostics
                .entries()
                .iter()
                .any(|e| *e == RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8)),
            "a valid UTF-8 body must not raise the not-utf8 marker"
        );
    }

    /// A non-UTF-8 body cannot be a `grpc-message` header value. It must fall
    /// back to the status-mapped default rather than emitting raw bytes or a
    /// U+FFFD-mangled decode, and the drop must be visible in the access log.
    #[tokio::test]
    async fn err_response_on_grpc_route_replaces_non_utf8_body_and_marks_it() {
        let (mut session, mut client) = plugin_test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(&[0x89, 0x50, 0x4e, 0x47, 0xff, 0x00])),
        })));

        runtime.run_request_plugins(&mut session, &mut ctx).await;
        let bytes = read_client_response(&mut client).await;
        let raw = String::from_utf8_lossy(&bytes).into_owned();

        let (expected_status, default_msg) = http_to_grpc_status(403);
        assert_eq!(ctx.request_info.grpc_status(), Some(expected_status));
        assert!(
            raw.contains(&format!("grpc-message: {default_msg}")),
            "non-utf8 body must fall back to the mapped default; got: {raw}"
        );
        // Neither the raw bytes nor a lossy decode may reach the wire.
        assert!(
            !bytes.windows(4).any(|w| w == [0x89, 0x50, 0x4e, 0x47]),
            "raw body bytes must not reach the trailer"
        );
        assert!(!raw.contains('\u{FFFD}'), "no lossy replacement characters on the wire");
        assert_eq!(
            ctx.diagnostics
                .entries()
                .iter()
                .filter(|e| **e == RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8))
                .count(),
            1,
            "the dropped body must be recorded exactly once"
        );
    }

    #[tokio::test]
    async fn err_grpc_error_writer_failure_preserves_both_statuses() {
        let (mut session, client) = plugin_test_session().await;
        drop(client);
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: Some("denied".to_string()),
        })));

        runtime.run_request_plugins(&mut session, &mut ctx).await;

        assert_eq!(ctx.final_downstream_status(), Some(200));
        assert_eq!(ctx.request_info.grpc_status(), Some(7));
        assert_eq!(client_closed_count(&ctx), 1);
    }

    #[tokio::test]
    async fn self_written_writer_failure_records_client_closed() {
        let (mut session, client) = plugin_test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(SelfWritingFilter {
            client: Mutex::new(Some(client)),
        }));

        runtime.run_request_plugins(&mut session, &mut ctx).await;

        assert_eq!(ctx.final_downstream_status(), Some(401));
        assert_eq!(client_closed_count(&ctx), 1);
    }

    #[tokio::test]
    async fn waf_request_runner_emits_only_the_dedicated_summary() {
        let (mut session, _client) = plugin_test_session().await;
        let mut runtime = PluginRuntime::new();
        runtime
            .add_waf(&WafConfig {
                rules: "SecAction \"id:1,phase:1,pass,log\"\n".to_string(),
                mode: edgion_resources::resources::edgion_plugins::WafMode::On,
                priority: 12,
            })
            .unwrap();
        let candidate = runtime.waf_candidate().unwrap();
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.select_waf(Some(crate::ctx::WafSelection::new(
            candidate.owner(),
            crate::ctx::WafLogSource::Route,
            candidate.priority(),
            candidate.mode(),
        ))));

        runtime.run_request_plugins(&mut session, &mut ctx).await;

        assert!(ctx.stage_logs().is_empty());
        let waf = serde_json::to_value(ctx.waf_access_log().unwrap()).unwrap();
        assert_eq!(waf["source"], "route");
        assert_eq!(waf["priority"], 12);
        assert_eq!(waf["phases"]["request_headers"]["findings"], 1);
        assert_eq!(waf["findings"][0]["rule_id"], 1);
    }

    #[test]
    fn invalid_update_policy_uses_plugin_variants_and_strictest_policy_wins() {
        let dsl = EdgionPlugin::Dsl(DslConfig::default());
        let key_auth = EdgionPlugin::KeyAuth(KeyAuthConfig::default());
        let mut invalid = InvalidPlugins::default();

        invalid.record(&dsl);
        invalid.record(&dsl);
        assert_eq!(invalid.update_policy(), Some(InvalidUpdatePolicy::RetainPrevious));

        invalid.record(&key_auth);
        invalid.record(&dsl);
        assert_eq!(invalid.update_policy(), Some(InvalidUpdatePolicy::ReplaceFailClosed));
    }

    fn json_schema_entry(
        body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
        enabled: bool,
    ) -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
        use serde_json::json;

        let mut entry = RequestFilterEntry::with_enable(
            EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
                schema: json!({"type": "object"}),
            }),
            enabled,
        );
        entry.body = body;
        entry
    }

    fn form_json_entry(
        direction: edgion_resources::resources::edgion_plugins::FormJsonDirection,
        body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
    ) -> RequestFilterEntry {
        use edgion_resources::resources::edgion_plugins::FormJsonTransformConfig;

        let mut entry = RequestFilterEntry::new(EdgionPlugin::FormJsonTransform(FormJsonTransformConfig { direction }));
        entry.body = body;
        entry
    }

    fn assert_sealed_invalid(runtime: &mut PluginRuntime) {
        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_iter().next().map(|filter| filter.name()),
            Some("RejectInvalidConfig")
        );
    }

    #[test]
    fn entry_scope_distinguishes_entries_and_resources() {
        // Two entries of one resource, and the same entry position in another
        // resource, must never share a scope.
        assert_eq!(
            crate::plugins::common::entry_scope("default/api-limiter", None, 0),
            "default/api-limiter/#0"
        );
        assert_ne!(
            crate::plugins::common::entry_scope("default/api-limiter", None, 0),
            crate::plugins::common::entry_scope("default/api-limiter", None, 1)
        );
        assert_ne!(
            crate::plugins::common::entry_scope("default/api-limiter", None, 0),
            crate::plugins::common::entry_scope("other/api-limiter", None, 0)
        );
        // An alias pins the scope across entry reordering, and cannot collide
        // with the index form of another entry.
        assert_eq!(
            crate::plugins::common::entry_scope("default/api-limiter", Some("tenant"), 3),
            "default/api-limiter/tenant"
        );
        assert_ne!(
            crate::plugins::common::entry_scope("default/api-limiter", Some("1"), 0),
            crate::plugins::common::entry_scope("default/api-limiter", None, 1)
        );
    }

    #[tokio::test]
    async fn add_from_request_filters_defers_hmac_body_capture() {
        // HMAC owns its resolved body requirement instead of using the generic
        // eager gate. A malformed Authorization header must be rejected before
        // any request-body capture is attempted.
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
        use edgion_resources::resources::HmacAuthConfig;

        let cfg = HmacAuthConfig {
            match_config: edgion_resources::resources::HmacMatchConfig {
                validate_request_body: true,
                ..Default::default()
            },
            anonymous: Some("anon".to_string()),
            ..Default::default()
        };
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(
            &[RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg))],
            "default",
            "test-plugins",
        );
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert_eq!(filter.name(), "HmacAuth", "gate must be transparent to name()");

        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("POST".to_string());
        setup_header_mocks(&mut session, &[("authorization", "malformed")]);
        session.expect_ensure_request_body().times(0);
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("HmacAuth");
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrTerminateRequest,
            "HMAC must reject malformed authentication before capturing the body"
        );
    }

    #[tokio::test]
    async fn json_schema_validation_gate_captures_without_materializing() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::EdgionHttpContext;
        use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
        use serde_json::json;

        let entry = RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: json!({"type": "object"}),
        }));
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert_eq!(filter.name(), "JsonSchemaValidation");

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(false));
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
            .return_once(|_| Ok(0));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(true))
            .return_const(Ok(0));

        let mut log = PluginLog::new("JsonSchemaValidation");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some(Bytes::from_static(b"Invalid request body")),
            }
        );
    }

    #[test]
    fn form_json_runtime_builds_and_rejects_fail_open_without_controller() {
        use edgion_resources::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement, FormJsonDirection};

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(
            &[form_json_entry(FormJsonDirection::FormToJson, None)],
            "default",
            "test-plugins",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            runtime.request_plugins_iter().next().map(|filter| filter.name()),
            Some("FormJsonTransform")
        );

        let body = BodyRequirement {
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..BodyRequirement::default()
        };
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(
            &[form_json_entry(FormJsonDirection::JsonToForm, Some(body))],
            "default",
            "test-plugins",
        );
        assert_eq!(
            errors,
            vec!["requestPlugins[0] (FormJsonTransform): body.onReadFailure must be failClose"]
        );
        assert_eq!(runtime.request_plugins_count(), 0);
        assert_sealed_invalid(&mut runtime);
    }

    #[tokio::test]
    async fn form_json_stock_gate_fails_before_header_or_body_mutation() {
        use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
        use crate::EdgionHttpContext;
        use edgion_resources::resources::edgion_plugins::FormJsonDirection;

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(
            &[form_json_entry(FormJsonDirection::FormToJson, None)],
            "default",
            "test-plugins",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
            .return_once(|_| Err(BodyReadError::Unimplemented));
        session
            .expect_record_error()
            .with(mockall::predicate::eq(
                edgion_resources::EdgionErrorCode::BodyReadUnimplemented,
            ))
            .return_const(());
        session.expect_request_header_values().times(0);
        session.expect_read_request_body().times(0);
        session.expect_overwrite_request_body().times(0);
        session.expect_set_request_header().times(0);
        let mut log = PluginLog::new(filter.name());

        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            }
        );
    }

    #[test]
    fn json_schema_invalid_body_rules_seal_fail_closed() {
        use edgion_resources::resources::edgion_plugins::BodyRequirement;

        let cases = [
            (
                BodyRequirement {
                    max_body_size: Some("invalid".to_string()),
                    ..BodyRequirement::default()
                },
                "invalid body.maxBodySize",
            ),
            (
                BodyRequirement {
                    max_body_size: Some("0".to_string()),
                    ..BodyRequirement::default()
                },
                "must be greater than 0",
            ),
            (
                BodyRequirement {
                    max_body_size: Some((1024 * 1024 + 1).to_string()),
                    ..BodyRequirement::default()
                },
                "must not exceed 1MiB",
            ),
            (
                BodyRequirement {
                    conditions: Some(Default::default()),
                    ..BodyRequirement::default()
                },
                "conditions are not supported",
            ),
        ];

        for (body, expected_error) in cases {
            let expected_errors: Vec<String> =
                edgion_resources::resources::edgion_plugins::bounded_structured_body_validation_errors(&body)
                    .into_iter()
                    .map(|error| format!("requestPlugins[0] (JsonSchemaValidation): {error}"))
                    .collect();
            let mut runtime = PluginRuntime::new();
            let errors =
                runtime.add_from_request_filters(&[json_schema_entry(Some(body), true)], "default", "test-plugins");
            assert_eq!(errors, expected_errors, "Gateway must mirror canonical body errors");
            assert!(
                errors.iter().any(|error| error.contains(expected_error)),
                "expected {expected_error:?}, got {errors:?}"
            );
            assert_eq!(runtime.request_plugins_count(), 0);
            assert_sealed_invalid(&mut runtime);
        }
    }

    #[test]
    fn proxy_rewrite_bounded_body_validation_matches_consumption() {
        use edgion_resources::resources::edgion_plugins::{
            BodyRequirement, JsonBodyRewrite, JsonPatchOperation, ProxyRewriteConfig,
        };

        let invalid_body = BodyRequirement {
            conditions: Some(Default::default()),
            ..BodyRequirement::default()
        };
        let mut consuming = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![JsonPatchOperation::Remove {
                    path: "/obsolete".to_string(),
                }],
            }),
            ..Default::default()
        }));
        consuming.body = Some(invalid_body.clone());

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[consuming], "default", "test-plugins");
        assert_eq!(
            errors,
            vec!["requestPlugins[0] (ProxyRewrite): body: conditions are not supported"]
        );
        assert_eq!(runtime.request_plugins_count(), 0);
        assert_sealed_invalid(&mut runtime);

        let mut uri_only = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
            uri: Some("/rewritten".to_string()),
            ..Default::default()
        }));
        uri_only.body = Some(invalid_body);
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[uri_only], "default", "test-plugins");
        assert_eq!(
            errors,
            vec!["requestPlugins[0] (ProxyRewrite): body: plugin does not consume a body"]
        );
        assert!(runtime.invalid_plugins.is_empty());
        assert_eq!(runtime.request_plugins_count(), 1);
    }

    #[test]
    fn multiple_enabled_json_schema_validators_seal_fail_closed() {
        let entries = [json_schema_entry(None, true), json_schema_entry(None, true)];
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");
        assert_eq!(
            errors,
            vec!["requestPlugins: at most one JsonSchemaValidation plugin is allowed, found 2"]
        );
        assert_sealed_invalid(&mut runtime);
    }

    #[test]
    fn disabled_json_schema_validator_is_inert_for_cardinality() {
        let entries = [json_schema_entry(None, true), json_schema_entry(None, false)];
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        runtime.seal_fail_closed();
        assert_eq!(runtime.request_plugins_count(), 1);
        assert_eq!(
            runtime.request_plugins_iter().next().map(|filter| filter.name()),
            Some("JsonSchemaValidation")
        );
    }

    #[test]
    fn bypassed_json_schema_is_inert_for_body_validation_and_cardinality() {
        use edgion_resources::resources::edgion_plugins::BodyRequirement;
        let malformed = json_schema_entry(
            Some(BodyRequirement {
                max_body_size: Some("invalid".into()),
                ..Default::default()
            }),
            true,
        );
        let compiled = compile_scoped_request_entries(
            vec![malformed, json_schema_entry(None, true)],
            serde_json::json!([{"name":"http/JsonSchemaValidation","action":"bypass"}]),
        );
        assert!(compiled.preparse_errors.is_empty());
        assert!(compiled.is_activatable);
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
        assert_eq!(
            compiled.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "PluginPolicyBypass"
        );
    }

    #[test]
    fn blocked_json_schema_is_inert_for_body_validation_and_cardinality() {
        use edgion_resources::resources::edgion_plugins::BodyRequirement;
        let malformed = json_schema_entry(
            Some(BodyRequirement {
                conditions: Some(Default::default()),
                ..Default::default()
            }),
            true,
        );
        let compiled = compile_scoped_request_entries(
            vec![malformed, json_schema_entry(None, true)],
            serde_json::json!([{"name":"http/JsonSchemaValidation","blockStatus":451}]),
        );
        assert!(compiled.preparse_errors.is_empty());
        assert!(compiled.is_activatable);
        assert_eq!(
            compiled.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "PluginPolicyBlock"
        );
    }

    #[tokio::test]
    async fn json_schema_default_limit_accepts_exactly_64_kib() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::EdgionHttpContext;
        use bytes::Bytes;

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[json_schema_entry(None, true)], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

        let mut body = vec![b' '; 64 * 1024];
        body[0] = b'{';
        body[1] = b'}';
        let body = Bytes::from(body);
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_claim_json_schema_validation().return_const(true);
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
            .return_const(Ok(64 * 1024));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .return_const(Ok(64 * 1024));
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        session.expect_request_header_map().times(1).return_const(headers);
        session
            .expect_read_request_body()
            .with(mockall::predicate::eq(None))
            .return_once(move |_| Ok(body));

        let mut log = PluginLog::new("JsonSchemaValidation");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::Nothing
        );
    }

    #[tokio::test]
    async fn json_schema_default_limit_rejects_64_kib_plus_one_before_materialization() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::EdgionHttpContext;

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[json_schema_entry(None, true)], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
            .return_const(Ok(64 * 1024 + 1));
        session
            .expect_record_error()
            .with(mockall::predicate::eq(
                edgion_resources::EdgionErrorCode::BodyReadTooLarge,
            ))
            .return_once(|_| ());

        let mut log = PluginLog::new("JsonSchemaValidation");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None,
            }
        );
    }

    #[tokio::test]
    async fn request_body_buffer_fails_closed_when_capture_is_unimplemented() {
        use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
        use crate::EdgionHttpContext;

        let entry = RequestFilterEntry::new(EdgionPlugin::RequestBodyBuffer(Default::default()));
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert_eq!(filter.name(), "RequestBodyBuffer");

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_ensure_request_body()
            .withf(|max_bytes| *max_bytes == Some(32 * 1024 * 1024))
            .returning(|_| Err(BodyReadError::Unimplemented));
        session
            .expect_record_error()
            .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyReadUnimplemented)
            .times(1)
            .returning(|_| ());

        let mut log = PluginLog::new("RequestBodyBuffer");
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn add_from_request_filters_wraps_wasm_with_operator_body() {
        // Wasm has no INTRINSIC body_requirement() (the guest's hooks are only
        // known after module load), so it opts into request buffer mode purely
        // through an operator `body:` block on the entry. Proof, mirroring the
        // HMAC case above: with `body:` set, the built filter must force-read
        // the body (the mock returns Unimplemented) and fail-close 500 BEFORE ever
        // reaching WasmPlugin's own logic (which would otherwise try to load
        // the module and fail differently). If the gate were not wired for
        // Wasm, `read_request_body` would never be called by the gate at all
        // and this assertion would not observe a 500 from the gate.
        use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
        use crate::EdgionHttpContext;
        use edgion_resources::resources::edgion_plugins::plugin_configs::WasmConfig;
        use edgion_resources::resources::edgion_plugins::BodyRequirement;

        let mut cfg = WasmConfig::default();
        cfg.source.file = Some("plugin.wasm".to_string());
        let mut entry = RequestFilterEntry::new(EdgionPlugin::Wasm(cfg));
        entry.body = Some(BodyRequirement::fail_close());

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(
            errors.is_empty(),
            "operator body: on a Wasm entry must be accepted at admission, got: {errors:?}"
        );

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert_eq!(filter.name(), "Wasm", "gate must be transparent to name()");

        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("POST".to_string());
        session.expect_header_value().returning(|_| None);
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_record_error().returning(|_| ());
        session
            .expect_read_request_body()
            .returning(|_| Err(BodyReadError::Unimplemented));

        let mut log = PluginLog::new("Wasm");
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            },
            "operator body: on Wasm must actually wrap the plugin in the body gate"
        );
    }

    #[test]
    fn add_from_request_filters_drops_a_duplicate_alias() {
        let mut runtime = PluginRuntime::new();

        let mut first = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        first.alias = Some("dup".to_string());
        let mut second = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        second.alias = Some("dup".to_string());

        let entries = vec![first, second];
        let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

        assert!(
            errors
                .iter()
                .any(|e| e.contains("requestPlugins[1]") && e.contains("duplicate alias 'dup'")),
            "expected a duplicate-alias error for index 1, got: {:?}",
            errors
        );

        // Reporting alone is not enough: lenient admission keeps an
        // `Accepted=False` resource serving, so the duplicate must also be
        // dropped. The second entry then falls back to its `#index` scope
        // instead of sharing the first entry's bucket namespace.
        let mut iter = runtime.request_plugins_iter();
        assert_eq!(iter.next().expect("first wrapper").alias(), Some("dup"));
        assert_eq!(
            iter.next().expect("second wrapper").alias(),
            None,
            "duplicate alias must be dropped, like an invalid one"
        );
    }

    #[test]
    fn add_from_request_filters_reports_alias_validation_error_and_threads_alias() {
        let mut runtime = PluginRuntime::new();

        let mut bad_entry = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        bad_entry.alias = Some("with space".to_string());

        let mut good_entry = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        good_entry.alias = Some("auth-internal".to_string());

        let entries = vec![bad_entry, good_entry];
        let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

        // The bad alias surfaces as a validation error referencing the index and the alias rule.
        assert!(
            errors
                .iter()
                .any(|e| e.contains("requestPlugins[0]") && e.contains("alias")),
            "expected alias error for index 0, got: {:?}",
            errors
        );

        // Both entries are still wrapped (we surface errors but don't drop the entry).
        let mut iter = runtime.request_plugins_iter();
        let first = iter.next().expect("first wrapper");
        // Bad alias is dropped on validation failure so an unvalidated value never reaches the access log.
        assert_eq!(first.alias(), None, "bad alias must be dropped (validation refused)");
        let second = iter.next().expect("second wrapper");
        assert_eq!(second.alias(), Some("auth-internal"));
    }

    /// Minimal valid `CanaryConfig` (all fields operator-required, no `Default`).
    /// Canary is a request-stage plugin whose `dye_capabilities()` include
    /// `Success`, so it can carry a request-direction dye rule.
    fn canary_config_for_dye_test() -> edgion_resources::resources::edgion_plugins::plugin_configs::CanaryConfig {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };
        use std::collections::HashMap;
        CanaryConfig {
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
        }
    }

    #[test]
    fn add_from_request_filters_compiles_and_threads_dye_plan() {
        use edgion_resources::resources::edgion_plugins::{DyeRule, DyeSuite};

        let mut entry = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config_for_dye_test()));
        entry.dye = Some(DyeSuite {
            request: vec![DyeRule {
                name: "X-Canary-Profile".to_string(),
                on: vec![edgion_resources::resources::edgion_plugins::DyeOutcome::Success],
                value: None,
                conditions: None,
            }],
            response: vec![],
        });

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        let plan = filter.dye_plan().expect("dye plan threaded through the wrapper");
        assert_eq!(plan.type_prefix, "canary");
        assert_eq!(plan.request.len(), 1);
        assert_eq!(plan.request[0].name, "X-Canary-Profile");
        assert!(plan.response.is_empty());
    }

    #[test]
    fn add_from_request_filters_has_no_dye_plan_when_entry_has_none() {
        let entry = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config_for_dye_test()));
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert!(filter.dye_plan().is_none());
    }

    #[test]
    fn csrf_without_resolved_key_fails_validation() {
        use edgion_resources::resources::edgion_plugins::{CsrfConfig, EdgionPlugin};
        // No secret_ref + no resolved_key → rejected.
        let plugin = EdgionPlugin::Csrf(CsrfConfig::default());
        assert!(PluginRuntime::plugin_validation_error(&plugin).is_some());
        // Empty resolved_key → still rejected (the gate is stronger than the shared check).
        let empty_key = EdgionPlugin::Csrf(CsrfConfig {
            resolved_key: Some(String::new()),
            ..Default::default()
        });
        assert!(PluginRuntime::plugin_validation_error(&empty_key).is_some());
        // Non-empty resolved_key → accepted.
        let resolved = EdgionPlugin::Csrf(CsrfConfig {
            resolved_key: Some("k".to_string()),
            ..Default::default()
        });
        assert!(PluginRuntime::plugin_validation_error(&resolved).is_none());
    }

    #[test]
    fn invalid_plugin_config_is_skipped_and_sealed_fail_closed() {
        use edgion_resources::resources::edgion_plugins::ForwardAuthConfig;

        let mut runtime = PluginRuntime::new();

        // Entry 0: invalid config (missing port fails ForwardAuthConfig validation).
        let invalid = RequestFilterEntry::new(EdgionPlugin::ForwardAuth(ForwardAuthConfig::default()));
        // Entry 1: valid config.
        let valid = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));

        let entries = vec![invalid, valid];
        let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

        // The invalid plugin is reported against its index.
        assert!(
            errors
                .iter()
                .any(|e| e.contains("requestPlugins[0]") && e.contains("ForwardAuth")),
            "expected ForwardAuth validation error for index 0, got: {:?}",
            errors
        );

        // The invalid plugin is NOT constructed: only the valid entry is in the chain.
        assert_eq!(
            runtime.request_plugins_count(),
            1,
            "invalid plugin must be skipped, not constructed"
        );

        // Sealing prepends a fail-closed RejectFilter so the route returns 500.
        runtime.seal_fail_closed();
        assert_eq!(runtime.request_plugins_count(), 2, "RejectFilter must be prepended");
        let first = runtime.request_plugins_iter().next().expect("reject filter");
        assert_eq!(first.name(), "RejectInvalidConfig");
    }

    #[test]
    fn seal_fail_closed_is_noop_when_all_plugins_valid() {
        let mut runtime = PluginRuntime::new();
        let entries = vec![RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()))];
        runtime.add_from_request_filters(&entries, "default", "test-plugins");

        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_count(),
            1,
            "no RejectFilter when every plugin config is valid"
        );
    }

    #[test]
    fn scoped_restrictions_do_not_instantiate_or_invalidate_entries() {
        use edgion_resources::resources::edgion_plugins::{CorsConfig, MockConfig};
        let compiled = compile_scoped_request_entries(
            vec![
                RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
                RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default())),
                RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default())),
            ],
            serde_json::json!([
                {"name":"http/KeyAuth","blockStatus":451},
                {"name":"http/Cors","action":"bypass"}
            ]),
        );
        assert!(compiled.preparse_errors.is_empty());
        assert!(compiled.is_activatable);
        let names: Vec<_> = compiled
            .plugin_runtime
            .request_plugins_iter()
            .map(|p| p.name())
            .collect();
        assert_eq!(names, vec!["PluginPolicyBlock", "PluginPolicyBypass", "Mock"]);
    }

    #[test]
    fn policy_block_gate_precedes_fail_closed_reject() {
        let compiled = compile_scoped_request_entries(
            vec![
                RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
                RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                    source: Some("invalid @@@".into()),
                    ..Default::default()
                })),
            ],
            serde_json::json!([{"name":"http/KeyAuth","blockStatus":451}]),
        );
        assert!(!compiled.is_activatable);
        let mut filters = compiled.plugin_runtime.request_plugins_iter();
        assert_eq!(filters.next().unwrap().name(), "PluginPolicyBlock");
        assert_eq!(filters.next().unwrap().name(), "RejectInvalidConfig");
    }

    #[tokio::test]
    async fn policy_bypass_note_cannot_hide_fail_closed_preflight_gate() {
        let compiled = compile_scoped_request_entries(
            vec![
                RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
                RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                    source: Some("invalid @@@".into()),
                    ..Default::default()
                })),
            ],
            serde_json::json!([{"name":"http/KeyAuth","action":"bypass"}]),
        );
        assert!(!compiled.is_activatable);
        let names: Vec<_> = compiled
            .plugin_runtime
            .request_plugins_iter()
            .map(|filter| filter.name())
            .collect();
        assert_eq!(names[0], "RejectInvalidConfig");
        assert_eq!(names[1], "PluginPolicyBypass");

        let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();
        compiled.plugin_runtime.run_preflight_gate(&mut session, &mut ctx).await;

        assert!(ctx.plugin_terminated());
    }

    #[test]
    fn unrestricted_entries_have_no_policy_filters() {
        use edgion_resources::resources::edgion_plugins::MockConfig;

        let normal = RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default()));
        let mut runtime = PluginRuntime::new();
        let _ = runtime.add_from_request_filters(&[normal], "default", "test-plugins");
        runtime.seal_fail_closed();
        runtime.seal_plugin_policy();
        assert_eq!(runtime.request_plugins.len(), 1); // just Mock, zero overhead
    }

    #[test]
    fn invalid_response_stage_plugin_seals_request_stage_fail_closed() {
        use edgion_resources::resources::edgion_plugins::{BandwidthLimitConfig, UpstreamResponseBodyFilterEntry};

        let mut runtime = PluginRuntime::new();

        // BandwidthLimit is a body-stage plugin; an empty rate fails its validation.
        let invalid = UpstreamResponseBodyFilterEntry::new(EdgionPlugin::BandwidthLimit(BandwidthLimitConfig {
            rate: String::new(),
            rate_bytes_per_second: None,
        }));
        let errors = runtime.add_from_upstream_response_body_filters(&[invalid], "default");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("upstreamResponseBodyFilterPlugins[0]") && e.contains("BandwidthLimit")),
            "expected BandwidthLimit validation error, got: {:?}",
            errors
        );

        // The invalid body-stage plugin is skipped, and nothing is in the request stage yet.
        assert_eq!(runtime.upstream_response_body_plugins_count(), 0);
        assert_eq!(runtime.request_plugins_count(), 0);

        // Sealing puts the RejectFilter on the REQUEST stage even though the
        // invalid plugin was a body-stage plugin: the request stage runs before
        // upstream is contacted, so this is the only gate that guarantees a 500.
        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_count(),
            1,
            "request-stage RejectFilter must cover body-stage invalid configs"
        );
        let first = runtime.request_plugins_iter().next().expect("reject filter");
        assert_eq!(first.name(), "RejectInvalidConfig");
    }

    #[test]
    fn dsl_construction_failure_seals_fail_closed() {
        use edgion_resources::resources::edgion_plugins::DslConfig;

        let mut runtime = PluginRuntime::new();

        // A DSL config whose source fails to build.
        let bad_dsl = RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "bad".to_string(),
            source: Some("this is not valid dsl @@@".to_string()),
            bytecode: None,
            ..Default::default()
        }));

        runtime.add_from_request_filters(&[bad_dsl], "default", "test-plugins");

        // A DSL config that fails to build (whether caught by `validation_error()`
        // or by the `Ok(None)` constructor-declined arm) must leave the route
        // sealed fail-closed — assert the end state: a RejectFilter is prepended.
        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_count(),
            1,
            "DSL construction failure must seal fail-closed: a RejectFilter must be prepended"
        );
        let first = runtime.request_plugins_iter().next().expect("reject filter");
        assert_eq!(first.name(), "RejectInvalidConfig");
    }

    #[test]
    fn canary_plugin_construction() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };
        use std::collections::HashMap;

        // Create a minimal valid CanaryConfig (must have at least one profile).
        let mut profiles = HashMap::new();
        profiles.insert(
            "base".to_string(),
            DynUpstreamTarget {
                name: "backend1".to_string(),
                namespace: None,
            },
        );

        let config = CanaryConfig {
            enable: true,
            active_profile: "base".to_string(),
            profiles,
            active_profile_ref: None,
            on_invalid: UpstreamFallback::Reject,
        };

        let plugin = EdgionPlugin::Canary(config);

        // Verify request stage returns Ok(Some(...)).
        let result =
            PluginRuntime::create_request_filter_from_edgion(&plugin, "default", None, "default/test-plugins/#0");
        assert!(
            matches!(result, CreateResult::Ok(Some(_))),
            "Canary should construct in request stage"
        );

        // Verify upstream response stage returns WrongStage.
        let result_response = PluginRuntime::create_upstream_response_filter_from_edgion(&plugin, "default");
        assert!(
            matches!(result_response, CreateResult::WrongStage),
            "Canary should not apply to upstream response stage"
        );

        // Verify get_plugin_name works.
        assert_eq!(PluginRuntime::get_plugin_name(&plugin), "Canary");
    }

    #[test]
    fn ai_proxy_is_constructed_only_for_request_stage() {
        use edgion_resources::resources::edgion_plugins::{
            AiBackendSelection, AiBackendSelectionConfig, AiProxyConfig, OnUnknownModel,
        };

        let plugin = EdgionPlugin::AiProxy(AiProxyConfig {
            backend_selection: AiBackendSelectionConfig {
                selection_type: AiBackendSelection::Weighted,
            },
            inject_usage_tracking: true,
            on_unknown_model: OnUnknownModel::Reject,
            model_routes: Vec::new(),
            token_quota: None,
        });

        let request = PluginRuntime::create_request_filter_from_edgion(
            &plugin,
            "tenant-a",
            plugin.body_requirement(edgion_resources::resources::edgion_plugins::BodyStage::Request),
            "tenant-a/plugins/#0",
        );
        let CreateResult::Ok(Some(filter)) = request else {
            panic!("AiProxy must construct in request stage");
        };
        assert_eq!(filter.name(), "AiProxy");

        assert!(matches!(
            PluginRuntime::create_upstream_response_filter_from_edgion(&plugin, "tenant-a"),
            CreateResult::WrongStage
        ));
        assert!(matches!(
            PluginRuntime::create_upstream_response_body_filter_from_edgion(&plugin, "tenant-a"),
            CreateResult::WrongStage
        ));
        assert!(matches!(
            PluginRuntime::create_upstream_response_from_edgion(&plugin, "tenant-a"),
            CreateResult::WrongStage
        ));
        assert_eq!(PluginRuntime::get_plugin_name(&plugin), "AiProxy");
    }
}

#[cfg(test)]
mod condition_body_requirement_tests {
    use super::*;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::conditions::{
        Condition, ConditionSet, KeyExistCondition, PluginConditions,
    };
    use edgion_resources::resources::edgion_plugins::{
        BodyStage, HmacCredential, HmacMatchConfig, RequestRestrictionConfig, RestrictionMode,
    };
    use std::collections::HashMap;

    #[test]
    fn nested_request_restriction_body_hmac_declares_plugin_level_requirement() {
        let plugin = EdgionPlugin::RequestRestriction(RequestRestrictionConfig {
            mode: Some(RestrictionMode::Allow),
            conditions: ConditionSet::all_of(vec![Condition::HmacAuth(HmacMatchConfig {
                validate_request_body: true,
                resolved_credentials: Some(HashMap::from([(
                    "alice".to_string(),
                    HmacCredential {
                        secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                        headers: HashMap::new(),
                        group: "default".to_string(),
                    },
                )])),
                ..Default::default()
            })]),
            ..Default::default()
        });

        let requirement = PluginRuntime::resolve_body_requirement(&plugin, None, BodyStage::Request);
        assert!(requirement.is_some());
        assert!(PluginRuntime::use_generic_body_gate(&plugin));
    }

    #[test]
    fn invalid_deny_restriction_bypasses_body_gate_for_generic_500() {
        let unresolved = Condition::SecretMatch(
            edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition {
                key: KeyGet::Header {
                    name: "X-Secret".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: edgion_resources::resources::common::SecretObjectReference {
                    name: "missing".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: None,
                descriptor: Box::from(""),
            },
        );
        let body_hmac = Condition::HmacAuth(HmacMatchConfig {
            validate_request_body: true,
            resolved_credentials: Some(HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        });
        let plugin = EdgionPlugin::RequestRestriction(RequestRestrictionConfig {
            mode: Some(RestrictionMode::Deny),
            conditions: ConditionSet::all_of(vec![unresolved, body_hmac]),
            ..Default::default()
        });

        assert!(PluginRuntime::resolve_body_requirement(&plugin, None, BodyStage::Request).is_some());
        assert!(!PluginRuntime::use_generic_body_gate(&plugin));

        let mut entry = RequestFilterEntry::new(plugin);
        entry.conditions = Some(PluginConditions {
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Bypass".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
            run: None,
        });
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[entry], "default", "default/policy");
        assert!(errors.iter().any(|error| error.contains("restriction config invalid")));
        assert!(runtime.has_invalid_plugins());
        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_iter().next().map(|filter| filter.name()),
            Some("RejectInvalidConfig")
        );
    }
}

#[cfg(test)]
mod header_modifier_validation_tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use serde_json::json;

    async fn assert_rejected(runtime: &PluginRuntime) {
        assert!(runtime.rejects_invalid_config());
        let filter = runtime.request_plugins_iter().next().expect("rejection filter");
        assert_eq!(filter.name(), "RejectInvalidConfig");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(filter.name());
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some(bytes::Bytes::from_static(
                    br#"{"message":"plugin configuration error"}"#
                )),
            }
        );
        assert!(!log.render_for_test().contains("private-value"));
    }

    #[tokio::test]
    async fn header_modifier_invalid_native_lists_seal_all_four_constructors() {
        for filters in [
            json!([{"type": "RequestHeaderModifier"}]),
            json!([{"type": "ResponseHeaderModifier"}]),
            json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "bad name", "value": "private-value"}]
            }}]),
            json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
                "add": [{"name": "x-test", "value": "private-value\r\ninvalid"}]
            }}]),
            json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "remove": ["bad name"]
            }}]),
            json!([{"type": "RequestHeaderModifier", "responseHeaderModifier": {}}]),
            json!([
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {}},
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {}}
            ]),
            json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
                "set": [{"name": "X-Same", "value": "ok"}], "remove": ["x-same"]
            }}]),
        ] {
            let http: Vec<HTTPRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
            let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(filters).unwrap();
            let tuning = MirrorTuning::default();
            for runtime in [
                PluginRuntime::from_httproute_filters_for_rule(&http, "default", &tuning),
                PluginRuntime::from_httproute_filters_for_backend(&http, "default", &tuning),
                PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
                PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
            ] {
                assert_rejected(&runtime).await;
            }
        }
    }

    #[tokio::test]
    async fn header_modifier_edgion_entries_reject_invalid_but_disabled_entries_are_inert() {
        let config: edgion_resources::HTTPHeaderFilter = serde_json::from_value(json!({
            "set": [{"name": "bad name", "value": "private-value"}]
        }))
        .unwrap();
        for enabled in [true, false] {
            let mut request = PluginRuntime::new();
            let errors = request.add_from_request_filters(
                &[RequestFilterEntry::with_enable(
                    EdgionPlugin::RequestHeaderModifier(config.clone()),
                    enabled,
                )],
                "default",
                "headers",
            );
            request.seal_fail_closed();
            assert_eq!(errors.is_empty(), !enabled);
            let mut response_entry =
                UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(config.clone()));
            response_entry.enable = enabled;
            let mut response = PluginRuntime::new();
            let errors = response.add_from_upstream_response_filters(&[response_entry], "default");
            response.seal_fail_closed();
            assert_eq!(errors.is_empty(), !enabled);
            for runtime in [request, response] {
                if enabled {
                    assert_rejected(&runtime).await;
                } else {
                    assert!(!runtime.has_invalid_plugins());
                    assert_eq!(runtime.request_plugins_count(), 0);
                }
            }
        }
    }

    #[test]
    fn header_modifier_valid_generation_keeps_native_operations() {
        let config = json!([
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "X-Set", "value": "ok"}]
            }},
            {"type": "ResponseHeaderModifier", "responseHeaderModifier": {
                "add": [{"name": "X-Add", "value": "ok"}]
            }}
        ]);
        let http: Vec<HTTPRouteFilter> = serde_json::from_value(config.clone()).unwrap();
        let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(config).unwrap();
        for runtime in [
            PluginRuntime::from_httproute_filters_for_rule(&http, "default", &MirrorTuning::default()),
            PluginRuntime::from_httproute_filters_for_backend(&http, "default", &MirrorTuning::default()),
            PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
            PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
        ] {
            assert!(!runtime.has_invalid_plugins());
            assert_eq!(runtime.request_plugins_count(), 1);
            assert_eq!(
                runtime.request_plugins_iter().next().unwrap().name(),
                "RequestHeaderModifier"
            );
            assert_eq!(runtime.upstream_response_plugins.len(), 1);
        }
    }
}

#[cfg(test)]
mod ext_ref_index_tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::{ForwardAuthConfig, GRPCRouteFilter, LocalObjectReference};

    async fn assert_generic_config_rejection(runtime: &PluginRuntime) {
        assert!(runtime.rejects_invalid_config());
        let filter = runtime.request_plugins_iter().next().expect("rejection filter");
        assert_eq!(filter.name(), "RejectInvalidConfig");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(filter.name());
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some(bytes::Bytes::from_static(
                    br#"{"message":"plugin configuration error"}"#
                )),
            }
        );
        let rendered = log.render_for_test();
        assert!(
            rendered.contains("ExtensionRef"),
            "missing static attribution: {rendered}"
        );
    }

    fn make_ext_ref_filter(name: &str) -> HTTPRouteFilter {
        make_ext_ref_filter_with_kind("EdgionPlugins", name)
    }

    fn make_ext_ref_filter_with_kind(kind: &str, name: &str) -> HTTPRouteFilter {
        HTTPRouteFilter {
            filter_type: HTTPRouteFilterType::ExtensionRef,
            request_header_modifier: None,
            response_header_modifier: None,
            request_redirect: None,
            url_rewrite: None,
            request_mirror: None,
            cors: None,
            external_auth: None,
            extension_ref: Some(LocalObjectReference {
                group: "edgion.io".into(),
                kind: kind.into(),
                name: name.into(),
                resolved_namespace: None,
            }),
        }
    }

    fn make_external_auth_filter(config: ForwardAuthConfig) -> HTTPRouteFilter {
        HTTPRouteFilter {
            filter_type: HTTPRouteFilterType::ExternalAuth,
            request_header_modifier: None,
            response_header_modifier: None,
            request_redirect: None,
            url_rewrite: None,
            request_mirror: None,
            cors: None,
            external_auth: Some(config),
            extension_ref: None,
        }
    }

    #[test]
    fn invalid_external_auth_seals_rule_and_backend_runtimes() {
        // The default ForwardAuth target is intentionally invalid: neither a URL
        // nor a Service name is configured, so ForwardAuth::new must fail.
        let filters = vec![make_external_auth_filter(ForwardAuthConfig::default())];

        let runtimes = [
            PluginRuntime::from_httproute_filters_for_rule(&filters, "ns1", &MirrorTuning::default()),
            PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default()),
        ];

        for runtime in runtimes {
            assert_eq!(runtime.request_plugins_count(), 1);
            assert_eq!(
                runtime.request_plugins_iter().next().map(|filter| filter.name()),
                Some("RejectInvalidConfig")
            );
        }
    }

    #[tokio::test]
    async fn missing_extension_ref_payload_fails_closed_in_all_route_scopes() {
        let http: Vec<HTTPRouteFilter> = serde_json::from_value(serde_json::json!([
            {"type": "ExtensionRef"}
        ]))
        .expect("HTTP filter fixture must deserialize");
        let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(serde_json::json!([
            {"type": "ExtensionRef"}
        ]))
        .expect("gRPC filter fixture must deserialize");

        for runtime in [
            PluginRuntime::from_httproute_filters_for_rule(&http, "default", &MirrorTuning::default()),
            PluginRuntime::from_httproute_filters_for_backend(&http, "default", &MirrorTuning::default()),
            PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
            PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
        ] {
            assert_generic_config_rejection(&runtime).await;
        }
    }

    #[test]
    fn rule_scope_assigns_sequential_indices() {
        let filters = vec![make_ext_ref_filter("p1"), make_ext_ref_filter("p2")];
        let rt = PluginRuntime::from_httproute_filters_for_rule(&filters, "ns1", &MirrorTuning::default());
        let keys = rt.ext_ref_keys();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], ("ns1".to_string(), "p1".to_string()));
        assert_eq!(keys[1], ("ns1".to_string(), "p2".to_string()));
    }

    #[test]
    fn backend_scope_does_not_collect_keys() {
        let filters = vec![make_ext_ref_filter("p1")];
        let rt = PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default());
        assert!(rt.ext_ref_keys().is_empty());
    }

    #[test]
    fn grpc_rule_scope_uses_controller_resolved_namespace() {
        let filter: GRPCRouteFilter = serde_json::from_value(serde_json::json!({
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io",
                "kind": "EdgionPlugins",
                "name": "child-plugin",
                "resolvedNamespace": "child"
            }
        }))
        .expect("gRPC ExtensionRef fixture must deserialize");
        let runtime = PluginRuntime::from_grpcroute_filters_for_rule(&[filter], "parent");
        assert_eq!(
            runtime.ext_ref_keys(),
            &[("child".to_string(), "child-plugin".to_string())]
        );
    }

    #[test]
    fn lb_policy_kind_extension_ref_now_builds_a_filter_like_any_other_kind() {
        // The route-level extensionRef LB mechanism was removed: `LBPolicy*`
        // kinds are no longer special-cased by the plugin runtime. A
        // `LBPolicyConsistentHash` ExtensionRef is now treated exactly like any
        // other ExtensionRef filter, so it builds an ExtensionRefFilter.
        // (Routes using this kind are excluded from the data-plane route table
        // upstream at the controller's extensionRef-kind validation gate, so
        // this filter would never actually execute in production — but the
        // gateway-side runtime construction no longer knows or cares about the
        // kind.)
        let filters = vec![make_ext_ref_filter_with_kind(
            "LBPolicyConsistentHash",
            "header.x-user-id",
        )];
        let rt = PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default());
        assert_eq!(
            rt.request_plugins_count(),
            1,
            "LBPolicy* kinds are no longer special-cased; an ExtensionRefFilter is built like any other kind"
        );
    }
}
