pub mod filter_log;
pub mod session_adapter;
pub mod traits;

pub use traits::{Filter, FilterSession, FilterSessionError, FilterSessionResult};
#[cfg(test)]
pub use traits::MockFilterSession;

use pingora_proxy::Session;
use std::collections::HashMap;

use crate::types::EdgionHttpContext;
use crate::core::filters::session_adapter::PingoraSessionAdapter;
use crate::core::filters::filter_log::FilterLog;
use crate::types::filters::{FilterConf, FilterRunningStage};
use crate::types::filters::FilterRunningResult::ErrTerminateRequest;

pub struct FilterRuntime {
    early_request_filter: Vec<Box<dyn Filter>>,
    request_filter: Vec<Box<dyn Filter>>,
}

impl FilterRuntime {
    pub fn new(filter_conf_list: &Option<HashMap<FilterRunningStage, Vec<FilterConf>>>) -> Self {
        let mut runtime = FilterRuntime {
            request_filter: vec![],
            early_request_filter: vec![],
        };

        if let Some(filter_conf_list) = filter_conf_list {
            for (stage, filter_list) in filter_conf_list {
                match stage {
                    FilterRunningStage::EarlyRequest => {
                        for filter_conf in filter_list {
                            let filter = get_filter_from_filter_conf(&filter_conf);
                            runtime.early_request_filter.push(filter);
                        }
                    }
                    FilterRunningStage::Request => {
                        for filter_conf in filter_list {
                            let filter = get_filter_from_filter_conf(&filter_conf);
                            runtime.request_filter.push(filter);
                        }
                    }
                    FilterRunningStage::Response => {
                        // TODO: Implement response stage filters
                    }
                    FilterRunningStage::ResponseHeader => {
                        // TODO: Implement response header stage filters
                    }
                }
            }
        }

        runtime
    }

    /// Get total filter count across all stages
    pub fn total_filter_count(&self) -> usize {
        self.early_request_filter.len() + self.request_filter.len()
    }

    pub async fn run_early_request_filters(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        use std::time::Instant;

        for filter in &self.early_request_filter {
            let filter_name = filter.name();
            let start_time = Instant::now();

            let mut session_adapter = PingoraSessionAdapter::new(
                s,
                FilterRunningStage::EarlyRequest,
            );

            let result = filter.run(&mut session_adapter).await;
            let timecost = start_time.elapsed();

            // Collect misc logs and create filter log entry
            let misc_logs = session_adapter.take_misc_logs();
            let log = if misc_logs.is_empty() {
                FilterLog::new(filter_name, timecost)
            } else {
                FilterLog::log(filter_name, timecost, misc_logs.join("; "))
            };
            ctx.filter_logs.push(log);

            if ErrTerminateRequest == result {
                ctx.filter_running_result = ErrTerminateRequest;
                return;
            }
        }
    }

    pub async fn run_request_filters(&self, s: &mut Session, ctx: &mut EdgionHttpContext) {
        use std::time::Instant;

        for filter in &self.request_filter {
            let filter_name = filter.name();
            let start_time = Instant::now();

            let mut session_adapter = PingoraSessionAdapter::new(
                s,
                FilterRunningStage::Request,
            );

            let result = filter.run(&mut session_adapter).await;
            let timecost = start_time.elapsed();

            // Collect misc logs and create filter log entry
            let misc_logs = session_adapter.take_misc_logs();
            let log = if misc_logs.is_empty() {
                FilterLog::new(filter_name, timecost)
            } else {
                FilterLog::log(filter_name, timecost, misc_logs.join("; "))
            };
            ctx.filter_logs.push(log);

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
        &_ => todo!(),
    }
}