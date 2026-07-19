//! EdgionGatewayConfig CRD definition
//!
//! This model defines the EdgionGatewayConfig custom resource, which is used
//! as parametersRef in GatewayClass to provide gateway-wide configuration.

use super::common::Condition;
use super::edgion_plugins::plugin_configs::LinkSysRef;
use super::edgion_plugins::{LogExtSource, RealIpConfig};
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

    /// Gateway-wide maximum HTTP body size buffered for body-aware plugins
    /// (human size, e.g. "32MiB"). This is the shared per-request buffer size and
    /// the global body-size limit; a per-plugin `body.maxBodySize` can only
    /// tighten it further, never enlarge it.
    #[serde(default = "default_max_body_size")]
    pub max_body_size: String,

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

    /// Load-balancing-wide settings (panic threshold, etc.).
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

fn default_dns_cache_ttl() -> String {
    "5s".to_string()
}

// ============================================
// Body Size Configuration
// ============================================

fn default_max_body_size() -> String {
    "32MiB".to_string()
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
    /// Panic threshold percent (0-100). When the fraction of healthy backends
    /// for a service drops below this percent, the gateway ignores health
    /// filtering and load-balances across all backends. 0 (default) disables
    /// panic mode. An EdgionBackendTrafficPolicy's `loadBalancer.panicThreshold`
    /// overrides this for its target Services.
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
/// - explicit `"0s"` / any zero-valued duration   -> `None` (operator opt-out, unbounded)
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
/// - explicit `"0s"` / any zero-valued duration   -> `None` (operator opt-out, unbounded)
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
            max_body_size: default_max_body_size(),
            global_plugins_ref: None,
            access_log_extern: None,
            preflight_policy: None,
            link_sys: None,
            tcp_timeout: None,
            load_balancing: None,
            outbound_tls: None,
            dns_resolver: None,
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

// ============================================
// Access Log Extension Policy (masking whitelist)
// ============================================

/// Access log extension policy. Platform-owned: it lives on the cluster-scoped
/// `EdgionGatewayConfig` (referenced via `GatewayClass.spec.parametersRef`) so a
/// tenant who owns a route and its `EdgionPlugins` cannot widen masking.
///
/// Scope caveat: `bound_listeners` dedup (`runtime/server/base.rs`) skips a
/// listener already bound at the same addr/protocol, so two Gateways of
/// different GatewayClasses sharing an addr:port share the first one's config.
/// The whitelist is therefore effectively per-listener, not per-Gateway.
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
    /// Resolved gateway-wide max body size in bytes. Falls back to the built-in
    /// default if `maxBodySize` is unparseable or zero (a `"0"` cap would reject
    /// every body under lenient admission; `validate_max_body_size` flags it but
    /// the resource is still synced).
    pub fn max_body_size_bytes(&self) -> u64 {
        crate::resources::common::parse_byte_size(&self.max_body_size)
            .filter(|v| *v > 0)
            .unwrap_or(crate::resources::edgion_plugins::body_requirement::DEFAULT_MAX_BODY_SIZE_BYTES)
    }

    /// Validation error for `spec.maxBodySize`, or `None` if it parses to a
    /// positive size.
    pub fn validate_max_body_size(&self) -> Option<String> {
        match crate::resources::common::parse_byte_size(&self.max_body_size) {
            Some(v) if v > 0 => None,
            _ => Some(format!(
                "maxBodySize '{}' is invalid (expected e.g. '32MiB')",
                self.max_body_size
            )),
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
    fn egc_outbound_tls_roundtrips() {
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "outboundTls": {"verify": false}
        }))
        .unwrap();
        assert_eq!(s.outbound_tls.unwrap().verify, false);
    }

    #[test]
    fn max_body_size_default_and_parsing() {
        // Default when unset.
        let s: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(s.max_body_size, "32MiB");
        assert_eq!(s.max_body_size_bytes(), 32 * 1024 * 1024);
        assert!(s.validate_max_body_size().is_none());

        // Human size parses + round-trips through the spec.
        let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "maxBodySize": "128m"
        }))
        .unwrap();
        assert_eq!(s.max_body_size_bytes(), 128 * 1024 * 1024);
        assert!(s.validate_max_body_size().is_none());
    }

    #[test]
    fn max_body_size_unparseable_or_zero_falls_back_to_builtin() {
        for bad in ["garbage", "0", "0KiB"] {
            let s = EdgionGatewayConfigSpec {
                max_body_size: bad.to_string(),
                ..Default::default()
            };
            // Accessor falls back to the built-in default (never a 0 cap that
            // would 413 every body)...
            assert_eq!(s.max_body_size_bytes(), 32 * 1024 * 1024, "for {bad:?}");
            // ...but validation surfaces the error for operator feedback.
            assert!(s.validate_max_body_size().is_some(), "validation for {bad:?}");
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
