//! Plugin runtime - manages plugin execution across different stages

use std::time::Duration;

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::filters::PluginRunningResult;
use crate::types::filters::PluginRunningResult::ErrTerminateRequest;
use crate::types::resources::{
    EdgionPlugin, GRPCRouteFilter, GRPCRouteFilterType, HTTPRouteFilter, HTTPRouteFilterType,
};
use crate::types::resources::{
    RequestFilterEntry, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry, UpstreamResponseFilterEntry,
};
use crate::types::EdgionHttpContext;

use super::conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseBodyFilter,
    ConditionalUpstreamResponseFilter,
};
use super::log::{PluginLog, StageLogs};
use super::session_adapter::PingoraSessionAdapter;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::core::plugins::edgion_plugins::all_endpoint_status::AllEndpointStatus;
use crate::core::plugins::edgion_plugins::bandwidth_limit::BandwidthLimit;
use crate::core::plugins::edgion_plugins::basic_auth::BasicAuth;
use crate::core::plugins::edgion_plugins::cors::Cors;
use crate::core::plugins::edgion_plugins::csrf::Csrf;
use crate::core::plugins::edgion_plugins::ctx_set::CtxSet;
use crate::core::plugins::edgion_plugins::direct_endpoint::DirectEndpoint;
use crate::core::plugins::edgion_plugins::dynamic_external_upstream::DynamicExternalUpstream;
use crate::core::plugins::edgion_plugins::dynamic_internal_upstream::DynamicInternalUpstream;
use crate::core::plugins::edgion_plugins::forward_auth::ForwardAuth;
use crate::core::plugins::edgion_plugins::ip_restriction::IpRestriction;
use crate::core::plugins::edgion_plugins::jwe_decrypt::JweDecrypt;
use crate::core::plugins::edgion_plugins::jwt_auth::JwtAuth;
use crate::core::plugins::edgion_plugins::key_auth::KeyAuth;
use crate::core::plugins::edgion_plugins::ldap_auth::LdapAuth;
use crate::core::plugins::edgion_plugins::mock::Mock;
use crate::core::plugins::edgion_plugins::openid_connect::OpenidConnect;
use crate::core::plugins::edgion_plugins::proxy_rewrite::ProxyRewrite;
use crate::core::plugins::edgion_plugins::rate_limit::RateLimit;
use crate::core::plugins::edgion_plugins::rate_limit_redis::RateLimitRedis;
use crate::core::plugins::edgion_plugins::real_ip::RealIp;
use crate::core::plugins::edgion_plugins::request_restriction::RequestRestriction;
use crate::core::plugins::edgion_plugins::response_rewrite::ResponseRewrite;
use crate::core::plugins::edgion_plugins::dsl::plugin::DslPlugin;
use crate::core::plugins::gapi_filters::extension_ref::DEFAULT_PLUGIN_REF_DEPTH;
use crate::core::plugins::gapi_filters::{
    DebugAccessLogToHeaderFilter, ExtensionRefFilter, RequestHeaderModifierFilter, RequestRedirectFilter,
    ResponseHeaderModifierFilter,
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
}

