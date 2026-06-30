//! EdgionGatewayConfig CRD definition
//!
//! This model defines the EdgionGatewayConfig custom resource, which is used
//! as parametersRef in GatewayClass to provide gateway-wide configuration.

use super::common::Condition;
use super::edgion_plugins::RealIpConfig;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// EdgionGatewayConfig is the configuration for a GatewayClass
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
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
    /// Server configuration for Pingora
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerConfig>,

    /// HTTP timeout configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_timeout: Option<HttpTimeout>,

    /// Maximum number of retries for upstream connections (default: 3)
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Real IP extraction configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_ip: Option<RealIpConfig>,

    /// Security protection configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_protect: Option<SecurityProtectConfig>,

    /// Global plugins references that apply to all routes using this GatewayClass
    /// These plugins are executed before route-level plugins
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_plugins_ref: Option<Vec<PluginReference>>,

    /// Preflight request handling policy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_policy: Option<PreflightPolicy>,

    /// Enable ReferenceGrant validation for cross-namespace references (default: false)
    /// When enabled, cross-namespace references in Routes and Gateways will be validated
    /// against ReferenceGrant policies
    #[serde(default)]
    pub enable_reference_grant_validation: bool,

    /// Global LinkSys provider configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_sys: Option<LinkSysGlobalConfig>,

    /// Timeout configuration for L4 TCP-transport relays
    /// (plain TCP, TLS-terminated, TLS-passthrough). UDP has its own
    /// session model and is configured separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_timeout: Option<TcpTimeout>,

    /// Load-balancing-wide settings (panic threshold, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancing: Option<LoadBalancingConfig>,

    /// Cluster-wide default for plugin / LinkSys outbound TLS verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_tls: Option<crate::resources::common::OutboundTlsConfig>,
}

// ============================================
// Server Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Number of worker threads (default: number of CPU cores)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,

    /// Enable work stealing (default: true)
    #[serde(default = "default_work_stealing")]
    pub work_stealing: bool,

    /// Grace period for shutdown in seconds (default: 30)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_period_seconds: Option<u64>,

    /// Graceful shutdown timeout in seconds (default: 10)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graceful_shutdown_timeout_s: Option<u64>,

    /// Upstream keepalive pool size (default: 128)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_keepalive_pool_size: Option<u32>,

    /// Error log file path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_log: Option<String>,

    /// Enable downstream response compression (default: false)
    #[serde(default)]
    pub enable_compression: bool,

    /// Maximum number of requests a downstream connection can serve before being closed.
    /// Per-connection limit, similar to Nginx's `keepalive_requests`.
    /// Helps with memory management and load balancing distribution.
    /// Default: 1000. Set to 0 to disable the limit.
    #[serde(default = "default_downstream_keepalive_request_limit")]
    pub downstream_keepalive_request_limit: u32,
}

fn default_downstream_keepalive_request_limit() -> u32 {
    1000
}

fn default_work_stealing() -> bool {
    true
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
    /// Without unit defaults to seconds (e.g., "60" = "60s")
    #[serde(default = "default_client_read_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub read_timeout: String,

    /// Timeout for writing response to client
    /// Format: Duration string (e.g., "60s", "1m", "500ms")
    /// Without unit defaults to seconds (e.g., "60" = "60s")
    #[serde(default = "default_client_write_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub write_timeout: String,

    /// HTTP keepalive timeout
    /// Format: Duration string (e.g., "75s", "1m", "500ms")
    /// Without unit defaults to seconds (e.g., "75" = "75s")
    #[serde(default = "default_client_keepalive_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub keepalive_timeout: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendTimeout {
    /// Timeout for connecting to upstream backend
    /// Format: Duration string (e.g., "5s", "1m", "500ms")
    /// Without unit defaults to seconds (e.g., "5" = "5s")
    #[serde(default = "default_backend_connect_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub default_connect_timeout: String,

    /// Default per-attempt backend request timeout — the fallback for the
    /// Gateway API `timeouts.backendRequest` field when a route does not set
    /// it. Applied per upstream attempt (NOT including retries); with a retry
    /// policy present it also acts as the per-try timeout (GEP-1731): an
    /// attempt exceeding it is canceled and retried. `"0s"` disables the bound.
    /// Format: Duration string (e.g., "60s", "1m", "500ms")
    /// Without unit defaults to seconds (e.g., "60" = "60s")
    #[serde(default = "default_backend_request_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub default_request_timeout: String,

    /// Idle timeout for backend connection pool
    /// Format: Duration string (e.g., "300s", "5m", "500ms")
    /// Without unit defaults to seconds (e.g., "300" = "300s")
    #[serde(default = "default_backend_idle_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub default_idle_timeout: String,

    /// **DEPRECATED**: Use top-level `maxRetries` in EdgionGatewayConfigSpec instead.
    /// This field is kept temporarily for backward compatibility detection.
    /// If present in config, a warning will be logged during config loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_retries: Option<u32>,
}

