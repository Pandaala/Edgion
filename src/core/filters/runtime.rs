//! Filter runtime - manages filter execution across different stages

use pingora_proxy::Session;
use std::collections::HashMap;

use crate::types::EdgionHttpContext;
use crate::types::filters::{FilterConf, FilterRunningStage};
use crate::types::filters::FilterRunningResult::ErrTerminateRequest;
use crate::types::resources::{HTTPRouteFilter, HTTPRouteFilterType};

use super::filter_log::FilterLog;
use super::session_adapter::PingoraSessionAdapter;
use super::standard::RequestHeaderModifierFilter;
use super::traits::Filter;

/// Runtime for executing filters at different stages
pub struct FilterRuntime {
    request_filter: Vec<Box<dyn Filter>>,
}

impl FilterRuntime {
    pub fn new(filter_conf_list: &Option<HashMap<FilterRunningStage, Vec<FilterConf>>>) -> Self {
        let mut runtime = FilterRuntime {
            request_filter: vec![],
        };

        if let Some(filter_conf_list) = filter_conf_list {
            for (stage, filter_list) in filter_conf_list {
                match stage {
                    FilterRunningStage::Request => {
                        for filter_conf in filter_list {
                            let filter = get_filter_from_filter_conf(filter_conf);
                            runtime.request_filter.push(filter);
                        }
                    }
                    _ => {
                        // TODO: Implement other stage filters
                    }
                }
            }
        }

        runtime
    }

    /// Create FilterRuntime from HTTPRouteFilter list
    pub fn new_from_httproute_filters(filters: &[HTTPRouteFilter]) -> Self {
        let mut runtime = FilterRuntime {
            request_filter: vec![],
        };

        for filter in filters {
            if let Some(f) = Self::create_filter(filter) {
                runtime.request_filter.push(f);
            }
        }

        runtime
    }

    /// Create a Filter instance from HTTPRouteFilter
    fn create_filter(filter: &HTTPRouteFilter) -> Option<Box<dyn Filter>> {
        match filter.filter_type {
            HTTPRouteFilterType::RequestHeaderModifier => {
                filter.request_header_modifier.as_ref().map(|config| {
                    Box::new(RequestHeaderModifierFilter::new(config.clone())) as Box<dyn Filter>
                })
            }
            // TODO: Add other filter types
            _ => None,
        }
    }

    /// Get total filter count across all stages
    pub fn total_filter_count(&self) -> usize {
        self.request_filter.len()
    }

    pub async fn run_request_filters(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        for filter in &self.request_filter {
            let mut filter_log = FilterLog::new(filter.name());

            let mut session_adapter = PingoraSessionAdapter::new(
                s,
                FilterRunningStage::Request,
            );

            let result = filter.run(&mut session_adapter, &mut filter_log).await;
            ctx.filter_logs.push(filter_log);

            if ErrTerminateRequest == result {
                ctx.filter_running_result = ErrTerminateRequest;
                return;
            }

            // Apply request header modifications after each filter
            session_adapter.apply_request_header_modifications();
        }
    }
}

fn get_filter_from_filter_conf(filter_conf: &FilterConf) -> Box<dyn Filter> {
    match filter_conf.name.as_str() {
        // "BasicAuth" => BasicAuth::new_filter(filter_conf),
        _ => todo!(),
    }
}
