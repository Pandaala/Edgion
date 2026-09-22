//! EdgionGatewayConfig CRD definition
//!
//! This model defines the EdgionGatewayConfig custom resource, which is used
//! as parametersRef in GatewayClass to provide gateway-wide configuration.

use super::common::Condition;
use super::edgion_plugins::plugin_configs::LinkSysRef;
use super::edgion_plugins::{LogExtSource, RealIpConfig};
use super::plugin_policy::PluginPolicyConfig;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest retry budget shared by Gateway API routes and the Gateway-wide
/// default. The public route fields are signed 32-bit integers, so this keeps
/// every configuration source in the same non-negative domain.
pub const MAX_RETRY_ATTEMPTS: u32 = i32::MAX as u32;

/// EdgionGatewayConfig is the configuration for a GatewayClass
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1alpha1",
    kind = "EdgionGatewayConfig",
    plural = "edgiongatewayclassconfigs",
    shortname = "edgwcfg",
    namespaced = false,
    status = "EdgionGatewayConfigStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionGatewayConfigSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<EdgionGatewayConfigStatus>,

    /// GatewayClass-scoped HTTP server behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerConfig>,

    /// HTTP timeout configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_timeout: Option<HttpTimeout>,

    /// GatewayClass-wide retry defaults used when a route omits its own policy.
    #[serde(default)]
    pub retry: RetryConfig,

    /// Real IP extraction configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_ip: Option<RealIpConfig>,

    /// Gateway-owned headers sent to upstream requests.
    #[serde(default)]
    pub forwarded_headers: ForwardedHeadersConfig,

    /// Security protection configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_protect: Option<SecurityProtectConfig>,

    /// GatewayClass-wide request-body buffering policy.
    #[serde(default)]
    pub request_body: RequestBodyConfig,

    /// GatewayClass-scoped plugin execution policy. Absence permits all plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_policy: Option<PluginPolicyConfig>,

    /// Global plugins references that apply to all routes using this GatewayClass
    /// These plugins are executed before route-level plugins
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_plugins_ref: Option<Vec<PluginReference>>,

    /// Access log extension policy (the masking whitelist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_log_extern: Option<AccessLogExternPolicy>,

    /// Preflight request handling policy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_policy: Option<PreflightPolicy>,

    /// Global LinkSys provider configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_sys: Option<LinkSysGlobalConfig>,

    /// Timeout configuration for L4 TCP-transport relays
    /// (plain TCP, TLS-terminated, TLS-passthrough). UDP has its own
    /// session model and is configured separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_timeout: Option<TcpTimeout>,

    /// Load-balancing-wide settings (degrade threshold, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancing: Option<LoadBalancingConfig>,

    /// Cluster-wide default for plugin / LinkSys outbound TLS verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_tls: Option<crate::resources::common::OutboundTlsConfig>,

    /// Data-plane default DNS resolver for `ServiceExternalName` backends.
    /// Overridden per-Gateway (annotation) and per-Service (annotation); this is
    /// the gateway-wide fallback below those. Control-plane resolution does NOT
    /// use this (it uses Service annotation → Controller YAML default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_resolver: Option<DnsResolverConfig>,
}

/// Data-plane default DNS resolver configuration (EdgionGatewayConfig).
///
/// `linkSysRef` (an httpdns LinkSys) and `servers` (inline standard DNS) are
/// mutually exclusive; the structural CRD cannot express `oneOf`, so both stay
/// optional and the exclusion is enforced at ingest.
//
// NOTE: no `PartialEq`/`Eq` — kept simple and consistent with the other EGC
// sub-structs; not required by any consumer.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DnsResolverConfig {
    /// httpdns LinkSys reference (`namespace` + `name`). Namespace is required
    /// because EdgionGatewayConfig is cluster-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_sys_ref: Option<LinkSysRef>,

    /// Inline standard-DNS server list (`ip[:port]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<String>>,

    /// Data-plane resolution cache TTL (GEP-2257 duration, default "5s").
    #[serde(default = "default_dns_cache_ttl")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub cache_ttl: String,
}

/// Gateway-owned headers sent to upstream requests.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForwardedHeadersConfig {
    /// Header receiving the final resolved RemoteIp value on upstream requests.
    #[serde(default = "default_remote_ip_header")]
    #[schemars(
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN),
        length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN)
    )]
    pub remote_ip_header: String,
}

/// GatewayClass-wide retry policy.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    /// Maximum retries after the initial upstream attempt.
    #[serde(default = "default_retry_attempts")]
    #[schemars(range(max = 2147483647))]
    pub attempts: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: default_retry_attempts(),
        }
    }
}

fn default_remote_ip_header() -> String {
    crate::gwapi_types::DEFAULT_REMOTE_IP_HEADER.to_string()
}

impl Default for ForwardedHeadersConfig {
    fn default() -> Self {
        Self {
            remote_ip_header: default_remote_ip_header(),
        }
    }
}

impl ForwardedHeadersConfig {
    /// Validate the operator-configured upstream RemoteIp header name.
    pub fn validate(&self) -> Result<(), String> {
        crate::gwapi_types::validate_remote_ip_header_name(&self.remote_ip_header)
    }
}

fn default_dns_cache_ttl() -> String {
    "5s".to_string()
}

// ============================================
// Body Size Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestBodyConfig {
    /// Runtime switch. A body-capable binary still refuses capture when false.
    #[serde(default = "default_request_body_enabled")]
    pub enabled: bool,

    /// Default in-memory bytes retained before spilling to disk.
    #[serde(default = "default_memory_buffer_size")]
    pub default_memory_buffer_size: String,

    /// Optional business override for the in-memory spill threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_buffer_size: Option<String>,

    /// Default complete request-body cap.
    #[serde(default = "default_request_body_max_size")]
    pub default_max_body_size: String,

    /// Optional business override for the complete request-body cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_size: Option<String>,

    /// Wall-clock deadline shared by admission and execution of one
    /// request-body storage file operation.
    #[serde(default = "default_request_body_storage_operation_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub storage_operation_timeout: String,
}

