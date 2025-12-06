//! Filter runtime - manages filter execution across different stages

use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::EdgionHttpContext;
use crate::types::filters::FilterRunningStage;
use crate::types::filters::FilterRunningResult::ErrTerminateRequest;
use crate::types::resources::{HTTPRouteFilter, HTTPRouteFilterType};

use super::filter_log::FilterLog;
use super::session_adapter::PingoraSessionAdapter;
use super::standard::{RequestHeaderModifierFilter, ResponseHeaderModifierFilter};
use super::traits::Filter;

pub struct FilterRuntime {
    request_filters: Vec<Box<dyn Filter>>,
    response_filters: Vec<Box<dyn Filter>>,
    response_header_filters: Vec<Box<dyn Filter>>,
}

impl FilterRuntime {
    pub fn new() -> Self {
        Self {
            request_filters: vec![],
            response_filters: vec![],
            response_header_filters: vec![],
        }
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
                FilterRunningStage::Request | FilterRunningStage::EarlyRequest => {
                    self.request_filters.push(filter);
                }
                FilterRunningStage::Response => {
                    self.response_filters.push(filter);
                }
                FilterRunningStage::ResponseHeader => {
                    self.response_header_filters.push(filter);
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
            // TODO: Add other filter types
            _ => None,
        }
    }

    /// Get total filter count across all stages
    pub fn total_filter_count(&self) -> usize {
        self.request_filters.len() 
            + self.response_filters.len() 
            + self.response_header_filters.len()
    }

    pub async fn run_request_filters(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        let mut session_adapter = PingoraSessionAdapter::new(s, ctx);

        for filter in &self.request_filters {
            let mut filter_log = FilterLog::new(filter.name());

            let result = filter.run(
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

    pub async fn run_response_header_filters(
        &self,
        s: &mut Session,
        ctx: &mut EdgionHttpContext,
        response_header: &mut ResponseHeader,
    ) {
        let mut session_adapter = PingoraSessionAdapter::with_response_header(s, ctx, response_header);

        for filter in &self.response_header_filters {
            let mut filter_log = FilterLog::new(filter.name());

            let result = filter.run(
                FilterRunningStage::ResponseHeader,
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