impl Clone for PluginRuntime {
    fn clone(&self) -> Self {
        // PluginRuntime is rebuilt from edgion_plugins during pre_parse, so clone creates empty runtime on purpose.
        Self::new()
    }
}

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
        }
    }

    pub fn from_httproute_filters(filters: &[HTTPRouteFilter], namespace: &str) -> Self {
        tracing::error!(
            "PluginRuntime: from_httproute_filters called with {} filters",
            filters.len()
        );
        let mut runtime = Self::new();
        runtime.add_from_httproute_filters(filters, namespace);
        runtime
    }

    pub fn add_from_httproute_filters(&mut self, filters: &[HTTPRouteFilter], namespace: &str) {
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
                HTTPRouteFilterType::ExtensionRef => {
                    if let Some(ext_ref) = &filter.extension_ref {
                        let max_depth = filter.extension_ref_max_depth.unwrap_or(DEFAULT_PLUGIN_REF_DEPTH);
                        let ext_filter = ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), max_depth);
                        self.add_request_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_body_filter(Box::new(ext_filter));
                    }
                }
                _ => {}
            }
        }
    }

    pub fn from_grpcroute_filters(filters: &[GRPCRouteFilter], namespace: &str) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_grpcroute_filters(filters, namespace);
        runtime
    }

    pub fn add_from_grpcroute_filters(&mut self, filters: &[GRPCRouteFilter], namespace: &str) {
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
                        let max_depth = filter.extension_ref_max_depth.unwrap_or(DEFAULT_PLUGIN_REF_DEPTH);
                        let ext_filter = ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), max_depth);
                        self.add_request_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_filter(Box::new(ext_filter.clone()));
                        self.add_upstream_response_body_filter(Box::new(ext_filter));
                    }
                }
                _ => {}
            }
        }
    }

    /// Add request filters from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalRequestFilter to support condition-based execution.
    /// Gateway API filters (via add_from_httproute_filters) are NOT wrapped to maintain compatibility.
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
                // Collect validation errors from plugin configs
                if let Some(error) = Self::get_plugin_validation_error(&entry.plugin) {
                    let plugin_name = Self::get_plugin_name(&entry.plugin);
                    errors.push(format!("requestPlugins[{}] ({}): {}", index, plugin_name, error));
                }

                if let Some(filter) = Self::create_request_filter_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalRequestFilter to support condition evaluation
                    let conditional_filter = ConditionalRequestFilter::new(filter, entry.conditions.clone());
                    self.add_request_filter(Box::new(conditional_filter));
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
                // Collect validation errors from plugin configs
                if let Some(error) = Self::get_plugin_validation_error(&entry.plugin) {
                    let plugin_name = Self::get_plugin_name(&entry.plugin);
                    errors.push(format!(
                        "upstreamResponseFilterPlugins[{}] ({}): {}",
                        index, plugin_name, error
                    ));
                }

                if let Some(filter) = Self::create_upstream_response_filter_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalUpstreamResponseFilter to support condition evaluation
                    let conditional_filter = ConditionalUpstreamResponseFilter::new(filter, entry.conditions.clone());
                    self.add_upstream_response_filter(Box::new(conditional_filter));
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
                // Collect validation errors from plugin configs
                if let Some(error) = Self::get_plugin_validation_error(&entry.plugin) {
                    let plugin_name = Self::get_plugin_name(&entry.plugin);
                    errors.push(format!(
                        "upstreamResponsePlugins[{}] ({}): {}",
                        index, plugin_name, error
                    ));
                }

                if let Some(filter) = Self::create_upstream_response_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalUpstreamResponse to support condition evaluation
                    let conditional_filter = ConditionalUpstreamResponse::new(filter, entry.conditions.clone());
                    self.add_upstream_response(Box::new(conditional_filter));
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
                // Collect validation errors from plugin configs
                if let Some(error) = Self::get_plugin_validation_error(&entry.plugin) {
                    let plugin_name = Self::get_plugin_name(&entry.plugin);
                    errors.push(format!(
                        "upstreamResponseBodyFilterPlugins[{}] ({}): {}",
                        index, plugin_name, error
                    ));
                }

                if let Some(filter) = Self::create_upstream_response_body_filter_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalUpstreamResponseBodyFilter to support condition evaluation
                    let conditional_filter =
                        ConditionalUpstreamResponseBodyFilter::new(filter, entry.conditions.clone());
                    self.add_upstream_response_body_filter(Box::new(conditional_filter));
                }
            }
        }

        errors
    }

    /// Create a RequestFilter instance from EdgionPlugin enum
    ///
    /// # Arguments
    /// * `plugin` - The EdgionPlugin configuration
    /// * `namespace` - The namespace of the EdgionPlugins resource
    fn create_request_filter_from_edgion(plugin: &EdgionPlugin, namespace: &str) -> Option<Box<dyn RequestFilter>> {
        match plugin {
            EdgionPlugin::RequestHeaderModifier(config) => {
                Some(Box::new(RequestHeaderModifierFilter::new(config.clone())))
            }
            EdgionPlugin::RequestRedirect(config) => Some(Box::new(RequestRedirectFilter::new(config.clone()))),
            EdgionPlugin::BasicAuth(config) => Some(Box::new(BasicAuth::new(config))),
            EdgionPlugin::Cors(config) => Some(Box::new(Cors::new(config))),
            EdgionPlugin::Csrf(config) => Some(Box::new(Csrf::new(config))),
            EdgionPlugin::IpRestriction(config) => Some(IpRestriction::create(config)),
            EdgionPlugin::JwtAuth(config) => Some(Box::new(JwtAuth::new(config, namespace.to_string()))),
            EdgionPlugin::JweDecrypt(config) => Some(Box::new(JweDecrypt::new(config, namespace.to_string()))),
            EdgionPlugin::KeyAuth(config) => Some(KeyAuth::create(config)),
            EdgionPlugin::LdapAuth(config) => Some(LdapAuth::create(config)),
            EdgionPlugin::Mock(config) => Some(Box::new(Mock::new(config))),
            EdgionPlugin::ProxyRewrite(config) => Some(Box::new(ProxyRewrite::new(config))),
            EdgionPlugin::RequestRestriction(config) => Some(RequestRestriction::create(config)),
            EdgionPlugin::RateLimit(config) => Some(RateLimit::create(config)),
            EdgionPlugin::RateLimitRedis(config) => Some(RateLimitRedis::create(config)),
            EdgionPlugin::CtxSet(config) => Some(CtxSet::create(config)),
            EdgionPlugin::DirectEndpoint(config) => Some(Box::new(DirectEndpoint::new(config))),
            EdgionPlugin::DynamicInternalUpstream(config) => Some(DynamicInternalUpstream::create(config)),
            EdgionPlugin::DynamicExternalUpstream(config) => Some(DynamicExternalUpstream::create(config)),
            EdgionPlugin::RealIp(config) => Some(RealIp::create(config)),
            EdgionPlugin::ForwardAuth(config) => Some(Box::new(ForwardAuth::new(config))),
            EdgionPlugin::AllEndpointStatus(config) => Some(Box::new(AllEndpointStatus::new(config))),
            EdgionPlugin::OpenidConnect(config) => Some(Box::new(OpenidConnect::new(config, namespace.to_string()))),
            EdgionPlugin::ExtensionRef(ext_ref) => {
                let ext_filter =
                    ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), DEFAULT_PLUGIN_REF_DEPTH);
                Some(Box::new(ext_filter))
            }
            EdgionPlugin::Dsl(config) => DslPlugin::from_config(config),
            _ => None,
        }
    }

    /// Create an UpstreamResponseFilter instance from EdgionPlugin enum
    fn create_upstream_response_filter_from_edgion(
        plugin: &EdgionPlugin,
        namespace: &str,
    ) -> Option<Box<dyn UpstreamResponseFilter>> {
        match plugin {
            EdgionPlugin::ResponseHeaderModifier(config) => {
                Some(Box::new(ResponseHeaderModifierFilter::new(config.clone())))
            }
            EdgionPlugin::DebugAccessLogToHeader(config) => Some(Box::new(DebugAccessLogToHeaderFilter::new(config))),
            EdgionPlugin::ResponseRewrite(config) => Some(Box::new(ResponseRewrite::new(config))),
            EdgionPlugin::ExtensionRef(ext_ref) => {
                let ext_filter =
                    ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), DEFAULT_PLUGIN_REF_DEPTH);
                Some(Box::new(ext_filter))
            }
            _ => None,
        }
    }

    /// Create an UpstreamResponse instance from EdgionPlugin enum
    fn create_upstream_response_from_edgion(
        _plugin: &EdgionPlugin,
        _namespace: &str,
    ) -> Option<Box<dyn UpstreamResponse>> {
        // Currently no plugins for this stage
        None
    }

    /// Create an UpstreamResponseBodyFilter instance from EdgionPlugin enum
    fn create_upstream_response_body_filter_from_edgion(
        plugin: &EdgionPlugin,
        namespace: &str,
    ) -> Option<Box<dyn UpstreamResponseBodyFilter>> {
        match plugin {
            EdgionPlugin::BandwidthLimit(config) => Some(BandwidthLimit::create(config)),
            EdgionPlugin::ExtensionRef(ext_ref) => {
                let ext_filter =
                    ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), DEFAULT_PLUGIN_REF_DEPTH);
                Some(Box::new(ext_filter))
            }
            _ => None,
        }
    }

    /// Get validation error from a plugin config (if any)
    fn get_plugin_validation_error(plugin: &EdgionPlugin) -> Option<String> {
        match plugin {
            EdgionPlugin::RateLimit(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::RateLimitRedis(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::CtxSet(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::DirectEndpoint(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::RequestRestriction(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::ProxyRewrite(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::ResponseRewrite(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::KeyAuth(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::LdapAuth(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::ForwardAuth(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::OpenidConnect(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::JweDecrypt(config) => config
                .get_validation_error()
                .map(|s| s.to_string())
                .or_else(|| config.detect_validation_error()),
            EdgionPlugin::BandwidthLimit(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::AllEndpointStatus(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::DynamicInternalUpstream(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::DynamicExternalUpstream(config) => config.get_validation_error().map(|s| s.to_string()),
            EdgionPlugin::Dsl(config) => config.get_validation_error_owned(),
            _ => None,
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
            EdgionPlugin::JwtAuth(_) => "JwtAuth",
            EdgionPlugin::JweDecrypt(_) => "JweDecrypt",
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
            EdgionPlugin::ForwardAuth(_) => "ForwardAuth",
            EdgionPlugin::OpenidConnect(_) => "OpenidConnect",
            EdgionPlugin::DebugAccessLogToHeader(_) => "DebugAccessLogToHeader",
            EdgionPlugin::ResponseRewrite(_) => "ResponseRewrite",
            EdgionPlugin::BandwidthLimit(_) => "BandwidthLimit",
            EdgionPlugin::AllEndpointStatus(_) => "AllEndpointStatus",
            EdgionPlugin::DynamicInternalUpstream(_) => "DynamicInternalUpstream",
            EdgionPlugin::DynamicExternalUpstream(_) => "DynamicExternalUpstream",
            EdgionPlugin::ExtensionRef(_) => "ExtensionRef",
            EdgionPlugin::UrlRewrite(_) => "UrlRewrite",
            EdgionPlugin::RequestMirror(_) => "RequestMirror",
            EdgionPlugin::Dsl(_) => "Dsl",
        }
    }

    fn add_request_filter(&mut self, filter: Box<dyn RequestFilter>) {
        self.request_plugins.push(filter);
    }

    fn add_upstream_response_filter(&mut self, filter: Box<dyn UpstreamResponseFilter>) {
        self.upstream_response_plugins.push(filter);
    }

    pub fn add_upstream_response_body_filter(&mut self, filter: Box<dyn UpstreamResponseBodyFilter>) {
        tracing::info!("PluginRuntime: adding body filter '{}'", filter.name());
        self.upstream_response_body_plugins.push(filter);
    }

    fn add_upstream_response(&mut self, filter: Box<dyn UpstreamResponse>) {
        self.upstream_response_async_plugins.push(filter);
    }

    /// Get total plugin count across all stages
    pub fn total_plugin_count(&self) -> usize {
        self.request_plugins.len()
            + self.upstream_response_plugins.len()
            + self.upstream_response_body_plugins.len()
            + self.upstream_response_async_plugins.len()
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

        let mut filter_logs = Vec::with_capacity(self.request_plugins.len());
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for filter in &self.request_plugins {
            let mut plugin_log = PluginLog::new(filter.name());
            let start = std::time::Instant::now();

            let result = filter.run_request(&mut session_adapter, &mut plugin_log).await;

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost = Some(start.elapsed().as_micros() as u64);
            }
            filter_logs.push(plugin_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                break;
            } else if let PluginRunningResult::ErrResponse { status, body } = result {
                if let Ok(status_code) = http::StatusCode::from_u16(status) {
                    if let Ok(mut resp) = ResponseHeader::build(status_code, Some(2)) {
                        let _ = resp.insert_header("content-type", "text/plain");
                        let body_bytes = body.map(bytes::Bytes::from);
                        let len = body_bytes.as_ref().map(|b| b.len()).unwrap_or(0);
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
        // Apply queued response headers from request stage
        // Use a temporary vector to avoid borrowing issues if we iterate ctx directly while modifying response_header
        // But ctx and response_header are separate arguments.
        // wait, drain requires &mut ctx.
        if !ctx.response_headers_to_add.is_empty() {
            let headers = std::mem::take(&mut ctx.response_headers_to_add);
            for (name, value) in headers {
                let _ = response_header.insert_header(name, value);
            }
        }

        if self.upstream_response_plugins.is_empty() {
            return;
        }

        let mut filter_logs = Vec::with_capacity(self.upstream_response_plugins.len());
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_plugins {
            let mut plugin_log = PluginLog::new(filter.name());
            let start = std::time::Instant::now();

            let result = filter.run_upstream_response_filter(&mut session_adapter, &mut plugin_log);

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost = Some(start.elapsed().as_micros() as u64);
            }
            filter_logs.push(plugin_log);

            if ErrTerminateRequest == result {
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
    /// NOTE: Unlike other stages, this does NOT log per-chunk execution to avoid
    /// excessive logging overhead (body filter is called for every chunk).
    /// The first invocation logs stage info only.
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

        // Create a session adapter for condition evaluation
        // Use a scoped borrow to avoid lifetime issues with ctx
        {
            let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

            for filter in &self.upstream_response_body_plugins {
                let mut plugin_log = PluginLog::new(filter.name());

                let delay = filter.run_upstream_response_body_filter(
                    body,
                    end_of_stream,
                    &mut session_adapter,
                    &mut plugin_log,
                );

                // Take the largest delay (most restrictive rate limit)
                if let Some(d) = delay {
                    max_delay = Some(match max_delay {
                        Some(current) => current.max(d),
                        None => d,
                    });
                }
            }
        }
        // session_adapter dropped here, ctx borrow released

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
            let mut plugin_log = PluginLog::new(filter.name());
            let start = std::time::Instant::now();

            let result = filter
                .run_upstream_response(&mut session_adapter, &mut plugin_log)
                .await;

            // Skip time_cost for ExtensionRef (identified by refer_to being set)
            if plugin_log.refer_to.is_none() {
                plugin_log.time_cost = Some(start.elapsed().as_micros() as u64);
            }
            filter_logs.push(plugin_log);

            if ErrTerminateRequest == result {
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
