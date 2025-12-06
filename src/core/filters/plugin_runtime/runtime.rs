//! Plugin runtime - manages plugin execution across different stages

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::EdgionHttpContext;
use crate::types::filters::PluginRunningStage;
use crate::types::filters::PluginRunningResult::ErrTerminateRequest;
use crate::types::resources::{HTTPRouteFilter, HTTPRouteFilterType};

use super::log::PluginLog;
use crate::core::filters::gapi_filters::{RequestHeaderModifierFilter, RequestRedirectFilter, ResponseHeaderModifierFilter};
use super::session_adapter::PingoraSessionAdapter;
use super::traits::Plugin;

pub struct PluginRuntime {
    /// Plugins for request stage (async)
    request_plugins: Vec<Box<dyn Plugin>>,
    /// Plugins for upstream_response_filter stage (sync)
    upstream_response_plugins: Vec<Box<dyn Plugin>>,
    /// Plugins for response_filter stage (async)
    upstream_response_async_plugins: Vec<Box<dyn Plugin>>,
}

impl Clone for PluginRuntime {
    fn clone(&self) -> Self {
        // PluginRuntime is rebuilt from plugins during pre_parse, so clone creates empty
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

    pub fn from_httproute_filters(filters: &[HTTPRouteFilter]) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_httproute_filters(filters);
        runtime
    }

    pub fn add_from_httproute_filters(&mut self, filters: &[HTTPRouteFilter]) {
        for filter in filters {
            if let Some(p) = Self::create_plugin(filter) {
                self.add_plugin(p);
            }
        }
    }

    fn add_plugin(&mut self, plugin: Box<dyn Plugin>) {
        if let Some(stage) = plugin.get_stages().first() {
            match stage {
                PluginRunningStage::Request => {
                    self.request_plugins.push(plugin);
                }
                PluginRunningStage::UpstreamResponseFilter => {
                    self.upstream_response_plugins.push(plugin);
                }
                PluginRunningStage::UpstreamResponse => {
                    self.upstream_response_async_plugins.push(plugin);
                }
            }
        }
    }

    fn create_plugin(filter: &HTTPRouteFilter) -> Option<Box<dyn Plugin>> {
        match filter.filter_type {
            HTTPRouteFilterType::RequestHeaderModifier => {
                filter.request_header_modifier.as_ref().map(|config| {
                    Box::new(RequestHeaderModifierFilter::new(config.clone())) as Box<dyn Plugin>
                })
            }
            HTTPRouteFilterType::ResponseHeaderModifier => {
                filter.response_header_modifier.as_ref().map(|config| {
                    Box::new(ResponseHeaderModifierFilter::new(config.clone())) as Box<dyn Plugin>
                })
            }
            HTTPRouteFilterType::RequestRedirect => {
                filter.request_redirect.as_ref().map(|config| {
                    Box::new(RequestRedirectFilter::new(config.clone())) as Box<dyn Plugin>
                })
            }
            // TODO: Add other plugin types
            _ => None,
        }
    }

    /// Get total plugin count across all stages
    pub fn total_plugin_count(&self) -> usize {
        self.request_plugins.len()
            + self.upstream_response_plugins.len()
            + self.upstream_response_async_plugins.len()
    }

    /// Run request stage plugins (async)
    pub async fn run_request_plugins(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for plugin in &self.request_plugins {
            let mut plugin_log = PluginLog::new(plugin.name());

            let result = plugin.run_async(
                PluginRunningStage::Request,
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

    /// Run upstream_response_filter stage plugins (sync)
    pub fn run_upstream_response_plugins_sync(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for plugin in &self.upstream_response_plugins {
            let mut plugin_log = PluginLog::new(plugin.name());

            let result = plugin.run_sync(
                PluginRunningStage::UpstreamResponseFilter,
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

    /// Run response_filter stage plugins (async)
    pub async fn run_upstream_response_plugins_async(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for plugin in &self.upstream_response_async_plugins {
            let mut plugin_log = PluginLog::new(plugin.name());

            let result = plugin.run_async(
                PluginRunningStage::UpstreamResponse,
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