impl RequestBodyConfig {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let default_memory = validate_positive_byte_size(
            Some(self.default_memory_buffer_size.as_str()),
            "requestBody.defaultMemoryBufferSize",
            &mut errors,
        );
        let memory = match self.max_memory_buffer_size.as_deref() {
            Some(value) => validate_positive_byte_size(Some(value), "requestBody.maxMemoryBufferSize", &mut errors),
            None => default_memory,
        };
        let default_body = validate_positive_byte_size(
            Some(self.default_max_body_size.as_str()),
            "requestBody.defaultMaxBodySize",
            &mut errors,
        );
        let body = match self.max_body_size.as_deref() {
            Some(value) => validate_positive_byte_size(Some(value), "requestBody.maxBodySize", &mut errors),
            None => default_body,
        };
        if let (Some(memory), Some(body)) = (memory, body) {
            if memory > body {
                errors.push(
                    "effective requestBody memory buffer size must not exceed the effective requestBody max body size"
                        .to_string(),
                );
            }
        }
        match crate::duration::parse_duration(&self.storage_operation_timeout) {
            Ok(timeout) if timeout.is_zero() => {
                errors.push("requestBody.storageOperationTimeout must be greater than zero".to_string());
            }
            Ok(_) => {}
            Err(error) => errors.push(format!(
                "requestBody.storageOperationTimeout must be a valid duration: {error}"
            )),
        }
        errors
    }
}

impl Default for RequestBodyConfig {
    fn default() -> Self {
        Self {
            enabled: default_request_body_enabled(),
            default_memory_buffer_size: default_memory_buffer_size(),
            max_memory_buffer_size: None,
            default_max_body_size: default_request_body_max_size(),
            max_body_size: None,
            storage_operation_timeout: default_request_body_storage_operation_timeout(),
        }
    }
}

fn default_request_body_enabled() -> bool {
    true
}

fn default_memory_buffer_size() -> String {
    "128KiB".to_string()
}

fn default_request_body_max_size() -> String {
    "32MiB".to_string()
}

fn default_request_body_storage_operation_timeout() -> String {
    "5s".to_string()
}

fn validate_positive_byte_size(value: Option<&str>, field: &str, errors: &mut Vec<String>) -> Option<u64> {
    let value = value?;
    match crate::resources::common::parse_byte_size(value) {
        Some(bytes) if bytes > 0 => Some(bytes),
        _ => {
            errors.push(format!("{field} '{value}' is invalid (expected e.g. '128KiB')"));
            None
        }
    }
}

// ============================================
// Server Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Enable downstream response compression (default: false)
    #[serde(default)]
    pub enable_compression: bool,

    /// Maximum number of requests a downstream HTTP/1 connection can serve before being closed.
    /// Per-connection limit, similar to Nginx's `keepalive_requests`.
    /// Helps with memory management and load balancing distribution.
    /// Default: 10000. Set to 0 to disable the limit.
    #[serde(default = "default_downstream_keepalive_request_limit")]
    pub downstream_keepalive_request_limit: u32,
}

fn default_downstream_keepalive_request_limit() -> u32 {
    10_000
}

// ============================================
// HTTP Timeout Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpTimeout {
    /// Client-side timeout settings
    #[serde(default)]
    pub client: ClientTimeout,

    /// Backend-side timeout settings
    #[serde(default)]
    pub backend: BackendTimeout,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientTimeout {
    /// Timeout for reading request from client
    /// Format: Duration string (e.g., "60s", "1m", "500ms")
    #[serde(default = "default_client_read_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub read_timeout: String,

    /// Timeout for writing response to client
    /// Format: Duration string (e.g., "60s", "1m", "500ms")
    #[serde(default = "default_client_write_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub write_timeout: String,

    /// HTTP keepalive timeout
    /// Format: Duration string (e.g., "75s", "1m", "500ms")
    #[serde(default = "default_client_keepalive_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub keepalive_timeout: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendTimeout {
    /// Timeout for connecting to upstream backend
    /// Format: Duration string (e.g., "5s", "1m", "500ms")
    #[serde(default = "default_backend_connect_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub default_connect_timeout: String,

    /// Default per-attempt backend request timeout — the fallback for the
    /// Gateway API `timeouts.backendRequest` field when a route does not set
    /// it. Applied per upstream attempt (NOT including retries); with a retry
    /// policy present it also acts as the per-try timeout (GEP-1731): an
    /// attempt exceeding it is canceled and retried. `"0s"` disables the bound.
    /// Format: Duration string (e.g., "60s", "1m", "500ms")
    #[serde(default = "default_backend_request_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub default_request_timeout: String,

    /// Idle timeout for backend connection pool
    /// Format: Duration string (e.g., "300s", "5m", "500ms")
    #[serde(default = "default_backend_idle_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub default_idle_timeout: String,
}

