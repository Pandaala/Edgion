use std::sync::Arc;
use std::time::Instant;
use crate::types::{EdgionStatus, HTTPBackendRef, HTTPRouteMatch};
use crate::types::filters::{FilterRunningResult};
use crate::core::filters::filter_log::FilterLog;
use crate::core::filters::FilterRuntime;

#[derive(Clone)]
pub struct MatchInfo {
    /// route namespace
    pub rns: String,
    /// route name
    pub rn: String,
    /// match item
    pub m: HTTPRouteMatch,
    /// Rule-level filter runtime
    pub rule_filter_runtime: Arc<FilterRuntime>,
}

impl MatchInfo {
    pub fn new(rns: String, rn: String, m: HTTPRouteMatch, rule_filter_runtime: Arc<FilterRuntime>) -> Self {
        Self { rns, rn, m, rule_filter_runtime }
    }
}

/// Request information extracted from the incoming request
#[derive(Debug, Clone, Default)]
pub struct RequestInfo {
    /// Hostname from the Host header
    pub hostname: String,
    /// Request path from URI
    pub path: String,
    /// Response status code (e.g., 200, 400, 404, 500)
    pub status: u16,
}

/// Upstream backend information
#[derive(Debug, Clone, Default)]
pub struct UpstreamInfo {
    /// Backend service name
    pub name: String,
    /// Backend service namespace
    pub namespace: String,
    /// Actual peer address (ip:port)
    pub peer: String,
}

pub struct EdgionHttpContext {
    /// Request start time for latency calculation
    pub start_time: Instant,
    
    pub x_trace_id: Option<String>,
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Request information (hostname, path)
    pub request_info: RequestInfo,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionStatus>,

    /// Matched route info (namespace, name, match item)
    pub matched_info: Option<Arc<MatchInfo>>,
    
    /// Selected backend from load balancing
    pub selected_backend: Option<HTTPBackendRef>,
    
    /// Upstream info after peer selection
    pub upstream_info: Option<UpstreamInfo>,
    
    /// Filter execution logs
    pub filter_logs: Vec<FilterLog>,
    
    /// Filter running result
    pub filter_running_result: FilterRunningResult,
}

impl EdgionHttpContext {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            x_trace_id: None,
            request_id: None,
            auto_gprc: false,
            request_info: RequestInfo::default(),
            error_codes: Vec::with_capacity(5),
            matched_info: None,
            selected_backend: None,
            upstream_info: None,
            filter_logs: Vec::with_capacity(10),
            filter_running_result: FilterRunningResult::Nothing,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.error_codes.push(err_code);
    }
}
