//! Plugin runtime - manages plugin execution across different stages

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::filters::PluginRunningResult::ErrTerminateRequest;
use crate::types::resources::{
    EdgionPlugin, GRPCRouteFilter, GRPCRouteFilterType, HTTPRouteFilter, HTTPRouteFilterType,
};
use crate::types::resources::{RequestFilterEntry, UpstreamResponseEntry, UpstreamResponseFilterEntry};
use crate::types::EdgionHttpContext;

use super::conditional_filter::{
    ConditionalRequestFilter, ConditionalUpstreamResponse, ConditionalUpstreamResponseFilter,
};
use super::log::{PluginLog, StageLogs};
use super::session_adapter::PingoraSessionAdapter;
use super::traits::{RequestFilter, UpstreamResponse, UpstreamResponseFilter};
use crate::core::plugins::edgion_plugins::basic_auth::BasicAuth;
use crate::core::plugins::edgion_plugins::cors::Cors;
use crate::core::plugins::edgion_plugins::csrf::Csrf;
use crate::core::plugins::edgion_plugins::ip_restriction::IpRestriction;
use crate::core::plugins::edgion_plugins::mock::Mock;
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
            upstream_response_async_plugins: vec![],
        }
    }

    pub fn from_httproute_filters(filters: &[HTTPRouteFilter], namespace: &str) -> Self {
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
                        self.add_upstream_response_filter(Box::new(ext_filter));
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
                        self.add_upstream_response_filter(Box::new(ext_filter));
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
    pub fn add_from_request_filters(&mut self, entries: &[RequestFilterEntry], namespace: &str) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_request_filter_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalRequestFilter to support condition evaluation
                    let conditional_filter =
                        ConditionalRequestFilter::new(filter, entry.conditions.clone());
                    self.add_request_filter(Box::new(conditional_filter));
                }
            }
        }
    }

    /// Add upstream response filters from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalUpstreamResponseFilter to support condition-based execution.
    pub fn add_from_upstream_response_filters(&mut self, entries: &[UpstreamResponseFilterEntry], namespace: &str) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_upstream_response_filter_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalUpstreamResponseFilter to support condition evaluation
                    let conditional_filter =
                        ConditionalUpstreamResponseFilter::new(filter, entry.conditions.clone());
                    self.add_upstream_response_filter(Box::new(conditional_filter));
                }
            }
        }
    }

    /// Add upstream response handlers from entries (only enabled)
    ///
    /// Filters are wrapped with ConditionalUpstreamResponse to support condition-based execution.
    pub fn add_from_upstream_responses(&mut self, entries: &[UpstreamResponseEntry], namespace: &str) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_upstream_response_from_edgion(&entry.plugin, namespace) {
                    // Wrap with ConditionalUpstreamResponse to support condition evaluation
                    let conditional_filter =
                        ConditionalUpstreamResponse::new(filter, entry.conditions.clone());
                    self.add_upstream_response(Box::new(conditional_filter));
                }
            }
        }
    }

    /// Create a RequestFilter instance from EdgionPlugin enum
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
            EdgionPlugin::Mock(config) => Some(Box::new(Mock::new(config))),
            EdgionPlugin::ExtensionRef(ext_ref) => {
                let ext_filter =
                    ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone(), DEFAULT_PLUGIN_REF_DEPTH);
                Some(Box::new(ext_filter))
            }
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

    fn add_request_filter(&mut self, filter: Box<dyn RequestFilter>) {
        self.request_plugins.push(filter);
    }

    fn add_upstream_response_filter(&mut self, filter: Box<dyn UpstreamResponseFilter>) {
        self.upstream_response_plugins.push(filter);
    }

    fn add_upstream_response(&mut self, filter: Box<dyn UpstreamResponse>) {
        self.upstream_response_async_plugins.push(filter);
    }

    /// Get total plugin count across all stages
    pub fn total_plugin_count(&self) -> usize {
        self.request_plugins.len() + self.upstream_response_plugins.len() + self.upstream_response_async_plugins.len()
    }

    /// Get request stage plugin count
    pub fn request_plugins_count(&self) -> usize {
        self.request_plugins.len()
    }

    /// Get upstream_response_filter stage plugin count (sync)
    pub fn upstream_response_plugins_count(&self) -> usize {
        self.upstream_response_plugins.len()
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