// ============================================
// TCP (L4) Timeout Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TcpTimeout {
    /// Idle timeout for the established L4 relay. The activity clock resets on
    /// any byte progress in either direction (a completed read OR an advancing
    /// partial write). "0s" (any zero-valued duration) disables it (unbounded —
    /// risky; a bare "0" is not valid GEP-2257 and is rejected at admission). Applies to
    /// plain TCP, TLS-terminated, and TLS-passthrough relays. UDP is not affected.
    /// Format: GEP-2257 duration string (e.g. "1h", "300s", "5m"). Default "1h".
    #[serde(default = "default_tcp_idle_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub idle_timeout: String,

    /// Upper bound for establishing the upstream TCP connection (the TCP
    /// handshake) on an L4 route. Mirrors the *mechanism* of the HTTP path's
    /// `backend.defaultConnectTimeout` (a per-peer connect bound); the default
    /// value differs — 10s here vs the HTTP path's 5s, matching the Istio L4
    /// `connectTimeout` baseline. A backend that silently drops SYNs (firewall,
    /// not-yet-ready pod) fails fast instead of stranding the downstream
    /// connection until the OS TCP retransmit timeout (~2m) fires.
    /// "0s" (any zero-valued duration) disables it (unbounded — relies on the OS
    /// bound; a bare "0" is not valid GEP-2257 and is rejected at admission). Applies to
    /// plain TCP, TLS-terminated, and TLS-passthrough relays — in all three the
    /// upstream connect is plain TCP (TLS-terminated terminates downstream TLS
    /// separately; TLS-passthrough carries the end-to-end TLS handshake over the
    /// established relay), so this bounds the connect itself, not a TLS
    /// handshake. UDP is not affected.
    /// Format: GEP-2257 duration string (e.g. "10s", "1m", "500ms"). Default "10s".
    #[serde(default = "default_tcp_connect_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub connect_timeout: String,
}

fn default_tcp_idle_timeout() -> String {
    "1h".to_string()
}

fn default_tcp_connect_timeout() -> String {
    "10s".to_string()
}

// ============================================
// Load-Balancing Configuration
// ============================================

/// Gateway-wide load-balancing configuration.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancingConfig {
    /// Degrade threshold percent (0-100). When the fraction of healthy backends
    /// for a service drops below this percent, the gateway ignores health
    /// filtering and load-balances across all backends. 0 (default) disables
    /// degrade mode. An EdgionBackendTrafficPolicy's `loadBalancer.degradeThreshold`
    /// overrides this for its target Services.
    #[serde(default)]
    pub degrade_threshold: u8,
}

/// Secure default idle timeout applied to every L4 relay when the operator
/// configures no `tcpTimeout` section. Bounded-by-default: protection does not
/// require config. Must stay in sync with `default_tcp_idle_timeout()` (a unit
/// test asserts it).
pub const DEFAULT_TCP_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Resolve the effective L4 relay idle timeout from the optional `tcpTimeout`
/// section.
///
/// - section absent (`None`)                      -> `Some(DEFAULT_TCP_IDLE)` (secure default)
/// - explicit `"0s"` / any zero-valued duration   -> `None` (operator opt-out, unbounded)
/// - any other valid duration                     -> `Some(parsed)`
/// - parse error (schema-validated upstream)       -> `Some(DEFAULT_TCP_IDLE)` (fail-safe, silent)
///
/// Used while publishing the gateway's resolved runtime policy; intentionally
/// emits no `tracing`.
pub fn resolve_tcp_idle_timeout(cfg: Option<&TcpTimeout>) -> Option<std::time::Duration> {
    match cfg {
        None => Some(DEFAULT_TCP_IDLE),
        Some(t) => match crate::duration::parse_duration(&t.idle_timeout) {
            Ok(d) if d.is_zero() => None,
            Ok(d) => Some(d),
            Err(_) => Some(DEFAULT_TCP_IDLE),
        },
    }
}

/// Secure default upstream connect timeout applied to every L4 relay when the
/// operator configures no `tcpTimeout` section. Bounded-by-default: protection
/// does not require config. Must stay in sync with `default_tcp_connect_timeout()`
/// (a unit test asserts it).
pub const DEFAULT_TCP_CONNECT: std::time::Duration = std::time::Duration::from_secs(10);

/// Resolve the effective L4 upstream connect timeout from the optional
/// `tcpTimeout` section. Result maps directly onto Pingora's
/// `BasicPeer.options.connection_timeout` (`None` = no application-level bound).
///
/// - section absent (`None`)                      -> `Some(DEFAULT_TCP_CONNECT)` (secure default)
/// - explicit `"0s"` / any zero-valued duration   -> `None` (operator opt-out, unbounded)
/// - any other valid duration                     -> `Some(parsed)`
/// - parse error (schema-validated upstream)       -> `Some(DEFAULT_TCP_CONNECT)` (fail-safe, silent)
///
/// Used while publishing the gateway's resolved runtime policy; intentionally
/// emits no `tracing`.
pub fn resolve_tcp_connect_timeout(cfg: Option<&TcpTimeout>) -> Option<std::time::Duration> {
    match cfg {
        None => Some(DEFAULT_TCP_CONNECT),
        Some(t) => match crate::duration::parse_duration(&t.connect_timeout) {
            Ok(d) if d.is_zero() => None,
            Ok(d) => Some(d),
            Err(_) => Some(DEFAULT_TCP_CONNECT),
        },
    }
}

// ClientTimeout defaults
fn default_client_read_timeout() -> String {
    "60s".to_string()
}

fn default_client_write_timeout() -> String {
    "60s".to_string()
}

fn default_client_keepalive_timeout() -> String {
    "75s".to_string()
}

// BackendTimeout defaults
fn default_backend_connect_timeout() -> String {
    "5s".to_string()
}

fn default_backend_request_timeout() -> String {
    "60s".to_string()
}

fn default_backend_idle_timeout() -> String {
    "300s".to_string()
}

// Global configuration defaults
fn default_retry_attempts() -> u32 {
    2
}

impl Default for ClientTimeout {
    fn default() -> Self {
        Self {
            read_timeout: default_client_read_timeout(),
            write_timeout: default_client_write_timeout(),
            keepalive_timeout: default_client_keepalive_timeout(),
        }
    }
}

