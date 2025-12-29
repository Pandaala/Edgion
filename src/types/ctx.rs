use std::sync::Arc;
use std::time::Instant;
use std::fmt;
use serde::Serialize;
use crate::types::{EdgionStatus, HTTPBackendRef, GRPCBackendRef, HTTPRouteMatch};
use crate::types::filters::{PluginRunningResult};
use crate::types::resources::http_route_preparse::ParsedLBPolicy;
use crate::core::plugins::PluginLogs;
use crate::core::routes::HttpRouteRuleUnit;
use crate::core::routes::grpc_routes::GrpcRouteRuleUnit;
use pingora_core::protocols::l4::socket::SocketAddr;

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

/// Client certificate information extracted from TLS connection
#[derive(Debug, Clone, Serialize)]
pub struct ClientCertInfo {
    /// Certificate subject DN (Distinguished Name)
    pub subject: String,
    /// Subject Alternative Names (SANs) from certificate
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sans: Vec<String>,
    /// Common Name (CN) extracted from subject
    pub cn: Option<String>,
    /// Certificate fingerprint (SHA256 hex)
    pub fingerprint: String,
}

/// Request information extracted from the incoming request
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestInfo {
    /// TCP client IP address (direct connection, immutable)
    #[serde(rename = "client-addr")]
    pub client_addr: String,
    /// TCP client port (direct connection, 0 if unknown)
    #[serde(rename = "client-port")]
    pub client_port: u16,
    /// Real client address (extracted from headers if behind trusted proxy)
    #[serde(rename = "remote-addr")]
    pub remote_addr: String,
    /// Trace ID from x-trace-id header
    #[serde(rename = "x-trace-id")]
    pub x_trace_id: Option<String>,
    /// Hostname from the Host header
    #[serde(rename = "host")]
    pub hostname: String,
    /// Request path from URI
    pub path: String,
    /// Response status code (e.g., 200, 400, 404, 500)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Original X-Forwarded-For header value (before appending client IP)
    #[serde(rename = "x-forwarded-for", skip_serializing_if = "Option::is_none")]
    pub x_forwarded_for: Option<String>,
    /// SNI (Server Name Indication) from TLS handshake
    /// Only present for HTTPS connections; HTTP connections will have None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// Auto-discovered protocol (e.g., "grpc", "grpc-web", "websocket")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discover_protocol: Option<String>,
    /// gRPC service (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_service: Option<String>,
    /// gRPC method (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_method: Option<String>,
    /// Client certificate information (for mTLS connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert_info: Option<ClientCertInfo>,
}

/// Upstream connection information for a single connection attempt
#[derive(Debug, Clone, Serialize)]
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
    /// Connect time in milliseconds (from start_time to connection established)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ct: Option<u64>,
    /// Header time in milliseconds (from start_time to receiving response header)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ht: Option<u64>,
    /// Body time in milliseconds (from start_time to receiving first body chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bt: Option<u64>,
    /// Elapsed time in milliseconds (total time for this upstream attempt)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub et: Option<u64>,
    /// Start time - when this upstream attempt started (for calculating ct, ht, and bt)
    #[serde(skip)]
    pub start_time: Instant,
    /// Error messages collected during upstream attempts
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub err: Vec<String>,
    /// Backend socket address for connection counting (used by LeastConnection LB)
    #[serde(skip)]
    pub backend_addr: Option<SocketAddr>,
    /// LB policy used for this upstream selection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lb_policy: Option<ParsedLBPolicy>,
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

    /// Request information (hostname, path, x-trace-id)
    pub request_info: RequestInfo,

    /// Error codes collected during request processing
    pub edgion_status: Vec<EdgionStatus>,

    /// Matched HTTP route unit containing full route information
    pub route_unit: Option<Arc<HttpRouteRuleUnit>>,

    /// Selected HTTP backend from load balancing
    pub selected_backend: Option<HTTPBackendRef>,

    /// Matched gRPC route unit (for gRPC routes)
    pub grpc_route_unit: Option<Arc<GrpcRouteRuleUnit>>,

    /// Selected gRPC backend (for gRPC routes)
    pub selected_grpc_backend: Option<GRPCBackendRef>,

    /// Whether this request is handled by GRPCRoute (not just gRPC protocol)
    /// Used to determine backend peer selection and plugin execution
    pub is_grpc_route: bool,

    /// Backend context containing service info and upstream attempts
    pub backend_context: Option<BackendContext>,

    /// Plugin execution logs (grouped by stage)
    pub plugin_logs: Vec<PluginLogs>,

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
            request_info: RequestInfo::default(),
            edgion_status: Vec::with_capacity(5),
            route_unit: None,
            selected_backend: None,
            grpc_route_unit: None,
            selected_grpc_backend: None,
            is_grpc_route: false,
            backend_context: None,
            plugin_logs: Vec::with_capacity(3),
            plugin_running_result: PluginRunningResult::Nothing,
            try_cnt: 0,
            upstream_start_time: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.edgion_status.push(err_code);
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
                ht: None,
                bt: None,
                et: None,
                start_time: std::time::Instant::now(),
                err: Vec::new(),
                backend_addr: None,
                lb_policy: None,
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
