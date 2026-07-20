//! Plugin runtime - manages plugin execution across different stages

use std::sync::Arc;
use std::time::Duration;

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::plugins::runtime::log::la;
use crate::runtime::server::http_to_grpc_status;
use crate::EdgionHttpContext;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::filters::PluginRunningResult::ErrTerminateRequest;
use edgion_resources::resources::edgion_plugins::CorsConfig;
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
use super::log::{PluginLog, StageLogs};
use super::reject_filter::RejectFilter;
use super::session_adapter::PingoraSessionAdapter;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::plugins::http::all_endpoint_status::AllEndpointStatus;
use crate::plugins::http::bandwidth_limit::BandwidthLimit;
use crate::plugins::http::basic_auth::BasicAuth;
use crate::plugins::http::canary::Canary;
use crate::plugins::http::cors::Cors;
use crate::plugins::http::csrf::Csrf;
use crate::plugins::http::ctx_set::CtxSet;
use crate::plugins::http::direct_endpoint::DirectEndpoint;
use crate::plugins::http::dsl::plugin::DslPlugin;
use crate::plugins::http::dynamic_external_upstream::DynamicExternalUpstream;
use crate::plugins::http::dynamic_internal_upstream::DynamicInternalUpstream;
use crate::plugins::http::ext_proc::ExtProc;
use crate::plugins::http::fault_injection::FaultInjection;
use crate::plugins::http::forward_auth::ForwardAuth;
use crate::plugins::http::global_access_control::GlobalAccessControl;
use crate::plugins::http::header_cert_auth::HeaderCertAuth;
use crate::plugins::http::hmac_auth::HmacAuth;
use crate::plugins::http::ip_restriction::IpRestriction;
use crate::plugins::http::jwe_decrypt::JweDecrypt;
use crate::plugins::http::jwt_auth::JwtAuth;
use crate::plugins::http::key_auth::KeyAuth;
use crate::plugins::http::ldap_auth::LdapAuth;
use crate::plugins::http::mock::Mock;
use crate::plugins::http::openid_connect::OpenidConnect;
use crate::plugins::http::proxy_rewrite::ProxyRewrite;
use crate::plugins::http::rate_limit::RateLimit;
use crate::plugins::http::rate_limit_redis::RateLimitRedis;
use crate::plugins::http::real_ip::RealIp;
use crate::plugins::http::region_route::RegionRoute;
use crate::plugins::http::request_mirror::RequestMirrorPlugin;
use crate::plugins::http::request_restriction::RequestRestriction;
use crate::plugins::http::response_rewrite::ResponseRewrite;
use crate::plugins::http::trace_context::TraceContext;
use crate::plugins::http::wasm::WasmPlugin;
use crate::plugins::runtime::gateway_api_filters::{
    DebugAccessLogToHeaderFilter, ExtensionRefFilter, GapiCorsFilter, RequestHeaderModifierFilter,
    RequestRedirectFilter, ResponseHeaderModifierFilter, URLRewriteFilter,
};