impl Default for BackendTimeout {
    fn default() -> Self {
        Self {
            default_connect_timeout: default_backend_connect_timeout(),
            default_request_timeout: default_backend_request_timeout(),
            default_idle_timeout: default_backend_idle_timeout(),
        }
    }
}

// ============================================
// Status
// ============================================

/// EdgionGatewayConfigStatus describes the status of the EdgionGatewayConfig resource
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct EdgionGatewayConfigStatus {
    /// Conditions describe the current conditions of the EdgionGatewayConfig.
    /// Standard conditions: Accepted, Ready
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

// ============================================
// Security Protection Configuration
// ============================================

/// Security protection configuration for the gateway
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityProtectConfig {
    /// Maximum length of X-Forwarded-For header in bytes (default: 200)
    /// Requests with X-Forwarded-For headers exceeding this limit will be rejected with 400 Bad Request
    #[serde(default = "default_xff_limit")]
    pub x_forwarded_for_limit: usize,

    /// Require SNI and Host header to match for HTTPS requests (default: true)
    /// When enabled, HTTPS requests with mismatched SNI and Host will be rejected with 421 Misdirected Request
    /// HTTP requests (no SNI) are not affected by this validation
    #[serde(default = "default_require_sni_host_match")]
    pub require_sni_host_match: bool,

    /// Fallback SNI hostname to use when client doesn't provide SNI in TLS handshake
    /// If not set, requests without SNI will fail with certificate error
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_sni: Option<String>,

    /// Enable TLS proxy connection logging (connect/disconnect events in tls.log).
    /// When false, only ssl.log (handshake) is written; tls.log and per-listener
    /// access log entries for TLS proxy connections are suppressed. (default: true)
    #[serde(default = "default_tls_proxy_log_record")]
    pub tls_proxy_log_record: bool,

    /// Allow loopback upstream addresses for this Gateway.
    ///
    /// Default false. This is an explicit operator override for local sidecar or
    /// node-local deployments. Link-local / cloud metadata ranges remain blocked.
    #[serde(default)]
    pub allow_loopback_upstream: bool,

    /// Reject HTTP/1.1 requests that contain more than one Host header.
    ///
    /// Per RFC 9110 §7.2 a server MUST respond with 400 to a request that
    /// contains more than one Host header field line; allowing such requests
    /// to pass enables Host smuggling across the gateway. Disable only as a
    /// transitional escape hatch for legacy middleware that emits duplicate
    /// Host lines. Default: true.
    #[serde(default = "default_reject_duplicate_host")]
    pub reject_duplicate_host: bool,
}

fn default_xff_limit() -> usize {
    200
}

fn default_require_sni_host_match() -> bool {
    true
}

fn default_tls_proxy_log_record() -> bool {
    true
}

fn default_reject_duplicate_host() -> bool {
    true
}

// ============================================
// Preflight Policy Configuration
// ============================================

/// Preflight request handling policy
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightPolicy {
    /// Preflight detection mode (default: cors-standard)
    #[serde(default = "default_preflight_mode")]
    pub mode: PreflightMode,

    /// Status code to return when no CORS plugin is configured (default: 204)
    #[serde(default = "default_preflight_status_code")]
    #[schemars(range(min = 200, max = 599))]
    pub status_code: u16,
}

/// Preflight detection mode
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PreflightMode {
    /// CORS standard: OPTIONS + Origin + Access-Control-Request-Method (recommended)
    CorsStandard,
    /// All OPTIONS requests are treated as preflight
    AllOptions,
}

fn default_preflight_mode() -> PreflightMode {
    PreflightMode::CorsStandard
}

fn default_preflight_status_code() -> u16 {
    204
}

// ============================================
// Plugin Reference
// ============================================

/// Plugin reference for referencing EdgionPlugins resources
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginReference {
    /// Name of the EdgionPlugins resource
    pub name: String,

    /// Namespace of the EdgionPlugins resource (defaults to "default")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

// ============================================
// LinkSys Global Configuration
// ============================================

/// Default `linkSys.maxInstancesPerKind` when the field is omitted.
pub const LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND: usize = 200;

/// Inclusive upper bound for `linkSys.maxInstancesPerKind`.
pub const LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT: usize = 10_000;

/// Global configuration for LinkSys providers
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysGlobalConfig {
    /// Global hard cap for webhook response body size (bytes).
    /// Per-resource `maxResponseBytes` is clamped to this value.
    /// Default: 16384 (16KB)
    #[serde(default = "default_webhook_max_response_bytes")]
    pub webhook_max_response_bytes: usize,

    /// Max current registered persistent LinkSys instances per provider kind.
    /// New keys wait for admission at the cap; removal or a cap increase retries
    /// them. Updates and deletes proceed without waiting for old cleanup. Default: 200.
    #[schemars(range(min = 1, max = 10_000))]
    #[serde(default = "default_max_instances_per_kind")]
    pub max_instances_per_kind: usize,
}

impl Default for LinkSysGlobalConfig {
    fn default() -> Self {
        Self {
            webhook_max_response_bytes: default_webhook_max_response_bytes(),
            max_instances_per_kind: default_max_instances_per_kind(),
        }
    }
}

fn default_webhook_max_response_bytes() -> usize {
    16384
}

fn default_max_instances_per_kind() -> usize {
    LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND
}

// ============================================
// Access Log Extension Policy (masking whitelist)
// ============================================

/// Access log extension policy. Platform-owned: it lives on the cluster-scoped
/// `EdgionGatewayConfig` (referenced via `GatewayClass.spec.parametersRef`) so a
/// tenant who owns a route and its `EdgionPlugins` cannot widen masking.
///
/// The request's captured plugin publication scope selects this policy through
/// its owning GatewayClass. EGC updates rebuild that scope; an in-flight request
/// retains its captured masking policy even when physical listeners are shared.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessLogExternPolicy {
    /// Keys logged in clear. Everything else is masked per
    /// `access_log_sensitive.mode`.
    pub unmasked_keys: UnmaskedKeys,
}

/// Per-source whitelists of keys that bypass masking.
///
/// Keyed by source rather than one flat list: `user-agent` as a header is
/// harmless, while a cookie of the same name is a different object entirely, and
/// a flat list could not tell them apart.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct UnmaskedKeys {
    /// Request header names. Matched case-insensitively.
    pub header: Vec<String>,
    /// Response header names. Matched case-insensitively.
    pub resp_header: Vec<String>,
    /// Query parameter names. Case-sensitive.
    pub query: Vec<String>,
    /// Cookie names. Case-sensitive.
    pub cookie: Vec<String>,
    /// Context variable names. Case-sensitive.
    pub ctx: Vec<String>,
}

