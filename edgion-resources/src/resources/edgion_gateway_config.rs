//! EdgionGatewayConfig CRD definition
//!
//! This model defines the EdgionGatewayConfig custom resource, which is used
//! as parametersRef in GatewayClass to provide gateway-wide configuration.

use super::common::{Condition, DnsLookupFamily};
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
    /// Default downstream TLS policy for listeners bound to this configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downstream_tls: Option<super::downstream_tls::DownstreamTlsConfig>,

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

    /// GatewayClass-wide policy for optional expensive WAF checks.
    #[serde(default)]
    pub waf: GatewayWafConfig,

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

/// Optional expensive checks, independent of normal JSON/XML inspection.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayWafConfig {
    /// Execute classified checks that retain cross-field aggregate records.
    /// Default false; mandatory parsing and ordinary attack rules remain active.
    #[serde(default)]
    pub expensive_checks: bool,
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
    /// Backend DNS address families; independent of resolver selection.
    #[serde(default)]
    pub lookup_family: DnsLookupFamily,
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
    /// Runtime capture switch; admission limits remain independent.
    #[serde(default = "default_request_body_enabled")]
    pub enabled: bool,
    /// Hard ceiling on the original request entity payload.
    #[serde(default = "default_request_body_max_size")]
    pub max_body_size: String,
    #[serde(default)]
    pub storage: RequestBodyStorageConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestBodyStorageConfig {
    /// Resident payload threshold before optional disk spill.
    #[serde(default = "default_memory_buffer_size")]
    pub memory_limit: String,
    #[serde(default = "default_request_body_enabled")]
    pub spill_to_disk: bool,
    /// New owner retention capacity; omission inherits maxBodySize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_buffered_size: Option<String>,
    /// Admission plus execution deadline for one storage file operation.
    #[serde(default = "default_request_body_storage_operation_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub operation_timeout: String,
}

impl RequestBodyConfig {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let body = validate_positive_byte_size(Some(&self.max_body_size), "requestBody.maxBodySize", &mut errors);
        let memory = validate_positive_byte_size(
            Some(&self.storage.memory_limit),
            "requestBody.storage.memoryLimit",
            &mut errors,
        );
        let buffered = match self.storage.max_buffered_size.as_deref() {
            Some(value) => validate_positive_byte_size(Some(value), "requestBody.storage.maxBufferedSize", &mut errors),
            None => body,
        };
        if let (Some(memory), Some(buffered)) = (memory, buffered) {
            if memory > buffered {
                errors.push(
                    "requestBody.storage.memoryLimit must not exceed maxBufferedSize (or inherited maxBodySize)"
                        .to_string(),
                );
            }
        }
        if let (Some(buffered), Some(body)) = (buffered, body) {
            if buffered > body {
                errors.push("requestBody.storage.maxBufferedSize must not exceed requestBody.maxBodySize".to_string());
            }
        }
        match crate::duration::parse_duration(&self.storage.operation_timeout) {
            Ok(timeout) if timeout.is_zero() => {
                errors.push("requestBody.storage.operationTimeout must be greater than zero".to_string())
            }
            Ok(_) => {}
            Err(error) => errors.push(format!(
                "requestBody.storage.operationTimeout must be a valid duration: {error}"
            )),
        }
        errors
    }
}

impl Default for RequestBodyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_body_size: default_request_body_max_size(),
            storage: RequestBodyStorageConfig::default(),
        }
    }
}

impl Default for RequestBodyStorageConfig {
    fn default() -> Self {
        Self {
            memory_limit: default_memory_buffer_size(),
            spill_to_disk: true,
            max_buffered_size: None,
            operation_timeout: default_request_body_storage_operation_timeout(),
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
#[path = "../../tests/unit/resources/edgion_gateway_config_test/tcp_timeout_tests.rs"]
mod tcp_timeout_tests;

#[cfg(test)]
#[path = "../../tests/unit/resources/edgion_gateway_config_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/resources/edgion_gateway_config_test/access_log_extern_policy_tests.rs"]
mod access_log_extern_policy_tests;
