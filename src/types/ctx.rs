use crate::core::gateway::plugins::{EdgionPluginsLog, StageLogs};
use crate::core::gateway::routes::grpc::GrpcRouteRuleUnit;
use crate::core::gateway::routes::HttpRouteRuleUnit;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::http_route_preparse::ParsedLBPolicy;
use crate::types::{EdgionStatus, GRPCBackendRef, HTTPBackendRef, HTTPRouteMatch};
use bytes::Bytes;
use pingora_core::protocols::l4::socket::SocketAddr;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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
    pub fn new(rns: String, rn: String, rule_id: usize, match_id: usize, m: HTTPRouteMatch) -> Self {
        Self {
            rns,
            rn,
            m,
            rule_id,
            match_id,
        }
    }
}

impl fmt::Display for MatchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} (rule:{}, match:{})",
            self.rns, self.rn, self.rule_id, self.match_id
        )
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

/// TLS connection identifier stored in SslDigestExtension
#[derive(Debug, Clone)]
pub struct TlsConnId(pub u64);

/// TLS handshake metadata stored in SslDigestExtension.
///
/// The value is created once per TLS connection and shared by requests on that
/// connection through Pingora's digest extension.
#[derive(Debug, Clone)]
pub struct TlsConnMeta {
    /// Connection id for tls.log <-> access.log correlation.
    pub tls_id: u64,
    /// SNI from handshake (if present).
    pub sni: Option<String>,
    /// mTLS client cert info (only when annotation gate is enabled).
    pub client_cert_info: Option<ClientCertInfo>,
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
    /// Whether the request is a gRPC or gRPC-Web request
    #[serde(skip)]
    pub is_grpc_request: bool,
    /// gRPC service (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_service: Option<String>,
    /// gRPC method (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_method: Option<String>,
    /// Client certificate information (for mTLS connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert_info: Option<ClientCertInfo>,
    /// TLS connection id for correlating tls.log and access.log
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<u64>,
    /// Listener port that received this request (set at context creation, always available)
    #[serde(rename = "listener-port")]
    pub listener_port: u16,
}

/// Backend TLS connection information
#[derive(Debug, Clone, Serialize)]
pub struct BackendTlsInfo {
    /// SNI (Server Name Indication) sent to backend
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// TLS handshake success
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_ok: Option<bool>,
    /// TLS protocol version (e.g., "TLSv1.3")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Cipher suite used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
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
    /// Upstream response body size in bytes (payload only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body_size: Option<usize>,
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
    /// Backend TLS connection information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<BackendTlsInfo>,
}

impl UpstreamInfo {
    #[inline]
    pub fn set_response_body_size(&mut self, size: usize) {
        if self.response_body_size.is_some() {
            self.err.push("response_body_size already set".to_string());
        }
        self.response_body_size = Some(size);
    }
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

/// Stored internal jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset which specifies an exact endpoint address,
/// InternalJumpPreset specifies a BackendRef by name. The actual endpoint
/// selection happens via normal LB in get_peer().
///
/// This preset is consumed by select_http_backend() to find the matching
/// BackendRef instead of using weighted round-robin selection.
#[derive(Debug, Clone)]
pub struct InternalJumpPreset {
    /// Name of the target backend_ref (must match a backend_ref in the route)
    pub backend_ref_name: String,
    /// Optional namespace of the target backend_ref
    /// If None, matches by name only
    pub backend_ref_namespace: Option<String>,
}

/// Stored external jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset (exact IP) or InternalJumpPreset (BackendRef name),
/// ExternalJumpPreset stores a domain name that requires DNS resolution
/// before creating the HttpPeer.
///
/// DNS resolution happens in upstream_peer_http() phase (async context).
/// On retry, DNS resolution is redone — the domain may resolve to a different
/// IP, providing natural DNS-level failover.
#[derive(Debug, Clone)]
pub struct ExternalJumpPreset {
    /// Target domain name (e.g., "api-us.example.com")
    pub domain: String,
    /// Target port
    pub port: u16,
    /// Whether to use TLS
    pub use_tls: bool,
    /// TLS SNI (Server Name Indication)
    pub sni: String,
}

/// Stored direct endpoint info (lightweight, survives retries via Clone)
///
/// Unlike Box<HttpPeer> which would be consumed on first use,
/// DirectEndpointPreset persists across retries so upstream_peer can
/// rebuild the same HttpPeer on each attempt.
#[derive(Debug, Clone)]
pub struct DirectEndpointPreset {
    /// Target socket address (ip:port)
    pub addr: std::net::SocketAddr,
    /// Whether to use TLS
    pub use_tls: bool,
    /// SNI for TLS connections
    pub sni: String,
    /// Index of the matched backend_ref in route_unit.rule.backend_refs
    /// Used to pre-select the correct backend_ref for logging and metrics
    pub backend_ref_idx: usize,
}

/// Snapshot of mirror config for request runtime.
/// Note: max_concurrent is managed by the Semaphore in RequestMirrorPlugin and
/// is NOT included here — the spawned task does not need it.
#[derive(Debug, Clone)]
pub struct MirrorConfig {
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub max_buffered_chunks: usize,
    pub mirror_log: bool,
}

/// Mirror stream state kept in request context.
pub enum MirrorState {
    /// Mirror request task is active and waiting for body chunks from request_body_filter.
    Streaming {
        body_tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
        writer_handle: JoinHandle<()>,
        /// Shared flag between request_body_filter and the mirror task.
        /// Set to true by request_body_filter when the mirror is abandoned due to
        /// channel full. The mirror task reads this flag to distinguish
        ///  "channel_full" from a genuine "write_err".
        channel_full_flag: Arc<AtomicBool>,
        /// Maximum milliseconds to wait for channel space before abandoning the mirror.
        /// 0 = immediate abandon (zero impact on main request latency, default).
        /// > 0 = brief back-pressure window (adds at most this many ms to body processing).
        channel_full_timeout_ms: u64,
    },
    /// Mirror has been disabled for this request (timeout/error/full buffer/closed channel).
    Abandoned,
}

pub struct EdgionHttpContext {
    /// Request start time for latency calculation
    pub start_time: Instant,

