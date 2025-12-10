use std::sync::Arc;
use std::time::Instant;
use std::fmt;
use serde::Serialize;
use crate::types::{EdgionStatus, HTTPBackendRef, HTTPRouteMatch};
use crate::types::filters::{PluginRunningResult};
use crate::core::filters::PluginLog;
use crate::core::routes::HttpRouteRuleUnit;

#[derive(Clone, Serialize)]
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
    #[serde(skip)]
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
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpstreamInfo {
    /// Backend service name
    pub name: String,
    /// Backend service namespace
    pub namespace: String,
    /// Peer IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Peer port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// HTTP status code for this upstream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
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
    
    /// Upstream info history (can be modified during request processing)
    pub upstream_info: Vec<UpstreamInfo>,
    
    /// Current upstream_id (index in upstream_info Vec) for status setting
    pub current_upstream_id: Option<usize>,
    
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
            upstream_info: Vec::with_capacity(5),
            current_upstream_id: None,
            plugin_logs: Vec::with_capacity(10),
            plugin_running_result: PluginRunningResult::Nothing,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.error_codes.push(err_code);
    }

    /// Push a new upstream_info and set it as current
    pub fn push_upstream_info(&mut self, upstream_info: UpstreamInfo) {
        self.upstream_info.push(upstream_info);
        let upstream_id = self.upstream_info.len() - 1;
        self.current_upstream_id = Some(upstream_id);
    }

    /// Get mutable reference to the current upstream_info
    pub fn get_current_upstream_info_mut(&mut self) -> Option<&mut UpstreamInfo> {
        let id = self.current_upstream_id?;
        self.upstream_info.get_mut(id)
    }

    /// Get reference to the current upstream_info
    pub fn get_current_upstream_info(&self) -> Option<&UpstreamInfo> {
        let id = self.current_upstream_id?;
        self.upstream_info.get(id)
    }
}
