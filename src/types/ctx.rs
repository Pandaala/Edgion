use std::sync::Arc;
use std::time::Instant;
use std::fmt;
use serde::Serialize;
use crate::types::{EdgionStatus, HTTPBackendRef, HTTPRouteMatch};
use crate::types::filters::{PluginRunningResult};
use crate::core::filters::PluginLog;
use crate::core::routes::HttpRouteRuleUnit;

#[derive(Clone)]
pub struct MatchInfo {
    /// route namespace
    pub rns: String,
    /// route name
    pub rn: String,

    /// Rule id in HTTPROUTE
    pub rule_id: usize,
    /// Match id at rule id,
    pub match_id: usize,

    /// match item
    pub m: HTTPRouteMatch,
}

impl MatchInfo {
    pub fn new(rns: String,
               rn: String,
               rule_id: usize,
               match_id: usize,
               m: HTTPRouteMatch) -> Self {
        Self { rns, rn, m, rule_id, match_id }
    }
}

impl fmt::Display for MatchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{} (rule:{}, match:{})", self.rns, self.rn, self.rule_id, self.match_id)
    }
}

/// Request information extracted from the incoming request
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestInfo {
    /// Trace ID from x-trace-id header
    #[serde(rename = "x-trace-id")]
    pub x_trace_id: Option<String>,
    /// Hostname from the Host header
    #[serde(rename = "host")]
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
    
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Request information (hostname, path, x-trace-id)
    pub request_info: RequestInfo,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionStatus>,
    
    /// Matched route unit containing full route information
    pub route_unit: Option<Arc<HttpRouteRuleUnit>>,
    
    /// Selected backend from load balancing
    pub selected_backend: Option<HTTPBackendRef>,
    
    /// Upstream info after peer selection
    pub upstream_info: Option<UpstreamInfo>,
    
    /// Plugin execution logs
    pub plugin_logs: Vec<PluginLog>,
    
    /// Plugin running result
    pub plugin_running_result: PluginRunningResult,
}

impl EdgionHttpContext {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            request_id: None,
            auto_gprc: false,
            request_info: RequestInfo::default(),
            error_codes: Vec::with_capacity(5),
            route_unit: None,
            selected_backend: None,
            upstream_info: None,
            plugin_logs: Vec::with_capacity(10),
            plugin_running_result: PluginRunningResult::Nothing,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.error_codes.push(err_code);
    }
}