impl Default for UnmaskedKeys {
    /// The defaults are the fields every access log already records that carry
    /// no identity: content negotiation plus `x-request-id` for correlation.
    ///
    /// `referer` and `location` are deliberately absent — see the design spec.
    /// `query` / `cookie` / `ctx` are empty because their key names are entirely
    /// business-defined; there is no universally safe name.
    fn default() -> Self {
        Self {
            header: [
                "user-agent",
                "origin",
                "content-type",
                "content-length",
                "accept",
                "x-request-id",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            resp_header: [
                "content-type",
                "content-length",
                "cache-control",
                "retry-after",
                "x-request-id",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            query: Vec::new(),
            cookie: Vec::new(),
            ctx: Vec::new(),
        }
    }
}

impl UnmaskedKeys {
    /// Whether `name` from `source` may be logged in clear.
    ///
    /// Category-1 sources (`routeLabel` / `routeAnnotation`) are never masked and
    /// bypass this check upstream; they answer `false` here so a caller that
    /// wrongly consults this cannot widen anything.
    pub fn allows(&self, source: LogExtSource, name: &str) -> bool {
        match source {
            LogExtSource::Header => self.header.iter().any(|k| k.eq_ignore_ascii_case(name)),
            LogExtSource::RespHeader => self.resp_header.iter().any(|k| k.eq_ignore_ascii_case(name)),
            LogExtSource::Query => self.query.iter().any(|k| k == name),
            LogExtSource::Cookie => self.cookie.iter().any(|k| k == name),
            LogExtSource::Ctx => self.ctx.iter().any(|k| k == name),
            LogExtSource::RouteLabel | LogExtSource::RouteAnnotation => false,
        }
    }
}

impl EdgionGatewayConfigSpec {
    /// Validate the Gateway-wide retry budget against the route field's
    /// representable domain.
    pub fn validate_retry(&self) -> Vec<String> {
        if self.retry.attempts > MAX_RETRY_ATTEMPTS {
            vec![format!(
                "retry.attempts must be between 0 and {MAX_RETRY_ATTEMPTS}, got {}",
                self.retry.attempts
            )]
        } else {
            Vec::new()
        }
    }

    /// Validate plugin policy with the same pure compiler used by the Gateway.
    pub fn validate_plugin_policy(&self) -> Vec<String> {
        self.plugin_policy
            .as_ref()
            .and_then(|policy| policy.compile().err())
            .into_iter()
            .collect()
    }

    /// Validate the status emitted by the built-in preflight response.
    pub fn validate_preflight_policy(&self) -> Vec<String> {
        self.preflight_policy
            .as_ref()
            .filter(|policy| !crate::http_status::is_valid_final_http_status(policy.status_code))
            .map(|policy| {
                vec![format!(
                    "preflightPolicy.statusCode must be between 200 and 599, got {}",
                    policy.status_code
                )]
            })
            .unwrap_or_default()
    }

    /// Validate the GatewayClass-wide request-body buffering policy.
    pub fn validate_request_body(&self) -> Vec<String> {
        self.request_body.validate()
    }

    /// Validate `spec.realIp` at ingest. Returns error strings suitable for an
    /// `Accepted=False/reason=Invalid` condition (empty = OK).
    ///
    /// Shares `RealIpConfig`'s validation body with the RealIp plugin admission
    /// path, so a header name accepted for one consumer is accepted for both.
    pub fn validate_real_ip(&self) -> Vec<String> {
        use crate::resources::edgion_plugins::PluginValidation;
        self.real_ip
            .as_ref()
            .and_then(|config| config.validation_error())
            .map(|error| vec![format!("realIp: {error}")])
            .unwrap_or_default()
    }

    /// Validate `spec.forwardedHeaders` at ingest. Returns error strings
    /// suitable for an `Accepted=False/reason=Invalid` condition.
    pub fn validate_forwarded_headers(&self) -> Vec<String> {
        self.forwarded_headers.validate().err().into_iter().collect()
    }

    /// Validate `spec.linkSys` at ingest. Returns error strings suitable for an
    /// `Accepted=False/reason=Invalid` condition (empty = OK).
    pub fn validate_link_sys(&self) -> Vec<String> {
        let Some(link_sys) = &self.link_sys else {
            return Vec::new();
        };
        if (1..=LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT).contains(&link_sys.max_instances_per_kind) {
            Vec::new()
        } else {
            vec![format!(
                "linkSys.maxInstancesPerKind must be between 1 and {LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT}, got {}",
                link_sys.max_instances_per_kind
            )]
        }
    }

    /// Validate `spec.dnsResolver` at ingest. Returns error strings suitable for
    /// an `Accepted=False/reason=Invalid` condition (empty = OK).
    ///
    /// The structural CRD cannot express `oneOf`, so `linkSysRef` and `servers`
    /// both stay optional in YAML and their mutual exclusion is enforced here.
    pub fn validate_dns_resolver(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(dr) = &self.dns_resolver {
            let has_ref = dr.link_sys_ref.is_some();
            let has_servers = dr.servers.as_ref().is_some_and(|s| !s.is_empty());
            if has_ref && has_servers {
                errors
                    .push("dnsResolver: `linkSysRef` and `servers` are mutually exclusive (set only one)".to_string());
            }
        }
        errors
    }
}

#[cfg(test)]
mod tcp_timeout_tests {
    use super::*;
    use crate::duration::parse_duration;
    use std::time::Duration;

    #[test]
    fn tcp_timeout_absent_section_is_none() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
        assert!(spec.tcp_timeout.is_none());
    }

    #[test]
    fn tcp_timeout_idle_defaults_to_1h_when_field_omitted() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
        assert_eq!(spec.tcp_timeout.unwrap().idle_timeout, "1h");
    }

