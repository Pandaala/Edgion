//! Plugin runtime - manages plugin execution across different stages

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::EdgionHttpContext;
use crate::types::filters::PluginRunningResult::ErrTerminateRequest;
use crate::types::resources::{HTTPRouteFilter, HTTPRouteFilterType, GRPCRouteFilter, GRPCRouteFilterType, EdgionPlugin};
use crate::types::resources::{RequestFilterEntry, UpstreamResponseFilterEntry, UpstreamResponseEntry};

use super::log::PluginLog;
use super::filters::{RequestFilter, UpstreamResponseFilter, UpstreamResponse};
use crate::core::plugins::gapi_filters::{ExtensionRefFilter, RequestHeaderModifierFilter, RequestRedirectFilter, ResponseHeaderModifierFilter};
use crate::core::plugins::edgion_plugins::basic_auth::BasicAuth;
use crate::core::plugins::edgion_plugins::cors::Cors;
use crate::core::plugins::edgion_plugins::csrf::Csrf;
use crate::core::plugins::edgion_plugins::ip_restriction::IpRestriction;
use crate::core::plugins::edgion_plugins::mock::Mock;
use super::session_adapter::PingoraSessionAdapter;

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
        // PluginRuntime is rebuilt from edgion_plugins during pre_parse, so clone creates empty
        Self::new()
    }
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("request_plugins_count", &self.request_plugins.len())
            .field("upstream_response_plugins_count", &self.upstream_response_plugins.len())
            .field("upstream_response_async_plugins_count", &self.upstream_response_async_plugins.len())
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
                        self.add_request_filter(Box::new(ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone())));
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
                        self.add_upstream_response_filter(Box::new(ResponseHeaderModifierFilter::new_from_grpc(config.clone())));
                    }
                }
                GRPCRouteFilterType::ExtensionRef => {
                    if let Some(ext_ref) = &filter.extension_ref {
                        self.add_request_filter(Box::new(ExtensionRefFilter::new(namespace.to_string(), ext_ref.clone())));
                    }
                }
                _ => {}
            }
        }
    }

    /// Add request filters from entries (only enabled)
    pub fn add_from_request_filters(&mut self, entries: &[RequestFilterEntry]) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_request_filter_from_edgion(&entry.plugin) {
                    self.add_request_filter(filter);
                }
            }
        }
    }

    /// Add upstream response filters from entries (only enabled)
    pub fn add_from_upstream_response_filters(&mut self, entries: &[UpstreamResponseFilterEntry]) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_upstream_response_filter_from_edgion(&entry.plugin) {
                    self.add_upstream_response_filter(filter);
                }
            }
        }
    }

    /// Add upstream response handlers from entries (only enabled)
    pub fn add_from_upstream_responses(&mut self, entries: &[UpstreamResponseEntry]) {
        for entry in entries {
            if entry.is_enabled() {
                if let Some(filter) = Self::create_upstream_response_from_edgion(&entry.plugin) {
                    self.add_upstream_response(filter);
                }
            }
        }
    }

    /// Create a RequestFilter instance from EdgionPlugin enum
    fn create_request_filter_from_edgion(plugin: &EdgionPlugin) -> Option<Box<dyn RequestFilter>> {
        match plugin {
            EdgionPlugin::RequestHeaderModifier(config) => {
                Some(Box::new(RequestHeaderModifierFilter::new(config.clone())))
            }
            EdgionPlugin::RequestRedirect(config) => {
                Some(Box::new(RequestRedirectFilter::new(config.clone())))
            }
            EdgionPlugin::BasicAuth(config) => {
                Some(Box::new(BasicAuth::new(config)))
            }
            EdgionPlugin::Cors(config) => {
                Some(Box::new(Cors::new(config)))
            }
            EdgionPlugin::Csrf(config) => {
                Some(Box::new(Csrf::new(config)))
            }
            EdgionPlugin::IpRestriction(config) => {
                Some(IpRestriction::new(config))
            }
            EdgionPlugin::Mock(config) => {
                Some(Box::new(Mock::new(config)))
            }
            _ => None,
        }
    }

    /// Create an UpstreamResponseFilter instance from EdgionPlugin enum
    fn create_upstream_response_filter_from_edgion(plugin: &EdgionPlugin) -> Option<Box<dyn UpstreamResponseFilter>> {
        match plugin {
            EdgionPlugin::ResponseHeaderModifier(config) => {
                Some(Box::new(ResponseHeaderModifierFilter::new(config.clone())))
            }
            _ => None,
        }
    }

    /// Create an UpstreamResponse instance from EdgionPlugin enum
    fn create_upstream_response_from_edgion(_plugin: &EdgionPlugin) -> Option<Box<dyn UpstreamResponse>> {
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
        self.request_plugins.len()
            + self.upstream_response_plugins.len()
            + self.upstream_response_async_plugins.len()
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
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for filter in &self.request_plugins {
            let mut plugin_log = PluginLog::new(filter.name());

            let result = filter.run_request(
                &mut session_adapter,
                &mut plugin_log,
            ).await;
            session_adapter.push_plugin_log(plugin_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }

    /// Run upstream_response_filter stage filters (sync)
    pub fn run_upstream_response_plugins_sync(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_plugins {
            let mut plugin_log = PluginLog::new(filter.name());

            let result = filter.run_upstream_response_filter(
                &mut session_adapter,
                &mut plugin_log,
            );
            session_adapter.push_plugin_log(plugin_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }

    /// Run response_filter stage filters (async)
    pub async fn run_upstream_response_plugins_async(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_async_plugins {
            let mut plugin_log = PluginLog::new(filter.name());

            let result = filter.run_upstream_response(
                &mut session_adapter,
                &mut plugin_log,
            ).await;
            session_adapter.push_plugin_log(plugin_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }
}