// ============================================
// TCP (L4) Timeout Configuration
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TcpTimeout {
    /// Idle timeout for the established L4 relay. The activity clock resets on
    /// any byte progress in either direction (a completed read OR an advancing
    /// partial write). "0" / "0s" disables it (unbounded — risky). Applies to
    /// plain TCP, TLS-terminated, and TLS-passthrough relays. UDP is not affected.
    /// Format: duration string (e.g. "1h", "300s", "5m"); a bare number is
    /// interpreted as seconds. Default "1h".
    #[serde(default = "default_tcp_idle_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
    pub idle_timeout: String,

    /// Upper bound for establishing the upstream TCP connection (the TCP
    /// handshake) on an L4 route. Mirrors the *mechanism* of the HTTP path's
    /// `backend.defaultConnectTimeout` (a per-peer connect bound); the default
    /// value differs — 10s here vs the HTTP path's 5s, matching the Istio L4
    /// `connectTimeout` baseline. A backend that silently drops SYNs (firewall,
    /// not-yet-ready pod) fails fast instead of stranding the downstream
    /// connection until the OS TCP retransmit timeout (~2m) fires.
    /// "0" / "0s" disables it (unbounded — relies on the OS bound). Applies to
    /// plain TCP, TLS-terminated, and TLS-passthrough relays — in all three the
    /// upstream connect is plain TCP (TLS-terminated terminates downstream TLS
    /// separately; TLS-passthrough carries the end-to-end TLS handshake over the
    /// established relay), so this bounds the connect itself, not a TLS
    /// handshake. UDP is not affected.
    /// Format: duration string (e.g. "10s", "1m", "500ms"); a bare number is
    /// interpreted as seconds. Default "10s".
    #[serde(default = "default_tcp_connect_timeout")]
    #[schemars(pattern(r"^([0-9]+(\.[0-9]+)?(milliseconds|millisecond|millis|ms|seconds|second|secs|sec|s|minutes|minute|mins|min|m|hours|hour|hrs|hr|h|days|day|d))+$|^[0-9]+(\.[0-9]+)?$"))]
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
    /// Panic threshold percent (0-100). When the fraction of healthy backends
    /// for a service drops below this percent, the gateway ignores health
    /// filtering and load-balances across all backends. 0 (default) disables
    /// panic mode. Per-service `edgion.io/panic-threshold` overrides this.
    #[serde(default)]
    pub panic_threshold: u8,
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
/// - explicit `"0"` / `"0s"` / any zero duration  -> `None` (operator opt-out, unbounded)
/// - any other valid duration                     -> `Some(parsed)`
/// - parse error (schema-validated upstream)       -> `Some(DEFAULT_TCP_IDLE)` (fail-safe, silent)
///
/// Runs per connection on the data plane; intentionally emits no `tracing`.
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
/// - explicit `"0"` / `"0s"` / any zero duration  -> `None` (operator opt-out, unbounded)
/// - any other valid duration                     -> `Some(parsed)`
/// - parse error (schema-validated upstream)       -> `Some(DEFAULT_TCP_CONNECT)` (fail-safe, silent)
///
/// Runs per connection on the data plane; intentionally emits no `tracing`.
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
fn default_max_retries() -> u32 {
    3
}

impl Default for EdgionGatewayConfigSpec {
    fn default() -> Self {
        Self {
            server: None,
            http_timeout: None,
            max_retries: default_max_retries(),
            real_ip: None,
            security_protect: None,
            global_plugins_ref: None,
            preflight_policy: None,
            enable_reference_grant_validation: false,
            link_sys: None,
            tcp_timeout: None,
            load_balancing: None,
            outbound_tls: None,
        }
    }
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
            default_max_retries: None,
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

/// Global configuration for LinkSys providers
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysGlobalConfig {
    /// Global hard cap for webhook response body size (bytes).
    /// Per-resource `maxResponseBytes` is clamped to this value.
    /// Default: 16384 (16KB)
    #[serde(default = "default_webhook_max_response_bytes")]
    pub webhook_max_response_bytes: usize,
}

fn default_webhook_max_response_bytes() -> usize {
    16384
}

impl EdgionGatewayConfigSpec {
    /// Check for deprecated fields and log warnings.
    pub fn check_deprecated_fields(&self) {
        if let Some(ref http_timeout) = self.http_timeout {
            if let Some(old_retries) = http_timeout.backend.default_max_retries {
                tracing::warn!(
                    deprecated_field = "httpTimeout.backend.defaultMaxRetries",
                    old_value = old_retries,
                    current_field = "maxRetries",
                    current_value = self.max_retries,
                    "EdgionGatewayConfig: 'httpTimeout.backend.defaultMaxRetries' is deprecated and will be removed. \
                     Please migrate to top-level 'maxRetries'. The deprecated value ({}) is ignored; \
                     current effective value is maxRetries={}.",
                    old_retries,
                    self.max_retries,
                );
            }
        }
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
        for s in ["0", "0s", "0m", "0.0s"] {
            let t = TcpTimeout {
                idle_timeout: s.to_string(),
                connect_timeout: default_tcp_connect_timeout(),
            };
            assert_eq!(resolve_tcp_idle_timeout(Some(&t)), None, "{s} should opt out");
        }
    }

    #[test]
    fn resolve_parse_error_fails_safe_to_default() {
        let t = TcpTimeout {
            idle_timeout: "not-a-duration".to_string(),
            connect_timeout: default_tcp_connect_timeout(),
        };
        assert_eq!(resolve_tcp_idle_timeout(Some(&t)), Some(DEFAULT_TCP_IDLE));
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
        for s in ["0", "0s", "0m", "0.0s"] {
            let t = TcpTimeout {
                idle_timeout: default_tcp_idle_timeout(),
                connect_timeout: s.to_string(),
            };
            assert_eq!(resolve_tcp_connect_timeout(Some(&t)), None, "{s} should opt out");
        }
    }

    #[test]
    fn resolve_connect_parse_error_fails_safe_to_default() {
        let t = TcpTimeout {
            idle_timeout: default_tcp_idle_timeout(),
            connect_timeout: "not-a-duration".to_string(),
        };
        assert_eq!(resolve_tcp_connect_timeout(Some(&t)), Some(DEFAULT_TCP_CONNECT));
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
    fn egc_outbound_tls_roundtrips() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "outboundTls": {"verify": false}
        }))
        .unwrap();
        assert_eq!(s.outbound_tls.unwrap().verify, false);
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
    fn test_deprecated_default_max_retries_accepted() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-deprecated
spec:
  httpTimeout:
    backend:
      defaultConnectTimeout: 5s
      defaultMaxRetries: 5
  maxRetries: 7
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should accept deprecated field");
        assert_eq!(config.spec.max_retries, 7);
        assert_eq!(
            config.spec.http_timeout.as_ref().unwrap().backend.default_max_retries,
            Some(5)
        );
    }

    #[test]
    fn test_no_deprecated_field_is_none() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-new
spec:
  maxRetries: 10
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should parse");
        assert_eq!(config.spec.max_retries, 10);
        assert!(config.spec.http_timeout.is_none());
    }
}