    #[test]
    fn tcp_timeout_idle_explicit_value_round_trips() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {"idleTimeout": "300s"}}"#).unwrap();
        assert_eq!(spec.tcp_timeout.unwrap().idle_timeout, "300s");
    }

    #[test]
    fn default_spec_has_no_tcp_timeout() {
        assert!(EdgionGatewayConfigSpec::default().tcp_timeout.is_none());
    }

    #[test]
    fn resolve_absent_section_uses_secure_default() {
        assert_eq!(resolve_tcp_idle_timeout(None), Some(DEFAULT_TCP_IDLE));
    }

    #[test]
    fn resolve_omitted_field_uses_serde_default_1h() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
        assert_eq!(
            resolve_tcp_idle_timeout(spec.tcp_timeout.as_ref()),
            Some(Duration::from_secs(3600))
        );
    }

    #[test]
    fn resolve_explicit_value() {
        let t = TcpTimeout {
            idle_timeout: "300s".to_string(),
            connect_timeout: default_tcp_connect_timeout(),
        };
        assert_eq!(resolve_tcp_idle_timeout(Some(&t)), Some(Duration::from_secs(300)));
    }

    #[test]
    fn resolve_zero_is_unbounded_optout() {
        // Any zero-valued GEP-2257 duration opts out; a bare "0" is not a
        // valid duration (see resolve_parse_error_fails_safe_to_default).
        for s in ["0s", "0m", "0h", "0ms"] {
            let t = TcpTimeout {
                idle_timeout: s.to_string(),
                connect_timeout: default_tcp_connect_timeout(),
            };
            assert_eq!(resolve_tcp_idle_timeout(Some(&t)), None, "{s} should opt out");
        }
    }

    #[test]
    fn resolve_parse_error_fails_safe_to_default() {
        // "0" (bare number) and "0.0s" (decimal) are NOT valid GEP-2257 and
        // must NOT opt out — they fail safe to the bounded default.
        for s in ["not-a-duration", "0", "0.0s"] {
            let t = TcpTimeout {
                idle_timeout: s.to_string(),
                connect_timeout: default_tcp_connect_timeout(),
            };
            assert_eq!(
                resolve_tcp_idle_timeout(Some(&t)),
                Some(DEFAULT_TCP_IDLE),
                "{s} should fail safe to default"
            );
        }
    }

    #[test]
    fn default_const_and_serde_default_stay_in_sync() {
        assert_eq!(parse_duration(&default_tcp_idle_timeout()).unwrap(), DEFAULT_TCP_IDLE);
    }

    // --- connect_timeout (mirrors the idle_timeout cases above) ---

    #[test]
    fn tcp_timeout_connect_defaults_to_10s_when_field_omitted() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
        assert_eq!(spec.tcp_timeout.unwrap().connect_timeout, "10s");
    }

    #[test]
    fn tcp_timeout_connect_explicit_value_round_trips() {
        let spec: EdgionGatewayConfigSpec =
            serde_json::from_str(r#"{"tcpTimeout": {"connectTimeout": "3s"}}"#).unwrap();
        assert_eq!(spec.tcp_timeout.unwrap().connect_timeout, "3s");
    }

    #[test]
    fn resolve_connect_absent_section_uses_secure_default() {
        assert_eq!(resolve_tcp_connect_timeout(None), Some(DEFAULT_TCP_CONNECT));
    }

    #[test]
    fn resolve_connect_omitted_field_uses_serde_default_10s() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
        assert_eq!(
            resolve_tcp_connect_timeout(spec.tcp_timeout.as_ref()),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn resolve_connect_explicit_value() {
        let t = TcpTimeout {
            idle_timeout: default_tcp_idle_timeout(),
            connect_timeout: "3s".to_string(),
        };
        assert_eq!(resolve_tcp_connect_timeout(Some(&t)), Some(Duration::from_secs(3)));
    }

    #[test]
    fn resolve_connect_zero_is_unbounded_optout() {
        // Any zero-valued GEP-2257 duration opts out; a bare "0" is not a
        // valid duration (see resolve_connect_parse_error_fails_safe_to_default).
        for s in ["0s", "0m", "0h", "0ms"] {
            let t = TcpTimeout {
                idle_timeout: default_tcp_idle_timeout(),
                connect_timeout: s.to_string(),
            };
            assert_eq!(resolve_tcp_connect_timeout(Some(&t)), None, "{s} should opt out");
        }
    }

    #[test]
    fn resolve_connect_parse_error_fails_safe_to_default() {
        // "0" (bare number) and "0.0s" (decimal) are NOT valid GEP-2257 and
        // must NOT opt out — they fail safe to the bounded default.
        for s in ["not-a-duration", "0", "0.0s"] {
            let t = TcpTimeout {
                idle_timeout: default_tcp_idle_timeout(),
                connect_timeout: s.to_string(),
            };
            assert_eq!(
                resolve_tcp_connect_timeout(Some(&t)),
                Some(DEFAULT_TCP_CONNECT),
                "{s} should fail safe to default"
            );
        }
    }

    #[test]
    fn connect_default_const_and_serde_default_stay_in_sync() {
        assert_eq!(
            parse_duration(&default_tcp_connect_timeout()).unwrap(),
            DEFAULT_TCP_CONNECT
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_keepalive_request_limit_defaults_to_ten_thousand() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "server": {}
        }))
        .unwrap();

        assert_eq!(spec.server.unwrap().downstream_keepalive_request_limit, 10_000);
    }

    #[test]
    fn preflight_policy_accepts_only_ordinary_final_statuses() {
        let valid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "preflightPolicy": {"statusCode": 204}
        }))
        .unwrap();
        assert!(valid.validate_preflight_policy().is_empty());

        for status in [100, 101, 103, 600, 700] {
            let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
                "preflightPolicy": {"statusCode": status}
            }))
            .unwrap();
            assert_eq!(invalid.validate_preflight_policy().len(), 1, "{status}");
        }
    }

    #[test]
    fn forwarded_headers_default_to_x_real_ip() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.forwarded_headers.remote_ip_header, "X-Real-IP");
        assert!(spec.validate_forwarded_headers().is_empty());
    }

    #[test]
    fn forwarded_headers_accept_custom_remote_ip_header() {
        let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "forwardedHeaders": {"remoteIpHeader": "X-Edgion-Remote-IP"}
        }))
        .unwrap();
        assert_eq!(spec.forwarded_headers.remote_ip_header, "X-Edgion-Remote-IP");
        assert!(spec.validate_forwarded_headers().is_empty());
    }

    #[test]
    fn forwarded_headers_reject_invalid_and_reserved_remote_ip_headers() {
        for name in [
            "",
            "bad header",
            "Host",
            "Content-Length",
            "Connection",
            "X-Forwarded-For",
        ] {
            let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
                "forwardedHeaders": {"remoteIpHeader": name}
            }))
            .unwrap();
            assert_eq!(spec.validate_forwarded_headers().len(), 1, "{name} must be rejected");
        }
    }

    #[test]
    fn egc_outbound_tls_roundtrips() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "outboundTls": {"verify": false}
        }))
        .unwrap();
        assert!(!s.outbound_tls.unwrap().verify);
    }

    #[test]
    fn request_body_defaults_are_gateway_class_policy() {
        let s: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
        assert!(s.request_body.enabled);
        assert_eq!(s.request_body.default_memory_buffer_size, "128KiB");
        assert_eq!(s.request_body.default_max_body_size, "32MiB");
        assert_eq!(s.request_body.storage_operation_timeout, "5s");
        assert!(s.validate_request_body().is_empty());
    }

    #[test]
    fn request_body_overrides_parse_and_validate() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "requestBody": {
                "maxMemoryBufferSize": "256KiB",
                "maxBodySize": "8MiB",
                "storageOperationTimeout": "750ms"
            }
        }))
        .unwrap();
        let request_body = s.request_body;
        assert_eq!(request_body.max_memory_buffer_size.as_deref(), Some("256KiB"));
        assert_eq!(request_body.max_body_size.as_deref(), Some("8MiB"));
        assert_eq!(request_body.storage_operation_timeout, "750ms");
        assert!(request_body.validate().is_empty());
    }

    #[test]
    fn request_body_rejects_invalid_and_inverted_limits() {
        let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "requestBody": {
                "maxMemoryBufferSize": "garbage",
                "maxBodySize": "0"
            }
        }))
        .unwrap();
        assert_eq!(invalid.validate_request_body().len(), 2);

        let invalid_override: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "requestBody": {
                "maxMemoryBufferSize": "garbage",
                "maxBodySize": "1"
            }
        }))
        .unwrap();
        assert_eq!(invalid_override.validate_request_body().len(), 1);

        let inverted: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "requestBody": {
                "maxMemoryBufferSize": "2MiB",
                "maxBodySize": "1MiB"
            }
        }))
        .unwrap();
        assert_eq!(
            inverted.validate_request_body(),
            vec!["effective requestBody memory buffer size must not exceed the effective requestBody max body size"]
        );

        for timeout in ["0s", "invalid"] {
            let invalid_timeout: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
                "requestBody": { "storageOperationTimeout": timeout }
            }))
            .unwrap();
            assert_eq!(invalid_timeout.validate_request_body().len(), 1, "{timeout}");
        }
    }

    #[test]
    fn egc_dns_resolver_absent_is_none() {
        let s: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
        assert!(s.dns_resolver.is_none());
        assert!(s.validate_dns_resolver().is_empty());
    }

    #[test]
    fn egc_dns_resolver_link_sys_ref_roundtrips() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "dnsResolver": {"linkSysRef": {"namespace": "dns", "name": "aliyun"}}
        }))
        .unwrap();
        let dr = s.dns_resolver.as_ref().unwrap();
        let r = dr.link_sys_ref.as_ref().unwrap();
        assert_eq!(r.namespace, "dns");
        assert_eq!(r.name, "aliyun");
        // cacheTtl defaults to 5s when omitted.
        assert_eq!(dr.cache_ttl, "5s");
        assert!(s.validate_dns_resolver().is_empty());
    }

    #[test]
    fn egc_dns_resolver_servers_roundtrip_and_cache_ttl() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "dnsResolver": {"servers": ["1.1.1.1", "8.8.8.8:5353"], "cacheTtl": "10s"}
        }))
        .unwrap();
        let dr = s.dns_resolver.as_ref().unwrap();
        assert_eq!(dr.servers.as_ref().unwrap().len(), 2);
        assert_eq!(dr.cache_ttl, "10s");
        assert!(s.validate_dns_resolver().is_empty());
    }

    #[test]
    fn egc_dns_resolver_ref_and_servers_are_mutually_exclusive() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "dnsResolver": {
                "linkSysRef": {"namespace": "dns", "name": "aliyun"},
                "servers": ["1.1.1.1"]
            }
        }))
        .unwrap();
        let errs = s.validate_dns_resolver();
        assert!(
            errs.iter().any(|e| e.contains("mutually exclusive")),
            "both linkSysRef and servers set must be rejected, got: {errs:?}"
        );
    }

    #[test]
    fn link_sys_max_instances_defaults_and_validates() {
        let omitted: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "linkSys": {"webhookMaxResponseBytes": 16384}
        }))
        .unwrap();
        assert_eq!(
            omitted.link_sys.as_ref().unwrap().max_instances_per_kind,
            LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND
        );
        assert!(omitted.validate_link_sys().is_empty());

        let valid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "linkSys": {"maxInstancesPerKind": 50}
        }))
        .unwrap();
        assert_eq!(valid.link_sys.as_ref().unwrap().max_instances_per_kind, 50);
        assert!(valid.validate_link_sys().is_empty());

        for value in [0_usize, LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT + 1] {
            let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
                "linkSys": {"maxInstancesPerKind": value}
            }))
            .unwrap();
            let errs = invalid.validate_link_sys();
            assert_eq!(errs.len(), 1, "{value} must be rejected, got: {errs:?}");
            assert!(errs[0].contains("maxInstancesPerKind"), "{errs:?}");
        }
    }

    #[test]
    fn test_deserialize_gateway_config() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-config
