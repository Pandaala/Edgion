//! Filter runtime - manages filter execution across different stages

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::EdgionHttpContext;
use crate::types::filters::FilterRunningStage;
use crate::types::filters::FilterRunningResult::ErrTerminateRequest;
use crate::types::resources::{HTTPRouteFilter, HTTPRouteFilterType};

use super::filter_log::FilterLog;
use crate::core::filters::gapi_filters::{RequestHeaderModifierFilter, RequestRedirectFilter, ResponseHeaderModifierFilter};
use super::session_adapter::PingoraSessionAdapter;
use super::traits::Filter;

pub struct FilterRuntime {
    /// Filters for request_filter stage (async)
    request_filters: Vec<Box<dyn Filter>>,
    /// Filters for upstream_response_filter stage (sync)
    upstream_response_filters: Vec<Box<dyn Filter>>,
    /// Filters for response_filter stage (async)
    upstream_response_async_filters: Vec<Box<dyn Filter>>,
}

impl Clone for FilterRuntime {
    fn clone(&self) -> Self {
        // FilterRuntime is rebuilt from filters during pre_parse, so clone creates empty
        Self::new()
    }
}

impl std::fmt::Debug for FilterRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterRuntime")
            .field("request_filters_count", &self.request_filters.len())
            .field("upstream_response_filters_count", &self.upstream_response_filters.len())
            .field("upstream_response_async_filters_count", &self.upstream_response_async_filters.len())
            .finish()
    }
}

impl Default for FilterRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterRuntime {
    pub fn new() -> Self {
        Self {
            request_filters: vec![],
            upstream_response_filters: vec![],
            upstream_response_async_filters: vec![],
        }
    }

    pub fn from_httproute_filters(filters: &[HTTPRouteFilter]) -> Self {
        let mut runtime = Self::new();
        runtime.add_from_httproute_filters(filters);
        runtime
    }

    pub fn add_from_httproute_filters(&mut self, filters: &[HTTPRouteFilter]) {
        for filter in filters {
            if let Some(f) = Self::create_filter(filter) {
                self.add_filter(f);
            }
        }
    }

    fn add_filter(&mut self, filter: Box<dyn Filter>) {
        if let Some(stage) = filter.get_stages().first() {
            match stage {
                FilterRunningStage::Request => {
                    self.request_filters.push(filter);
                }
                FilterRunningStage::UpstreamResponseFilter => {
                    self.upstream_response_filters.push(filter);
                }
                FilterRunningStage::UpstreamResponse => {
                    self.upstream_response_async_filters.push(filter);
                }
            }
        }
    }

    fn create_filter(filter: &HTTPRouteFilter) -> Option<Box<dyn Filter>> {
        match filter.filter_type {
            HTTPRouteFilterType::RequestHeaderModifier => {
                filter.request_header_modifier.as_ref().map(|config| {
                    Box::new(RequestHeaderModifierFilter::new(config.clone())) as Box<dyn Filter>
                })
            }
            HTTPRouteFilterType::ResponseHeaderModifier => {
                filter.response_header_modifier.as_ref().map(|config| {
                    Box::new(ResponseHeaderModifierFilter::new(config.clone())) as Box<dyn Filter>
                })
            }
            HTTPRouteFilterType::RequestRedirect => {
                filter.request_redirect.as_ref().map(|config| {
                    Box::new(RequestRedirectFilter::new(config.clone())) as Box<dyn Filter>
                })
            }
            // TODO: Add other filter types
            _ => None,
        }
    }

    /// Get total filter count across all stages
    pub fn total_filter_count(&self) -> usize {
        self.request_filters.len() 
            + self.upstream_response_filters.len() 
            + self.upstream_response_async_filters.len()
    }

    /// Run request_filter stage filters (async)
    pub async fn run_request_filters(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for filter in &self.request_filters {
            let mut filter_log = FilterLog::new(filter.name());

            let result = filter.run_async(
                FilterRunningStage::Request,
                &mut session_adapter,
                &mut filter_log,
            ).await;
            session_adapter.push_filter_log(filter_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }

    /// Run upstream_response_filter stage filters (sync)
    pub fn run_upstream_response_filters_sync(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_filters {
            let mut filter_log = FilterLog::new(filter.name());

            let result = filter.run_sync(
                FilterRunningStage::UpstreamResponseFilter,
                &mut session_adapter,
                &mut filter_log,
            );
            session_adapter.push_filter_log(filter_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }

    /// Run response_filter stage filters (async)
    pub async fn run_upstream_response_filters_async(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.upstream_response_async_filters {
            let mut filter_log = FilterLog::new(filter.name());

            let result = filter.run_async(
                FilterRunningStage::UpstreamResponse,
                &mut session_adapter,
                &mut filter_log,
            ).await;
            session_adapter.push_filter_log(filter_log);

            if ErrTerminateRequest == result {
                session_adapter.set_terminate();
                return;
            }
        }
    }
}

