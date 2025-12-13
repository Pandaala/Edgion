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

/// Upstream connection information for a single connection attempt
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpstreamInfo {
    /// Upstream IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Upstream port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// HTTP status code for this upstream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Connect time - when connection to upstream was established
    #[serde(skip)]
    pub ct: Option<Instant>,
}

/// Backend context containing service info and upstream attempt history
#[derive(Debug, Clone, Serialize)]
pub struct BackendContext {
    /// Backend service name
    pub name: String,
    /// Backend service namespace
    pub namespace: String,
    /// List of upstream connection attempts (for retry tracking and access log)
    pub upstreams: Vec<UpstreamInfo>,
    /// Current upstream index (for status updates)
    #[serde(skip)]
    pub current_upstream_id: Option<usize>,
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

    /// Backend context containing service info and upstream attempts
    pub backend_context: Option<BackendContext>,

    /// Plugin execution logs
    pub plugin_logs: Vec<PluginLog>,

    /// Plugin running result
    pub plugin_running_result: PluginRunningResult,
    
    /// Number of connection attempts to backends
    pub try_cnt: u32,
    
    /// Time when first upstream connection was initiated
    /// Set only once on first connection attempt
    pub upstream_start_time: Option<Instant>,
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
            backend_context: None,
            plugin_logs: Vec::with_capacity(10),
            plugin_running_result: PluginRunningResult::Nothing,
            try_cnt: 0,
            upstream_start_time: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.error_codes.push(err_code);
    }

    /// Initialize backend context with name and namespace (call once)
    pub fn init_backend_context(&mut self, name: String, namespace: String) {
        self.backend_context = Some(BackendContext {
            name,
            namespace,
            upstreams: Vec::new(),
            current_upstream_id: None,
        });
    }

    /// Push a new upstream connection attempt with address info
    pub fn push_upstream(&mut self, ip: Option<String>, port: Option<u16>) {
        if let Some(ref mut bc) = self.backend_context {
            bc.upstreams.push(UpstreamInfo {
                ip,
                port,
                status: None,
                ct: None,
            });
            bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        }
    }

    /// Get mutable reference to current upstream
    pub fn get_current_upstream_mut(&mut self) -> Option<&mut UpstreamInfo> {
        self.backend_context.as_mut()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get_mut(id)))
    }

    /// Get reference to current upstream
    pub fn get_current_upstream(&self) -> Option<&UpstreamInfo> {
        self.backend_context.as_ref()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get(id)))
    }
}