    /// Gateway information (copied from EdgionHttp for easy access)
    pub gateway_info: GatewayInfo,

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
    pub is_grpc_route_matched: bool,

    /// Backend context containing service info and upstream attempts
    pub backend_context: Option<BackendContext>,

    /// Stage execution logs (grouped by stage)
    pub stage_logs: Vec<StageLogs>,

    /// Pending EdgionPlugins logs (collected during current stage, merged at stage end)
    pub pending_edgion_plugins_logs: Vec<EdgionPluginsLog>,

    /// Tracking stack for nested plugin references to prevent cycles
    pub plugin_ref_stack: Vec<String>,

    /// Plugin running result
    pub plugin_running_result: PluginRunningResult,

    /// Number of connection attempts to backends
    pub try_cnt: u32,

    /// Time when first upstream connection was initiated
    /// Set only once on first connection attempt
    pub upstream_start_time: Option<Instant>,

    /// Hash key used for consistent hashing (for test metrics)
    pub hash_key: Option<String>,

    /// Context variables map for plugin communication
    /// Plugins can set values (e.g., KeySet plugin) and conditions can read them
    pub ctx_map: HashMap<String, String>,

    /// Lazily extracted path parameters from route pattern (e.g., "/api/:uid/profile")
    /// - None: not yet extracted
    /// - Some(HashMap): already extracted (may be empty if no params or no match)
    pub path_params: Option<HashMap<String, String>>,

    /// Direct endpoint set by DirectEndpoint plugin in request_filter stage.
    pub direct_endpoint: Option<DirectEndpointPreset>,

    /// Internal jump target set by DynamicInternalUpstream plugin in request_filter stage.
    /// When present, select_http_backend() finds the matching BackendRef by name
    /// instead of using weighted round-robin selection.
    /// Normal LB within the selected service's endpoints still applies.
    pub internal_jump: Option<InternalJumpPreset>,

    /// External jump target set by DynamicExternalUpstream plugin in request_filter stage.
    /// When present, upstream_peer_http() resolves the domain and builds
    /// HttpPeer from this info, bypassing normal select_http_backend().
    /// Host header override is applied separately via set_upstream_host()
    /// during request_filter.
    pub external_jump: Option<ExternalJumpPreset>,

    /// Response headers to add (queued from request stage)
    pub response_headers_to_add: Vec<(String, String)>,

    /// Per-request mirror state for RequestMirror plugin.
    pub mirror_state: Option<MirrorState>,
}

impl Default for EdgionHttpContext {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionHttpContext {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            gateway_info: GatewayInfo::default(),
            request_info: RequestInfo::default(),
            edgion_status: Vec::with_capacity(5),
            route_unit: None,
            selected_backend: None,
            grpc_route_unit: None,
            selected_grpc_backend: None,
            is_grpc_route_matched: false,
            backend_context: None,
            stage_logs: Vec::with_capacity(3),
            pending_edgion_plugins_logs: Vec::new(),
            plugin_ref_stack: Vec::new(),
            plugin_running_result: PluginRunningResult::Nothing,
            try_cnt: 0,
            upstream_start_time: None,
            hash_key: None,
            ctx_map: HashMap::new(),
            path_params: None,
            direct_endpoint: None,
            internal_jump: None,
            external_jump: None,
            response_headers_to_add: Vec::new(),
            mirror_state: None,
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
                response_body_size: None,
                start_time: std::time::Instant::now(),
                err: Vec::new(),
                backend_addr: None,
                lb_policy: None,
                tls: None,
            });
            bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        }
    }

    /// Get mutable reference to current upstream
    pub fn get_current_upstream_mut(&mut self) -> Option<&mut UpstreamInfo> {
        self.backend_context
            .as_mut()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get_mut(id)))
    }

    /// Get reference to current upstream
    pub fn get_current_upstream(&self) -> Option<&UpstreamInfo> {
        self.backend_context
            .as_ref()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get(id)))
    }

    /// Push a plugin reference path onto the stack
    pub fn push_plugin_ref(&mut self, key: String) {
        self.plugin_ref_stack.push(key);
    }

    /// Pop a plugin reference path from the stack
    pub fn pop_plugin_ref(&mut self) {
        self.plugin_ref_stack.pop();
    }

    /// Current depth of nested plugin references
    pub fn plugin_ref_depth(&self) -> usize {
        self.plugin_ref_stack.len()
    }

    /// Whether the stack already contains the given reference key (for cycle detection)
    pub fn has_plugin_ref(&self, key: &str) -> bool {
        self.plugin_ref_stack.iter().any(|k| k == key)
    }

    // ========== Context variable methods ==========

    /// Get a context variable by key
    pub fn get_ctx_var(&self, key: &str) -> Option<&str> {
        self.ctx_map.get(key).map(|s| s.as_str())
    }

    /// Set a context variable (for plugins like KeySet)
    pub fn set_ctx_var(&mut self, key: String, value: String) {
        self.ctx_map.insert(key, value);
    }

    /// Remove a context variable
    pub fn remove_ctx_var(&mut self, key: &str) -> Option<String> {
        self.ctx_map.remove(key)
    }

    /// Check if a context variable exists
    pub fn has_ctx_var(&self, key: &str) -> bool {
        self.ctx_map.contains_key(key)
    }
}