pub struct PluginRuntime {
    /// Plugins for request stage (async)
    request_plugins: Vec<Box<dyn RequestFilter>>,
    /// Plugins for upstream_response_filter stage (sync)
    upstream_response_plugins: Vec<Box<dyn UpstreamResponseFilter>>,
    /// Plugins for upstream_response_body_filter stage (sync, bandwidth throttling)
    upstream_response_body_plugins: Vec<Box<dyn UpstreamResponseBodyFilter>>,
    /// Plugins for response_filter stage (async)
    upstream_response_async_plugins: Vec<Box<dyn UpstreamResponse>>,
    /// (namespace, name) of every ExtensionRef in this runtime, in build order.
    /// Position == index assigned via `ExtensionRefFilter::set_index`. Empty
    /// when this runtime is built via the backend-scope path
    /// (`from_httproute_filters_for_backend` / `from_grpcroute_filters_for_backend`),
    /// because backend-scope ExtensionRefFilters keep `index = None` and use
    /// the legacy direct-store-lookup path.
    ext_ref_keys: Vec<(String, String)>,
    /// Pre-resolved Arc to the first enabled `EdgionPlugin::Cors` config in
    /// `request_plugins`. Built once at preparse so the global CORS preflight
    /// handler (`pg_request_filter::extract_cors_config_from_route_filters`)
    /// can return an Arc clone instead of deep-cloning the full `CorsConfig`
    /// per OPTIONS preflight. Hot-reload semantics are preserved: every
    /// `EdgionPlugins` update rebuilds this runtime, producing a new Arc that
    /// the plugin store swaps in atomically.
    cors_config: Option<Arc<CorsConfig>>,
    /// Names of plugins whose config failed `validation_error()` while building
    /// this runtime. Such plugins are NOT constructed (an invalid config must
    /// never run in the request path). `seal_fail_closed` consumes this list to
    /// prepend a single request-stage `RejectFilter`, so the whole route fails
    /// closed with 500 instead of silently dropping the misconfigured plugin.
    invalid_plugins: Vec<&'static str>,
    /// Qualified names + status of Block-stamped entries, in spec order across
    /// all four stages (builders run request → urf → urbf → ur). The first
    /// element's status is the one the gate returns.
    policy_blocks: Vec<(String, u16)>,
    /// Qualified names of Bypass-stamped entries.
    policy_bypassed: Vec<String>,
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
            invalid_plugins: Vec::new(),
            policy_blocks: Vec::new(),
            policy_bypassed: Vec::new(),
        }
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
        let reject = RejectFilter::new(std::mem::take(&mut self.invalid_plugins));
        self.request_plugins.insert(0, Box::new(reject));
    }

    /// Prepend policy enforcement filters. MUST be called AFTER
    /// `seal_fail_closed` so the block gate lands ahead of the fail-closed
    /// RejectFilter (policy status code wins; spec §5 precedence).
    pub fn seal_plugin_policy(&mut self) {
        if !self.policy_bypassed.is_empty() {
            let note = super::policy_filter::PolicyBypassNote::new(std::mem::take(&mut self.policy_bypassed));
            self.request_plugins.insert(0, Box::new(note));
        }
        if let Some((plugin, status)) = self.policy_blocks.first().cloned() {
            self.policy_blocks.clear();
            let gate = super::policy_filter::PolicyBlockFilter::new(plugin, status);
            self.request_plugins.insert(0, Box::new(gate));
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
        runtime
    }

    fn add_from_httproute_filters_inner(
        &mut self,
        filters: &[HTTPRouteFilter],
        namespace: &str,
        assign_indices: bool,
        mirror_tuning: &MirrorTuning,
    ) {
        for filter in filters {
            match filter.filter_type {
                HTTPRouteFilterType::RequestHeaderModifier => {
                    if let Some(config) = &filter.request_header_modifier {
                        self.add_request_filter(Box::new(RequestHeaderModifierFilter::new(config.clone())));
                    }
                }
                HTTPRouteFilterType::ResponseHeaderModifier => {
                    if let Some(config) = &filter.response_header_modifier {
                        self.add_upstream_response_filter(Box::new(ResponseHeaderModifierFilter::new(config.clone())));
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
                            Err(_) => self.invalid_plugins.push("ForwardAuth"),
                        }
                    }
                }
                HTTPRouteFilterType::ExtensionRef => {
                    if let Some(ext_ref) = &filter.extension_ref {
                        let mut ext_filter = ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone());
                        if assign_indices {
                            let idx = self.ext_ref_keys.len();
                            self.ext_ref_keys.push((namespace.to_string(), ext_ref.name.clone()));
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
        runtime
    }

    /// Build a runtime for a gRPC backend reference. ExtensionRef filters retain
    /// `index = None` and fall back to direct global store lookup at exec time.
    pub fn from_grpcroute_filters_for_backend(filters: &[GRPCRouteFilter], namespace: &str) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_grpcroute_filters_inner(filters, namespace, false);
        runtime
    }

    fn add_from_grpcroute_filters_inner(&mut self, filters: &[GRPCRouteFilter], namespace: &str, assign_indices: bool) {
        for filter in filters {
            match filter.filter_type {
                GRPCRouteFilterType::RequestHeaderModifier => {
                    if let Some(config) = &filter.request_header_modifier {
                        self.add_request_filter(Box::new(RequestHeaderModifierFilter::new_from_grpc(config.clone())));
                    }
                }
                GRPCRouteFilterType::ResponseHeaderModifier => {
                    if let Some(config) = &filter.response_header_modifier {
                        self.add_upstream_response_filter(Box::new(ResponseHeaderModifierFilter::new_from_grpc(
                            config.clone(),
                        )));
                    }
                }
                GRPCRouteFilterType::ExtensionRef => {
                    if let Some(ext_ref) = &filter.extension_ref {
                        let mut ext_filter = ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone());
                        if assign_indices {
                            let idx = self.ext_ref_keys.len();
                            self.ext_ref_keys.push((namespace.to_string(), ext_ref.name.clone()));
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
    pub fn add_from_request_filters(&mut self, entries: &[RequestFilterEntry], namespace: &str) -> Vec<String> {
        let mut errors = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                // Plugin policy: a stamped entry is never instantiated and is
                // deliberately NOT recorded in invalid_plugins (that would turn
                // Bypass into a fail-closed 500 via seal_fail_closed).
                if let Some(action) = &entry.policy_action {
                    let qualified = format!("http/{}", Self::get_plugin_name(&entry.plugin));
                    match action {
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { status } => {
                            self.policy_blocks.push((qualified, *status));
                        }
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass => {
                            self.policy_bypassed.push(qualified);
                        }
                    }
                    continue;
                }

                // Reject plugins whose config fails validation: record the name,
                // surface the error, and skip construction so an invalid config
                // can never run in the request path. `seal_fail_closed` later
                // turns a non-empty `invalid_plugins` into a fail-closed 500.
                if let Some(error) = Self::plugin_validation_error(&entry.plugin) {
                    let plugin_name = Self::get_plugin_name(&entry.plugin);
                    errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, error));
                    self.invalid_plugins.push(plugin_name);
                    continue;
                }

                // Cache the first enabled CORS config so the global preflight
                // handler can fetch it without deep-cloning per request.
                // Initialize the `#[serde(skip)]` runtime fields (origins_cache /
                // compiled_origins_regex / compiled_timing_regex) here: the
                // preflight handler builds `Cors::from_arc` from this Arc and never
                // re-initializes, so an uncompiled config would silently break
                // regex / subdomain-wildcard / comma-separated origins on the
                // preflight path. On failure keep the uncompiled (deny-safe) config.
                if self.cors_config.is_none() {
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
                        self.cors_config = Some(Arc::new(cfg));
                    }
                }

                // Validate optional entry-level alias. On failure, surface
                // the error and drop the alias so an unvalidated value never
                // reaches the access log.
                let entry_alias = match entry.validate_alias() {
                    None => entry.alias.clone(),
                    Some(err) => {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, err));
                        None
                    }
                };

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
                        plugin_name,
                        &format!("requestPlugins[{}] ({})", index, plugin_name),
                        &mut errors,
                        &mut self.invalid_plugins,
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
                    if !entry.plugin.accepts_body() {
                        errors.push(format!(
                            "requestPlugins[{}] ({}): body: plugin does not consume a body",
                            index, plugin_name
                        ));
                    } else if let Some(err) = entry.body.as_ref().and_then(|b| b.validation_error()) {
                        errors.push(format!("requestPlugins[{}] ({}): body: {}", index, plugin_name, err));
                    }
                }

                match Self::create_request_filter_from_edgion(&entry.plugin, namespace) {
                    CreateResult::Ok(Some(filter)) => {
                        // Resolve the effective body requirement from the
                        // operator's `body:` and the plugin's intrinsic
                        // declaration (the operator may tune size/conditions but
                        // may not downgrade an intrinsic fail-close). When present,
                        // wrap the inner filter in a body gate (inside the
                        // Conditional wrapper, so entry run_if/skip_if runs first).
                        let body_req = edgion_resources::resources::edgion_plugins::BodyRequirement::resolve(
                            entry.body.clone(),
                            entry.plugin.body_requirement(),
                        );
                        let gated: Box<dyn RequestFilter> = match body_req {
                            Some(req) => Box::new(BodyGatedRequestFilter::new(filter, req)),
                            None => filter,
                        };
                        // Wrap with ConditionalRequestFilter to support condition evaluation
                        let conditional_filter = ConditionalRequestFilter::with_alias_and_dye(
                            gated,
                            entry.conditions.clone(),
                            entry_alias,
                            dye_plan,
                        );
                        self.add_request_filter(Box::new(conditional_filter));
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
                        self.invalid_plugins.push(plugin_name);
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
                        self.invalid_plugins.push(plugin_name);
                    }
                }
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
        let mut errors = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                // Plugin policy: a stamped entry is never instantiated and is
                // deliberately NOT recorded in invalid_plugins (that would turn
                // Bypass into a fail-closed 500 via seal_fail_closed).
                if let Some(action) = &entry.policy_action {
                    let qualified = format!("http/{}", Self::get_plugin_name(&entry.plugin));
                    match action {
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { status } => {
                            self.policy_blocks.push((qualified, *status));
                        }
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass => {
                            self.policy_bypassed.push(qualified);
                        }
                    }
                    continue;
                }

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
                    self.invalid_plugins.push(plugin_name);
                    continue;
                }

                // Validate optional entry-level alias. On failure, surface
                // the error and drop the alias so an unvalidated value never
                // reaches the access log.
                let entry_alias = match entry.validate_alias() {
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
                        plugin_name,
                        &format!("upstreamResponseFilterPlugins[{}] ({})", index, plugin_name),
                        &mut errors,
                        &mut self.invalid_plugins,
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
                        self.add_upstream_response_filter(Box::new(conditional_filter));
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
                        self.invalid_plugins.push(plugin_name);
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
                        self.invalid_plugins.push(plugin_name);
                    }
                }
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
        let mut errors = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                // Plugin policy: a stamped entry is never instantiated and is
                // deliberately NOT recorded in invalid_plugins (that would turn
                // Bypass into a fail-closed 500 via seal_fail_closed).
                if let Some(action) = &entry.policy_action {
                    let qualified = format!("http/{}", Self::get_plugin_name(&entry.plugin));
                    match action {
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { status } => {
                            self.policy_blocks.push((qualified, *status));
                        }
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass => {
                            self.policy_bypassed.push(qualified);
                        }
                    }
                    continue;
                }

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
                    self.invalid_plugins.push(plugin_name);
                    continue;
                }

                // Validate the optional operator body requirement. `body:` only
                // applies to plugins that consume the body in their current config;
                // reject it elsewhere so it is never a silent no-op. Non-fatal.
                if entry.body.is_some() && !entry.plugin.accepts_body() {
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

                let entry_alias = match entry.validate_alias() {
                    None => entry.alias.clone(),
                    Some(err) => {
                        let plugin_name = Self::get_plugin_name(&entry.plugin);
                        errors.push(format!("upstreamResponsePlugins[{}] ({}): {}", index, plugin_name, err));
                        None
                    }
                };

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
                        plugin_name,
                        &format!("upstreamResponsePlugins[{}] ({})", index, plugin_name),
                        &mut errors,
                        &mut self.invalid_plugins,
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
                        let body_req = edgion_resources::resources::edgion_plugins::BodyRequirement::resolve(
                            entry.body.clone(),
                            entry.plugin.body_requirement(),
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
                        self.add_upstream_response(Box::new(conditional_filter));
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
                        self.invalid_plugins.push(plugin_name);
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
                        self.invalid_plugins.push(plugin_name);
                    }
                }
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
        let mut errors = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            if entry.is_enabled() {
                // Plugin policy: a stamped entry is never instantiated and is
                // deliberately NOT recorded in invalid_plugins (that would turn
                // Bypass into a fail-closed 500 via seal_fail_closed).
                if let Some(action) = &entry.policy_action {
                    let qualified = format!("http/{}", Self::get_plugin_name(&entry.plugin));
                    match action {
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { status } => {
                            self.policy_blocks.push((qualified, *status));
                        }
                        edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass => {
                            self.policy_bypassed.push(qualified);
                        }
                    }
                    continue;
                }

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
                    self.invalid_plugins.push(plugin_name);
                    continue;
                }

                // Validate optional entry-level alias. On failure, surface
                // the error and drop the alias so an unvalidated value never
                // reaches the access log.
                let entry_alias = match entry.validate_alias() {
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
                        plugin_name,
                        &format!("upstreamResponseBodyFilterPlugins[{}] ({})", index, plugin_name),
                        &mut errors,
                        &mut self.invalid_plugins,
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
                        self.add_upstream_response_body_filter(Box::new(conditional_filter));
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
                        self.invalid_plugins.push(plugin_name);
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
                        self.invalid_plugins.push(plugin_name);
                    }
                }
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

    /// Create a RequestFilter instance from EdgionPlugin enum
    ///
    /// # Arguments
    /// * `plugin` - The EdgionPlugin configuration
    /// * `namespace` - The namespace of the EdgionPlugins resource
    fn create_request_filter_from_edgion(
        plugin: &EdgionPlugin,
        namespace: &str,
    ) -> CreateResult<Box<dyn RequestFilter>> {
        match plugin {
            EdgionPlugin::RequestHeaderModifier(config) => {
                CreateResult::Ok(Some(Box::new(RequestHeaderModifierFilter::new(config.clone()))))
            }
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
            EdgionPlugin::Cors(config) => CreateResult::Ok(Some(Box::new(Cors::new(config)))),
            EdgionPlugin::Csrf(config) => CreateResult::Ok(Some(Box::new(Csrf::new(config)))),
            EdgionPlugin::IpRestriction(config) => CreateResult::Ok(Some(IpRestriction::create(config, namespace))),
            EdgionPlugin::GlobalAccessControl(config) => {
                CreateResult::Ok(Some(GlobalAccessControl::create(config, namespace)))
            }
            EdgionPlugin::Canary(config) => CreateResult::Ok(Some(Canary::create(config, namespace))),
            EdgionPlugin::JwtAuth(config) => {
                CreateResult::Ok(Some(Box::new(JwtAuth::new(config, namespace.to_string()))))
            }
            EdgionPlugin::JweDecrypt(config) => {
                CreateResult::Ok(Some(Box::new(JweDecrypt::new(config, namespace.to_string()))))
            }
            EdgionPlugin::HmacAuth(config) => CreateResult::Ok(Some(HmacAuth::create(config))),
            EdgionPlugin::HeaderCertAuth(config) => {
                CreateResult::Ok(Some(Box::new(HeaderCertAuth::new(config, namespace.to_string()))))
            }
            EdgionPlugin::KeyAuth(config) => CreateResult::Ok(Some(KeyAuth::create(config))),
            EdgionPlugin::LdapAuth(config) => CreateResult::Ok(LdapAuth::create(config)),
            EdgionPlugin::FaultInjection(config) => CreateResult::Ok(Some(Box::new(FaultInjection::new(config)))),
            EdgionPlugin::Mock(config) => CreateResult::Ok(Some(Box::new(Mock::new(config)))),
            EdgionPlugin::ProxyRewrite(config) => CreateResult::Ok(Some(Box::new(ProxyRewrite::new(config)))),
            EdgionPlugin::RequestRestriction(config) => CreateResult::Ok(Some(RequestRestriction::create(config))),
            EdgionPlugin::RateLimit(config) => CreateResult::Ok(Some(RateLimit::create(config))),
            EdgionPlugin::RateLimitRedis(config) => CreateResult::Ok(Some(RateLimitRedis::create(config))),
            EdgionPlugin::CtxSet(config) => CreateResult::Ok(Some(CtxSet::create(config))),
            EdgionPlugin::DirectEndpoint(config) => CreateResult::Ok(Some(Box::new(DirectEndpoint::new(config)))),
            EdgionPlugin::DynamicInternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicInternalUpstream::create(config)))
            }
            EdgionPlugin::DynamicExternalUpstream(config) => {
                CreateResult::Ok(Some(DynamicExternalUpstream::create(config)))
            }
            EdgionPlugin::RealIp(config) => CreateResult::Ok(Some(RealIp::create(config))),
            EdgionPlugin::TraceContext(config) => CreateResult::Ok(Some(TraceContext::create(config))),
            EdgionPlugin::ExtProc(config) => CreateResult::Ok(ExtProc::create_request(config)),
            EdgionPlugin::ForwardAuth(config) => match ForwardAuth::new(config) {
                Ok(fa) => CreateResult::Ok(Some(Box::new(fa))),
                Err(_) => CreateResult::Ok(None),
            },
            EdgionPlugin::AllEndpointStatus(config) => CreateResult::Ok(Some(Box::new(AllEndpointStatus::new(config)))),
            EdgionPlugin::OpenidConnect(config) => {
                CreateResult::Ok(Some(Box::new(OpenidConnect::new(config, namespace.to_string()))))
            }
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
            EdgionPlugin::ResponseHeaderModifier(config) => {
                CreateResult::Ok(Some(Box::new(ResponseHeaderModifierFilter::new(config.clone()))))
            }
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
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::RateLimit(_)
            | EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::TraceContext(_)
            | EdgionPlugin::ForwardAuth(_)
            | EdgionPlugin::OpenidConnect(_)
            | EdgionPlugin::BandwidthLimit(_)
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::GlobalAccessControl(_)
            | EdgionPlugin::Canary(_) => CreateResult::WrongStage,
        }
    }

    /// Create an UpstreamResponse instance from EdgionPlugin enum
    fn create_upstream_response_from_edgion(
        plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> CreateResult<Box<dyn UpstreamResponse>> {
        match plugin {
            EdgionPlugin::ExtProc(config) => CreateResult::Ok(ExtProc::create_response(config)),
            // Every other variant belongs to a different stage.
            _ => CreateResult::WrongStage,
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
            | EdgionPlugin::DebugAccessLogToHeader(_)
            | EdgionPlugin::ProxyRewrite(_)
            | EdgionPlugin::RequestRestriction(_)
            | EdgionPlugin::ResponseRewrite(_)
            | EdgionPlugin::RateLimit(_)
            | EdgionPlugin::RateLimitRedis(_)
            | EdgionPlugin::CtxSet(_)
            | EdgionPlugin::RealIp(_)
            | EdgionPlugin::TraceContext(_)
            | EdgionPlugin::ForwardAuth(_)
            | EdgionPlugin::OpenidConnect(_)
            | EdgionPlugin::DirectEndpoint(_)
            | EdgionPlugin::AllEndpointStatus(_)
            | EdgionPlugin::DynamicInternalUpstream(_)
            | EdgionPlugin::DynamicExternalUpstream(_)
            | EdgionPlugin::FaultInjection(_)
            | EdgionPlugin::ExtProc(_)
            | EdgionPlugin::RegionRoute(_)
            | EdgionPlugin::GlobalAccessControl(_)
            | EdgionPlugin::Canary(_)
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
            EdgionPlugin::Cors(_) => "Cors",
            EdgionPlugin::Csrf(_) => "Csrf",
            EdgionPlugin::IpRestriction(_) => "IpRestriction",
            EdgionPlugin::GlobalAccessControl(_) => "GlobalAccessControl",
            EdgionPlugin::Canary(_) => "Canary",
            EdgionPlugin::JwtAuth(_) => "JwtAuth",
            EdgionPlugin::JweDecrypt(_) => "JweDecrypt",
            EdgionPlugin::HmacAuth(_) => "HmacAuth",
            EdgionPlugin::HeaderCertAuth(_) => "HeaderCertAuth",
            EdgionPlugin::KeyAuth(_) => "KeyAuth",
            EdgionPlugin::LdapAuth(_) => "LdapAuth",
            EdgionPlugin::Mock(_) => "Mock",
            EdgionPlugin::ProxyRewrite(_) => "ProxyRewrite",
            EdgionPlugin::RequestRestriction(_) => "RequestRestriction",
            EdgionPlugin::RateLimit(_) => "RateLimit",
            EdgionPlugin::RateLimitRedis(_) => "RateLimitRedis",
            EdgionPlugin::CtxSet(_) => "CtxSet",
            EdgionPlugin::DirectEndpoint(_) => "DirectEndpoint",
            EdgionPlugin::RealIp(_) => "RealIp",
            EdgionPlugin::TraceContext(_) => "TraceContext",
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
        plugin_name: &'static str,
        context: &str,
        errors: &mut Vec<String>,
        invalid_plugins: &mut Vec<&'static str>,
    ) -> Option<Arc<super::dye_plan::CompiledDyePlan>> {
        let suite = dye?;
        match super::dye_plan::CompiledDyePlan::compile(suite, plugin.dye_type_prefix()) {
            Ok(plan) => Some(plan),
            Err(e) => {
                errors.push(format!(
                    "{context}: dye plan compile error (bug, already validated): {e}"
                ));
                invalid_plugins.push(plugin_name);
                None
            }
        }
    }

    fn add_request_filter(&mut self, filter: Box<dyn RequestFilter>) {
        self.request_plugins.push(filter);
    }

    fn add_upstream_response_filter(&mut self, filter: Box<dyn UpstreamResponseFilter>) {
        self.upstream_response_plugins.push(filter);
    }

    fn add_upstream_response_body_filter(&mut self, filter: Box<dyn UpstreamResponseBodyFilter>) {
        self.upstream_response_body_plugins.push(filter);
    }

    fn add_upstream_response(&mut self, filter: Box<dyn UpstreamResponse>) {
        self.upstream_response_async_plugins.push(filter);
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
    pub fn request_plugins_iter(&self) -> impl Iterator<Item = &Box<dyn RequestFilter>> {
        self.request_plugins.iter()
    }

    /// Iterate over upstream_response_filter stage filters (sync)
    pub fn upstream_response_plugins_iter(&self) -> impl Iterator<Item = &Box<dyn UpstreamResponseFilter>> {
        self.upstream_response_plugins.iter()
    }

    /// Iterate over upstream_response_body_filter stage filters (sync)
    pub fn upstream_response_body_plugins_iter(&self) -> impl Iterator<Item = &Box<dyn UpstreamResponseBodyFilter>> {
        self.upstream_response_body_plugins.iter()
    }

    /// Iterate over response_filter stage filters (async)
    pub fn upstream_response_async_plugins_iter(&self) -> impl Iterator<Item = &Box<dyn UpstreamResponse>> {
        self.upstream_response_async_plugins.iter()
    }

    /// Run request stage filters (async)
    pub async fn run_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        if self.request_plugins.is_empty() {
            return;
        }

        // Snapshot before `ctx` is borrowed by `session_adapter`. Set in
        // `pg_request_filter` and not mutated during plugin execution.
        let is_grpc_request = ctx.request_info.is_grpc_request;
        let mut filter_logs = Vec::with_capacity(self.request_plugins.len());
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for filter in &self.request_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Request-direction
            // anti-spoof strip runs here too, before the plugin's kill
            // switch / gating / conditions, so a client-forged value of a
            // name this entry declares can never survive into the plugin.
            session_adapter.install_dye_plan(filter.dye_plan().cloned());
            session_adapter.strip_request_dye_once();

            let mut plugin_log = PluginLog::with_alias(filter.name(), filter.alias());
            let start = std::time::Instant::now();

            let result = filter.run_request(&mut session_adapter, &mut plugin_log).await;

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost =
                    Some(std::time::Instant::now().saturating_duration_since(start).as_micros() as u64);
            }
            filter_logs.push(plugin_log);

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
                    let message = body.as_deref().or(Some(default_msg));
                    let _ = session_adapter.send_grpc_error_direct(grpc_status, message).await;
                } else if let Ok(status_code) = http::StatusCode::from_u16(status) {
                    if let Ok(mut resp) = ResponseHeader::build(status_code, Some(2)) {
                        let _ = resp.insert_header("content-type", "text/plain");
                        let body_bytes = body.map(bytes::Bytes::from);
                        let len = body_bytes.as_ref().map(|b| b.len()).unwrap_or(0);
                        // Apply queued ops first so plugin-set values (e.g. Mock's
                        // Content-Type) override the runtime defaults, THEN re-assert
                        // framing so a queued Content-Length / Transfer-Encoding (now
                        // reachable via an ext_proc ImmediateResponse's header mutation)
                        // cannot desync the wire from the actual body length.
                        session_adapter.drain_pending_response_header_ops(&mut resp);
                        let _ = resp.remove_header(&http::header::TRANSFER_ENCODING);
                        let _ = resp.insert_header("content-length", len.to_string());

                        let _ = session_adapter
                            .write_response_header(Box::new(resp), body_bytes.is_none())
                            .await;
                        if let Some(b) = body_bytes {
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

        ctx.stage_logs.push(StageLogs {
            stage: "request_filters",
            filters: filter_logs,
            edgion_plugins: std::mem::take(&mut ctx.pending_edgion_plugins_logs),
        });
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

        let mut filter_logs = Vec::with_capacity(self.upstream_response_plugins.len());
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Response-stage
            // loops do not strip: request-direction rules are meaningless
            // here (validation forbids them on non-request-stage entries).
            session_adapter.install_dye_plan(filter.dye_plan().cloned());

            let mut plugin_log = PluginLog::with_alias(filter.name(), filter.alias());
            let start = std::time::Instant::now();

            let result = filter.run_upstream_response_filter(&mut session_adapter, &mut plugin_log);

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost =
                    Some(std::time::Instant::now().saturating_duration_since(start).as_micros() as u64);
            }

            let should_terminate = check_response_stage_terminate(&result, &mut plugin_log, &mut session_adapter);
            filter_logs.push(plugin_log);

            if should_terminate {
                session_adapter.set_terminate();
                break;
            }
        }

        ctx.stage_logs.push(StageLogs {
            stage: "upstream_response_filters",
            filters: filter_logs,
            edgion_plugins: std::mem::take(&mut ctx.pending_edgion_plugins_logs),
        });
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

        let mut filter_logs = Vec::with_capacity(self.upstream_response_async_plugins.len());
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_async_plugins {
            // Install this entry's dye plan before invocation (Task 7's
            // record_outcome lifts it back out of ctx). Response-stage
            // loops do not strip: request-direction rules are meaningless
            // here (validation forbids them on non-request-stage entries).
            session_adapter.install_dye_plan(filter.dye_plan().cloned());

            let mut plugin_log = PluginLog::with_alias(filter.name(), filter.alias());
            let start = std::time::Instant::now();

            let result = filter
                .run_upstream_response(&mut session_adapter, &mut plugin_log)
                .await;

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost =
                    Some(std::time::Instant::now().saturating_duration_since(start).as_micros() as u64);
            }

            let should_terminate = check_response_stage_terminate(&result, &mut plugin_log, &mut session_adapter);
            filter_logs.push(plugin_log);

            if should_terminate {
                session_adapter.set_terminate();
                break;
            }
        }

        ctx.stage_logs.push(StageLogs {
            stage: "upstream_responses",
            filters: filter_logs,
            edgion_plugins: std::mem::take(&mut ctx.pending_edgion_plugins_logs),
        });
    }
}