spec:
  httpTimeout:
    client:
      readTimeout: 60s
      writeTimeout: 60s
    backend:
      defaultConnectTimeout: 5s
      defaultRequestTimeout: 60s
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Failed to parse YAML");
        assert_eq!(config.metadata.name, Some("test-config".to_string()));

        let spec = config.spec;
        assert!(spec.http_timeout.is_some());
    }

    #[test]
    fn retry_defaults_to_two_attempts() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-default
spec: {}
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should parse defaults");
        assert_eq!(config.spec.retry.attempts, 2);
    }

    #[test]
    fn retry_attempts_parse_and_validate_at_domain_boundary() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-retry
spec:
  retry:
    attempts: 10
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should parse");
        assert_eq!(config.spec.retry.attempts, 10);
        assert!(config.spec.validate_retry().is_empty());

        let mut spec = EdgionGatewayConfigSpec::default();
        spec.retry.attempts = MAX_RETRY_ATTEMPTS;
        assert!(spec.validate_retry().is_empty());

        spec.retry.attempts = MAX_RETRY_ATTEMPTS + 1;
        let errors = spec.validate_retry();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("retry.attempts"));
    }
}

#[cfg(test)]
mod access_log_extern_policy_tests {
    use super::*;
    use crate::resources::edgion_plugins::LogExtSource;