/// Check if a plugin result should terminate execution in the response stage,
/// and — for HTTP routes — turn an `ErrResponse { status, body }` into a local
/// reply that fully replaces the upstream response (status + headers + body).
///
/// Building the local reply is synchronous, so this is shared by both the sync
/// (`run_upstream_response_plugins_sync`) and async
/// (`run_upstream_response_plugins_async`) loops: it stashes the built reply in
/// `ctx` (`local_reply_header`/`local_reply_body`). `response_filter` then writes
/// that reply to the downstream and returns a sentinel `Err` to abort the
/// upstream forward — this is what makes a body-less upstream (where Pingora
/// never calls the body filter) still emit the replacement body. gRPC routes
/// are not supported: the response is terminated by status only (body not
/// replaced) and a marker is recorded.
fn check_response_stage_terminate(
    result: &PluginRunningResult,
    plugin_log: &mut PluginLog,
    adapter: &mut PingoraSessionAdapter<'_>,
) -> bool {
    match result {
        PluginRunningResult::ErrTerminateRequest => true,
        PluginRunningResult::ErrResponse { status, body } => {
            if adapter.apply_response_local_reply(*status, body.clone()) {
                plugin_log.push_fmt("replace response {}", la![status.to_string().as_str()]);
            } else {
                // Refused (gRPC route, Upgrade/1xx upstream, or no header bound):
                // keep legacy log-and-terminate behavior. Only tag the gRPC marker
                // for genuine gRPC routes so it does not mislabel Upgrade/1xx.
                plugin_log.push_fmt("deny response {}", la![status.to_string().as_str()]);
                if adapter.is_grpc_request() {
                    adapter.note_response_local_reply_unsupported();
                }
            }
            true
        }
        PluginRunningResult::ErrGrpcError { grpc_status, .. } => {
            plugin_log.push_fmt("deny grpc {}", la![grpc_status.to_string().as_str()]);
            true
        }
        PluginRunningResult::Nothing | PluginRunningResult::GoodNext => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::{EdgionPlugin, KeyAuthConfig, RequestFilterEntry};

    #[tokio::test]
    async fn add_from_request_filters_wraps_body_consuming_plugin() {
        // A body-consuming plugin (HMAC with validateRequestBody) must be wrapped
        // in a body gate by the wiring. Proof: running the built filter force-reads
        // the body (Unimplemented today) and fail-closes 500 BEFORE HMAC's own
        // logic. If the gate were NOT wired in, HMAC would instead 401 on the
        // missing Authorization header (ErrTerminateRequest).
        use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
        use crate::EdgionHttpContext;
        use edgion_resources::resources::HmacAuthConfig;

        let cfg = HmacAuthConfig {
            validate_request_body: true,
            anonymous: Some("anon".to_string()),
            ..Default::default()
        };
        let mut runtime = PluginRuntime::new();
        let errors =
            runtime.add_from_request_filters(&[RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg))], "default");
        assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

        let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
        assert_eq!(filter.name(), "HmacAuth", "gate must be transparent to name()");

        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("POST".to_string());
        session.expect_header_value().returning(|_| None);
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_read_request_body()
            .returning(|_| Err(BodyReadError::Unimplemented));

        let mut log = PluginLog::new("HmacAuth");
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            },
            "consuming plugin must be gated (fail-close) before its own auth logic"
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
        let errors = runtime.add_from_request_filters(&entries, "default");

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
        use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};
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
            on_invalid: Default::default(),
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
        let errors = runtime.add_from_request_filters(&[entry], "default");
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
        let errors = runtime.add_from_request_filters(&[entry], "default");
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
        let errors = runtime.add_from_request_filters(&entries, "default");

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
        runtime.add_from_request_filters(&entries, "default");

        runtime.seal_fail_closed();
        assert_eq!(
            runtime.request_plugins_count(),
            1,
            "no RejectFilter when every plugin config is valid"
        );
    }

    #[test]
    fn policy_stamped_entries_are_not_instantiated_and_not_invalid() {
        use edgion_resources::resources::edgion_plugins::{CorsConfig, MockConfig};
        use edgion_resources::resources::plugin_policy::PluginPolicyAction;

        let mut blocked = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        blocked.policy_action = Some(PluginPolicyAction::Block { status: 451 });
        let mut bypassed = RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        bypassed.policy_action = Some(PluginPolicyAction::Bypass);
        let normal = RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default()));

        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(&[blocked, bypassed, normal], "default");
        assert!(errors.is_empty(), "policy stamps are not validation errors: {errors:?}");

        // Only the normal plugin was instantiated.
        assert_eq!(runtime.request_plugins.len(), 1);
        // Stamped entries never enter invalid_plugins (Bypass must not 500).
        assert!(runtime.invalid_plugins.is_empty());

        runtime.seal_fail_closed();
        runtime.seal_plugin_policy();
        // Block gate + bypass note prepended; block gate is FIRST.
        assert_eq!(runtime.request_plugins.len(), 3);
        assert_eq!(runtime.request_plugins[0].name(), "PluginPolicyBlock");
        assert_eq!(runtime.request_plugins[1].name(), "PluginPolicyBypass");
    }

    #[test]
    fn policy_block_gate_precedes_fail_closed_reject() {
        // A chain with BOTH an invalid plugin and a policy block: policy wins.
        use edgion_resources::resources::plugin_policy::PluginPolicyAction;

        let mut blocked = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        blocked.policy_action = Some(PluginPolicyAction::Block { status: 451 });
        let mut invalid_alias = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
        invalid_alias.alias = Some("bad alias with spaces".to_string());

        let mut runtime = PluginRuntime::new();
        let _ = runtime.add_from_request_filters(&[blocked, invalid_alias], "default");
        runtime.seal_fail_closed();
        runtime.seal_plugin_policy();
        assert_eq!(runtime.request_plugins[0].name(), "PluginPolicyBlock");
    }

    #[test]
    fn no_stamps_means_no_policy_filters() {
        use edgion_resources::resources::edgion_plugins::MockConfig;

        let normal = RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default()));
        let mut runtime = PluginRuntime::new();
        let _ = runtime.add_from_request_filters(&[normal], "default");
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

        runtime.add_from_request_filters(&[bad_dsl], "default");

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
        use edgion_resources::resources::edgion_plugins::plugin_configs::CanaryConfig;
        use edgion_resources::resources::edgion_plugins::plugin_configs::DynUpstreamTarget;
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
            on_invalid: Default::default(),
        };

        let plugin = EdgionPlugin::Canary(config);

        // Verify request stage returns Ok(Some(...)).
        let result = PluginRuntime::create_request_filter_from_edgion(&plugin, "default");
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
}

#[cfg(test)]
mod ext_ref_index_tests {
    use super::*;
    use edgion_resources::resources::LocalObjectReference;

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
            }),
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