    #[test]
    fn default_whitelist_allows_safe_headers_only() {
        let w = UnmaskedKeys::default();
        assert!(w.allows(LogExtSource::Header, "user-agent"));
        assert!(w.allows(LogExtSource::Header, "content-type"));
        // Header matching is case-insensitive (HTTP semantics).
        assert!(w.allows(LogExtSource::Header, "User-Agent"));

        // `referer` leaks someone else's full URL (session ids in the query);
        // `location` carries OAuth implicit tokens and reset links. Neither is a
        // default. Both remain addable by an operator.
        assert!(!w.allows(LogExtSource::Header, "referer"));
        assert!(!w.allows(LogExtSource::RespHeader, "location"));
        // Weak PII / noise.
        assert!(!w.allows(LogExtSource::Header, "accept-language"));
        assert!(!w.allows(LogExtSource::Header, "accept-encoding"));
    }

    #[test]
    fn default_whitelist_is_empty_for_business_defined_sources() {
        // No key name is universally safe for these — operators opt in per cluster.
        let w = UnmaskedKeys::default();
        assert!(!w.allows(LogExtSource::Query, "page"));
        assert!(!w.allows(LogExtSource::Cookie, "session"));
        assert!(!w.allows(LogExtSource::Ctx, "tenant_id"));
    }

    #[test]
    fn category_one_sources_are_never_consulted() {
        // routeLabel/routeAnnotation bypass the whitelist entirely upstream of
        // this call; `allows` must still answer false so a caller that wrongly
        // consults it cannot accidentally widen anything.
        let w = UnmaskedKeys::default();
        assert!(!w.allows(LogExtSource::RouteLabel, "app"));
        assert!(!w.allows(LogExtSource::RouteAnnotation, "owner"));
    }

    #[test]
    fn cookie_and_ctx_names_are_case_sensitive() {
        let w = UnmaskedKeys {
            cookie: vec!["Session".to_string()],
            ctx: vec!["tenant_id".to_string()],
            ..Default::default()
        };
        assert!(w.allows(LogExtSource::Cookie, "Session"));
        assert!(!w.allows(LogExtSource::Cookie, "session"));
        assert!(w.allows(LogExtSource::Ctx, "tenant_id"));
        assert!(!w.allows(LogExtSource::Ctx, "TENANT_ID"));
    }

    #[test]
    fn operator_overrides_only_the_named_list() {
        let policy: AccessLogExternPolicy = serde_yaml::from_str("unmaskedKeys:\n  cookie: [tier]\n").unwrap();
        // The named list is replaced...
        assert!(policy.unmasked_keys.allows(LogExtSource::Cookie, "tier"));
        // ...and the unnamed ones keep their defaults.
        assert!(policy.unmasked_keys.allows(LogExtSource::Header, "user-agent"));
    }
}
