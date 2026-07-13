use anyhow::{Context, Result};
use clap::Args;
use edgion_resources::rate_limit_config::RateLimitGlobalConfig;
use edgion_resources::LogConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default admin API listen address. Binds loopback by default so the admin API
/// (which serves secret-bearing `/configclient` introspection) is not exposed to the
/// cluster unless the operator opts in via `admin_listen`. Liveness/readiness live on
/// the separate probe port (`DEFAULT_GATEWAY_PROBE_LISTEN`), so kubelet never needs to
/// reach the admin port and the shipped manifests keep this on loopback.
/// Port is in the Gateway management block (120xx): 12001 = admin.
pub const DEFAULT_GATEWAY_ADMIN_LISTEN: &str = "127.0.0.1:12001";

/// Default probe API listen address. The probe API serves only the non-sensitive
/// liveness/readiness endpoints (`/health`, `/ready`) and binds `0.0.0.0` by default
/// so a kubelet `httpGet` probe can reach it over the pod IP. The sensitive admin API
/// stays loopback (`DEFAULT_GATEWAY_ADMIN_LISTEN`) and need never be `0.0.0.0`.
/// Port is in the Gateway management block (120xx): 12000 = probe.
pub const DEFAULT_GATEWAY_PROBE_LISTEN: &str = "0.0.0.0:12000";

/// Edgion Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
#[serde(default)]
pub struct EdgionGatewayConfig {
    /// Working directory for Edgion runtime files
    /// Priority: CLI --work-dir > ENV EDGION_WORK_DIR > Config > Default (".")
    /// All relative paths in configuration will be relative to this directory.
    #[arg(
        short = 'w',
        long,
        value_name = "DIR",
        help = "Working directory for Edgion runtime files\n\
                Priority: CLI > ENV (EDGION_WORK_DIR) > Config > Default\n\
                Example: --work-dir /usr/local/edgion"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<PathBuf>,

    /// Configuration file path (YAML format)
    #[arg(
        short = 'c',
        long = "config-file",
        value_name = "FILE",
        default_value = "config/edgion-gateway.yaml"
    )]
    #[serde(skip)]
    pub config_file: Option<String>,

    #[command(flatten)]
    #[serde(default)]
    pub gateway: GatewayConfig,

    #[command(flatten)]
    #[serde(default)]
    pub logging: LoggingConfig,

    #[arg(skip)]
    #[serde(default = "default_access_log_config")]
    pub access_log: LogConfig,

    /// Sensitive data masking for access logs. Disabled by default.
    ///
    /// YAML example (replace mode):
    ///   access_log_sensitive:
    ///     mode: replace
    ///     replace_with: "***"
    ///
    /// YAML example (wrap mode):
    ///   access_log_sensitive:
    ///     mode: wrap
    ///     wrap_prefix: "<E>"
    ///     wrap_suffix: "</E>"
    #[arg(skip)]
    #[serde(default, skip_serializing)]
    pub access_log_sensitive: edgion_resources::sensitive::SensitiveMaskConfig,

    /// Access log output options that don't fit into the shared `LogConfig`.
    ///
    /// YAML example:
    ///   access_log_options:
    ///     include_raw_uri: true
    #[arg(skip)]
    #[serde(default)]
    pub access_log_options: edgion_resources::observe::AccessLogOptions,

    #[arg(skip)]
    #[serde(default = "default_ssl_log_config")]
    pub ssl_log: LogConfig,

    #[arg(skip)]
    #[serde(default)]
    pub tcp_log: LogConfig,

    #[arg(skip)]
    #[serde(default = "default_tls_log_config")]
    pub tls_log: LogConfig,

    #[arg(skip)]
    #[serde(default)]
    pub udp_log: LogConfig,

    #[command(flatten)]
    #[serde(default)]
    pub server: ServerConfig,

    /// Metrics configuration
    #[arg(skip)]
    #[serde(default)]
    pub metrics: MetricsConfig,

    /// RateLimit plugin global configuration
    #[arg(skip)]
    #[serde(default, alias = "rate_limiter")]
    pub rate_limit: RateLimitGlobalConfig,

    /// Global LRU cache configuration for plugins
    #[arg(skip)]
    #[serde(default, alias = "auth_cache")]
    pub lru_cache: LruCacheGlobalConfig,

    /// AllEndpointStatus plugin global configuration
    #[arg(skip)]
    #[serde(default)]
    pub all_endpoint_status: crate::plugins::http::all_endpoint_status::plugin::AllEndpointStatusGlobalConfig,

    /// Enable integration testing mode.
    /// When enabled, the gateway activates test subsystems:
    /// - Access Log Store: stores complete access logs in DashMap, queryable via Admin API
    /// - Metrics Test Data: collects test_key/test_data labels on backend_requests_total
    ///   This flag MUST NEVER be used in production.
    #[arg(long = "integration-testing-mode", default_value = "false")]
    #[serde(skip)]
    pub integration_testing_mode: bool,

    /// conf_sync gRPC channel security (mTLS).
    /// Omit this section to keep plaintext gRPC (backward compatible).
    #[arg(skip)]
    #[serde(default)]
    pub conf_sync_security: edgion_common::conf_sync::config::ConfSyncSecurityConfig,

    /// URI path normalization configuration.
    ///
    /// Controls rejection behaviour for unusual but technically valid encodings.
    /// Both flags default to `false`; the BASE normalization algorithm always runs.
    ///
    /// YAML example:
    ///   path_normalization:
    ///     reject_escaped_slashes: true
    ///     reject_invalid_percent_encoding: false
    #[arg(skip)]
    #[serde(default)]
    pub path_normalization: edgion_resources::path_normalization::PathNormalizationConfig,
}

/// Gateway configuration
#[derive(Clone, Serialize, Deserialize, Args)]
pub struct GatewayConfig {
    /// Operator gRPC address (e.g., http://127.0.0.1:12151)
    #[arg(long = "server-addr", value_name = "ADDR")]
    #[serde(default)]
    pub server_addr: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long = "admin-listen", value_name = "ADDR")]
    #[serde(default)]
    pub admin_listen: Option<String>,

    /// Gateway probe HTTP listen address (liveness/readiness only)
    #[arg(long = "probe-listen", value_name = "ADDR")]
    #[serde(default)]
    pub probe_listen: Option<String>,

    /// Shared bearer token for the Gateway admin API. Config-file only (no CLI
    /// flag, so it never lands in process args / `ps`). In Kubernetes, prefer
    /// the `EDGION_GATEWAY_ADMIN_TOKEN` env var injected from a Secret; this
    /// field is the fallback for a Secret-mounted config file. Never serialized
    /// out, and redacted in `Debug` (see the manual impl below).
    #[arg(skip)]
    #[serde(default, skip_serializing)]
    pub admin_token: Option<String>,

    /// Require the FIPS-validated crypto module at startup. Default false.
    /// When true and the module is absent (a non-FIPS build), the gateway logs
    /// a clear error and exits non-zero. Also settable via `EDGION_REQUIRE_FIPS`
    /// (truthy: 1/true/yes/on). See the FIPS design spec.
    #[arg(long = "require-fips")]
    #[serde(default)]
    pub require_fips: bool,

    /// How to select the FIPS-approved TLS policy (approved cipher list +
    /// curves) on downstream listeners. `auto` (the default) follows the host
    /// kernel's FIPS mode; `on` always enforces; `off` never does. A real FIPS
    /// build enforces the policy unconditionally regardless of this setting.
    ///
    /// Accepts only `auto`/`on`/`off`. Also settable via `EDGION_FIPS_POLICY`
    /// (same three values), which takes precedence over this field. `None`
    /// means "not specified".
    /// See docs/superpowers/specs/2026-07-10-fips-host-detection-design.md.
    #[arg(
        long = "fips-policy",
        value_name = "MODE",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "on"
    )]
    #[serde(default)]
    pub fips_policy: Option<crate::tls::fips_profile::FipsPolicyMode>,

    /// Reject RFC1918 / unique-local addresses for outbound URL targets by default.
    /// A `target.blockPrivate` on the resource overrides this. Loopback and
    /// link-local are always rejected regardless. Default false, matching
    /// `classify_backend_addr`'s stance for Service targets.
    /// Set in the gateway config file, or via `EDGION_OUTBOUND_BLOCK_PRIVATE`
    /// (truthy: 1/true/yes/on).
    #[arg(skip)]
    #[serde(default)]
    pub outbound_egress_block_private: bool,

    /// TTL in milliseconds for `EgressResolver`'s per-client address memo.
    /// Deliberately distinct from `EdgionGatewayConfig.dnsResolver.cacheTtl`, which
    /// governs ExternalName self-resolution. Default 5s, matching
    /// `backends::discovery::dns_cache::DEFAULT_DNS_CACHE_TTL`.
    /// Set in the gateway config file, or via `EDGION_OUTBOUND_DNS_CACHE_TTL_MS`.
    #[arg(skip)]
    #[serde(default = "default_outbound_egress_dns_cache_ttl_ms")]
    pub outbound_egress_dns_cache_ttl_ms: u64,

    /// Directory for the Wasm OCI image disk cache (manifests + layer blobs,
    /// content-addressed). Relative paths resolve against the work directory.
    /// Unset uses `wasm-oci-cache`; an empty string disables disk caching.
    #[arg(skip)]
    #[serde(default)]
    pub wasm_oci_cache_dir: Option<String>,

    /// Seconds since last use before a cached OCI file is purged (default 24h).
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_ttl_secs")]
    pub wasm_oci_cache_ttl_secs: u64,

    /// Seconds between purge sweeps of the OCI disk cache (default 1h).
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_purge_interval_secs")]
    pub wasm_oci_cache_purge_interval_secs: u64,

    /// Total-size budget in bytes for the OCI disk cache (default 512MiB).
    /// Enforced on every write with LRU eviction; `0` removes the cap.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_max_bytes")]
    pub wasm_oci_cache_max_bytes: u64,

    /// Per-request timeout in seconds for OCI pull requests (manifest, token,
    /// and blob alike; istio-agent WASM_HTTP_REQUEST_TIMEOUT parity). Default 60.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_pull_timeout_secs")]
    pub wasm_oci_pull_timeout_secs: u64,

    /// Total attempts (including the first) for each OCI pull request;
    /// transport errors and 5xx retry, 4xx never does. Default 3.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_pull_attempts")]
    pub wasm_oci_pull_attempts: u32,
}

/// 5s — matches `backends::discovery::dns_cache::DEFAULT_DNS_CACHE_TTL`.
fn default_outbound_egress_dns_cache_ttl_ms() -> u64 {
    5_000
}

fn default_wasm_oci_cache_ttl_secs() -> u64 {
    86_400
}
fn default_wasm_oci_cache_purge_interval_secs() -> u64 {
    3_600
}
fn default_wasm_oci_cache_max_bytes() -> u64 {
    512 * 1024 * 1024
}
fn default_wasm_oci_pull_timeout_secs() -> u64 {
    60
}
fn default_wasm_oci_pull_attempts() -> u32 {
    3
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            server_addr: None,
            admin_listen: None,
            probe_listen: None,
            admin_token: None,
            require_fips: false,
            fips_policy: None,
            outbound_egress_block_private: false,
            outbound_egress_dns_cache_ttl_ms: default_outbound_egress_dns_cache_ttl_ms(),
            wasm_oci_cache_dir: None,
            wasm_oci_cache_ttl_secs: default_wasm_oci_cache_ttl_secs(),
            wasm_oci_cache_purge_interval_secs: default_wasm_oci_cache_purge_interval_secs(),
            wasm_oci_cache_max_bytes: default_wasm_oci_cache_max_bytes(),
            wasm_oci_pull_timeout_secs: default_wasm_oci_pull_timeout_secs(),
            wasm_oci_pull_attempts: default_wasm_oci_pull_attempts(),
        }
    }
}

impl std::fmt::Debug for GatewayConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayConfig")
            .field("server_addr", &self.server_addr)
            .field("admin_listen", &self.admin_listen)
            .field("probe_listen", &self.probe_listen)
            .field("admin_token", &self.admin_token.as_ref().map(|_| "[redacted]"))
            .field("require_fips", &self.require_fips)
            .field("fips_policy", &self.fips_policy)
            .field("outbound_egress_block_private", &self.outbound_egress_block_private)
            .field(
                "outbound_egress_dns_cache_ttl_ms",
                &self.outbound_egress_dns_cache_ttl_ms,
            )
            .field("wasm_oci_cache_dir", &self.wasm_oci_cache_dir)
            .field("wasm_oci_cache_ttl_secs", &self.wasm_oci_cache_ttl_secs)
            .field(
                "wasm_oci_cache_purge_interval_secs",
                &self.wasm_oci_cache_purge_interval_secs,
            )
            .field("wasm_oci_cache_max_bytes", &self.wasm_oci_cache_max_bytes)
            .field("wasm_oci_pull_timeout_secs", &self.wasm_oci_pull_timeout_secs)
            .field("wasm_oci_pull_attempts", &self.wasm_oci_pull_attempts)
            .finish()
    }
}

/// Environment variable that must be explicitly set (to a truthy value) for the
/// `--integration-testing-mode` flag to take effect. This is a deliberate second
/// guard: the mode activates production-unsafe behavior (test-only admin routes,
/// in-memory access-log storage, per-request metric labels, loopback upstreams),
/// so a single CLI flag must not be enough to enable it in a production rollout.
pub const ALLOW_INTEGRATION_TESTING_MODE_ENV: &str = "EDGION_ALLOW_INTEGRATION_TESTING_MODE";

/// Whether the integration-testing-mode env override is set to a truthy value.
/// Accepts `1`, `true`, `yes`, `on` (case-insensitive, trimmed). Anything else —
/// including unset/empty — is treated as not enabled, so the gateway fails closed.
pub fn integration_testing_mode_env_enabled(env_value: Option<&str>) -> bool {
    match env_value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

/// Env var that forces the FIPS-module startup assertion. Truthy: 1/true/yes/on.
pub const REQUIRE_FIPS_ENV: &str = "EDGION_REQUIRE_FIPS";

/// Whether `EDGION_REQUIRE_FIPS` is set to a truthy value (trimmed, lowercased).
/// Unset/empty/other => false.
pub fn require_fips_env_enabled(env_value: Option<&str>) -> bool {
    match env_value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

/// Env var that selects the FIPS-approved TLS policy. Accepts `auto`/`on`/`off`.
pub const FIPS_POLICY_ENV: &str = "EDGION_FIPS_POLICY";

/// The three outcomes of reading `EDGION_FIPS_POLICY`. `Invalid` is kept
/// distinct from `Unset` so the caller can WARN on a typo rather than silently
/// falling through — a misspelled value must never change policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Not set, or set to whitespace.
    Unset,
    /// Set to an unrecognized token, carried for the WARN message.
    Invalid(String),
    /// Set to a recognized mode.
    Set(crate::tls::fips_profile::FipsPolicyMode),
}

/// Parse `EDGION_FIPS_POLICY`. Unset or empty falls through to the config file.
pub fn fips_policy_env_mode(env_value: Option<&str>) -> EnvPolicy {
    let Some(raw) = env_value else {
        return EnvPolicy::Unset;
    };
    if raw.trim().is_empty() {
        return EnvPolicy::Unset;
    }
    match crate::tls::fips_profile::FipsPolicyMode::parse_token(raw) {
        Some(mode) => EnvPolicy::Set(mode),
        None => EnvPolicy::Invalid(raw.trim().to_string()),
    }
}

/// Env var that force-enables `block_private` for outbound URL targets.
/// Truthy: 1/true/yes/on.
pub const OUTBOUND_BLOCK_PRIVATE_ENV: &str = "EDGION_OUTBOUND_BLOCK_PRIVATE";

/// Whether `EDGION_OUTBOUND_BLOCK_PRIVATE` is set to a truthy value (trimmed,
/// lowercased). Unset/empty/other => false.
pub fn outbound_block_private_env_enabled(env_value: Option<&str>) -> bool {
    match env_value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

/// Env var overriding the outbound egress DNS cache TTL, in milliseconds.
pub const OUTBOUND_DNS_CACHE_TTL_MS_ENV: &str = "EDGION_OUTBOUND_DNS_CACHE_TTL_MS";

/// Resolve the outbound egress DNS cache TTL (ms): the env override takes
/// precedence over `config_value` when present and parses as `u64`. An
/// unparseable env value is not worth failing startup over — this logs a WARN
/// and falls back to `config_value` instead of propagating the parse error.
pub fn resolve_outbound_egress_dns_cache_ttl_ms(config_value: u64, env_value: Option<&str>) -> u64 {
    match env_value {
        Some(raw) => match raw.trim().parse::<u64>() {
            Ok(parsed) => parsed,
            Err(_) => {
                tracing::warn!(
                    target: "startup",
                    raw_value = raw,
                    "EDGION_OUTBOUND_DNS_CACHE_TTL_MS is not a valid u64; keeping configured value"
                );
                config_value
            }
        },
        None => config_value,
    }
}

/// Startup gate: when FIPS is required but the validated module is absent,
/// return an error string for the caller to log before exiting non-zero.
pub fn check_fips_requirement(require_fips: bool, module_available: bool) -> Result<(), String> {
    if require_fips && !module_available {
        return Err("FIPS required (require_fips / EDGION_REQUIRE_FIPS) but this binary \
             does not link the FIPS-validated crypto module; deploy a FIPS build \
             or unset the requirement"
            .to_string());
    }
    Ok(())
}

/// Resolve the gateway admin token from its sources, highest precedence first:
/// the `EDGION_GATEWAY_ADMIN_TOKEN` env var, then the config field. Values are
/// trimmed; empty/whitespace-only is treated as absent (so a present-but-empty
/// env var does not shadow a real config value). Returns `None` when no token is
/// configured (the caller then fails closed with 503).
pub fn resolve_admin_token(env_value: Option<String>, config_value: Option<&str>) -> Option<String> {
    let cleaned = |v: &str| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    };
    if let Some(env) = env_value.as_deref().and_then(cleaned) {
        return Some(env);
    }
    config_value.and_then(cleaned)
}

// Default log configurations
fn default_access_log_config() -> LogConfig {
    LogConfig::enabled_default("logs/edgion_access.log")
}

fn default_ssl_log_config() -> LogConfig {
    LogConfig::enabled_default("logs/ssl.log")
}

fn default_tls_log_config() -> LogConfig {
    LogConfig::enabled_default("logs/tls_access.log")
}

/// Pingora server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
pub struct ServerConfig {
    /// Number of worker threads (default: number of CPU cores)
    #[arg(long = "threads", value_name = "NUM")]
    #[serde(default)]
    pub threads: Option<usize>,

    /// Enable work stealing (default: true)
    #[arg(long = "work-stealing")]
    #[serde(default)]
    pub work_stealing: Option<bool>,

    /// Grace period for shutdown in seconds (default: 30)
    #[arg(long = "grace-period", value_name = "SECS")]
    #[serde(default)]
    pub grace_period_seconds: Option<u64>,

    /// Graceful shutdown timeout in seconds (default: 10)
    #[arg(long = "graceful-shutdown-timeout", value_name = "SECS")]
    #[serde(default)]
    pub graceful_shutdown_timeout_seconds: Option<u64>,

    /// Upstream keepalive pool size (default: 128)
    #[arg(long = "upstream-keepalive-pool-size", value_name = "SIZE")]
    #[serde(default)]
    pub upstream_keepalive_pool_size: Option<usize>,

    /// Error log file path (optional)
    #[arg(long = "error-log", value_name = "FILE")]
    #[serde(default)]
    pub error_log: Option<String>,

    /// Maximum requests per downstream connection before close (default: 1000, 0 = unlimited)
    #[arg(long = "downstream-keepalive-request-limit", value_name = "NUM")]
    #[serde(default)]
    pub downstream_keepalive_request_limit: Option<u32>,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct LoggingConfig {
    /// Log directory path
    #[arg(long = "log-dir", value_name = "DIR")]
    #[serde(default)]
    pub log_dir: Option<String>,

    #[arg(skip)]
    #[serde(default = "default_log_prefix")]
    pub log_prefix: String,

    /// Log level: trace, debug, info, warn, error
    #[arg(long = "log-level", value_name = "LEVEL")]
    #[serde(default)]
    pub log_level: Option<String>,

    /// Enable JSON log format
    #[arg(long = "json-format")]
    #[serde(default)]
    pub json_format: Option<bool>,

    /// Enable console output
    #[arg(skip)]
    #[serde(default = "default_console")]
    pub console: bool,

    #[arg(skip)]
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
}

// Default values
fn default_log_dir() -> String {
    "/usr/local/edgion/logs".to_string()
}

fn default_log_prefix() -> String {
    "edgion-gateway".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_console() -> bool {
    true
}

fn default_buffer_size() -> usize {
    10_000
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_dir: None,
            log_prefix: default_log_prefix(),
            log_level: None,
            json_format: None,
            console: default_console(),
            buffer_size: default_buffer_size(),
        }
    }
}

// ============================================
// Metrics Configuration
// ============================================

/// Metrics configuration section (`metrics` in YAML)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    /// Prometheus metrics listen address
    pub listen: String,
    /// Request duration histogram configuration
    pub request_duration: RequestDurationConfig,
    /// `edgion_backend_requests_total` per-label toggles
    pub backend_requests: BackendRequestsConfig,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:12090".to_string(),
            request_duration: RequestDurationConfig::default(),
            backend_requests: BackendRequestsConfig::default(),
        }
    }
}

/// Request duration histogram configuration (`metrics.request_duration` in YAML)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestDurationConfig {
    /// Enable the request duration histogram
    pub enabled: bool,
    /// Bucket boundaries in milliseconds
    pub buckets: Vec<f64>,
    /// Idle timeout in seconds for cold time-series cleanup (0 = disabled)
    pub idle_timeout_secs: u64,
    /// Per-label enable/disable toggles
    pub labels: RequestDurationLabels,
}

impl Default for RequestDurationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            buckets: vec![
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0,
            ],
            idle_timeout_secs: 600,
            labels: RequestDurationLabels::default(),
        }
    }
}

/// Per-label toggles for request duration histogram
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestDurationLabels {
    pub gateway_namespace: bool,
    pub gateway_name: bool,
    pub route_namespace: bool,
    pub route_name: bool,
    pub backend_namespace: bool,
    pub backend_name: bool,
    pub protocol: bool,
    pub status: bool,
}

impl Default for RequestDurationLabels {
    fn default() -> Self {
        Self {
            gateway_namespace: false,
            gateway_name: false,
            route_namespace: false,
            route_name: true,
            backend_namespace: false,
            backend_name: true,
            protocol: true,
            status: true,
        }
    }
}

// ============================================
// Global RequestDurationConfig Store
// ============================================

use std::sync::OnceLock;

/// Global request duration config, initialized once at startup.
static REQUEST_DURATION_CONFIG: OnceLock<RequestDurationConfig> = OnceLock::new();

/// Initialize the global request duration config (call once at startup).
pub fn init_request_duration_config(config: RequestDurationConfig) {
    let result = REQUEST_DURATION_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_request_duration_config called more than once");
}

/// Get the global request duration config.
///
/// Falls back to `RequestDurationConfig::default()` (histogram disabled) if
/// `init_request_duration_config` has not been called yet. This matches the
/// `get_or_init` pattern used by `get_backend_requests_config` and allows
/// route-unit construction in test environments that skip gateway startup.
#[inline]
pub fn get_request_duration_config() -> &'static RequestDurationConfig {
    REQUEST_DURATION_CONFIG.get_or_init(RequestDurationConfig::default)
}

// ============================================
// Backend Requests Counter Configuration
// ============================================

/// `edgion_backend_requests_total` configuration (`metrics.backend_requests` in YAML).
///
/// The counter itself is always on; this struct controls **which labels** get
/// emitted, so operators can trade observability depth for Prometheus
/// cardinality. Splitting each resource into `_namespace` + `_name` would be
/// redundant — you almost always want both or neither — so the toggles group
/// them by Kubernetes identity (gateway / route / backend).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendRequestsConfig {
    /// Per-label toggles.
    pub labels: BackendRequestsLabels,
}

/// Per-label toggles for `edgion_backend_requests_total`.
///
/// Each `gateway` / `route` / `backend` toggle controls **both** the
/// `*_namespace` and `*_name` labels for that resource. Default picks the
/// minimal set that still supports basic SLI / alerting: `gateway` identity
/// (so multi-gateway deployments can be told apart) + `protocol` + `status`
/// (bounded enums, effectively free). For per-route or per-backend drill-down
/// (typical dashboard layer), prefer access log aggregation — see
/// `skills/02-features/04-observability/01-metrics.md` for the metric-vs-log
/// division of responsibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendRequestsLabels {
    /// Emit `gateway_namespace` and `gateway_name`.
    pub gateway: bool,
    /// Emit `route_namespace` and `route_name`.
    pub route: bool,
    /// Emit `backend_namespace` and `backend_name`.
    pub backend: bool,
    /// Emit `protocol` (bounded enum: http / grpc / websocket).
    pub protocol: bool,
    /// Emit `status` (bounded enum: 2xx / 3xx / 4xx / 5xx / failed).
    pub status: bool,
}

impl Default for BackendRequestsLabels {
    fn default() -> Self {
        Self {
            gateway: true,
            route: false,
            backend: false,
            protocol: true,
            status: true,
        }
    }
}

/// Global backend-requests config, initialized once at startup.
static BACKEND_REQUESTS_CONFIG: OnceLock<BackendRequestsConfig> = OnceLock::new();

/// Initialize the global backend-requests config (call once at startup).
pub fn init_backend_requests_config(config: BackendRequestsConfig) {
    let result = BACKEND_REQUESTS_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_backend_requests_config called more than once");
}

/// Get the global backend-requests config (zero-cost after init).
///
/// Returns the default config if `init_backend_requests_config` was not
/// called (e.g., in unit tests that don't boot the full Gateway). Production
/// paths always initialize before first request.
#[inline]
pub fn get_backend_requests_config() -> &'static BackendRequestsConfig {
    BACKEND_REQUESTS_CONFIG.get_or_init(BackendRequestsConfig::default)
}

// ============================================
// Access Log Options Global Configuration
// ============================================

/// Global access log options, initialized once at startup.
static ACCESS_LOG_OPTIONS: OnceLock<edgion_resources::observe::AccessLogOptions> = OnceLock::new();

/// Initialize the global access log options (call once at startup).
pub fn init_access_log_options(options: edgion_resources::observe::AccessLogOptions) {
    let result = ACCESS_LOG_OPTIONS.set(options);
    debug_assert!(result.is_ok(), "init_access_log_options called more than once");
}

/// Get the global access log options (zero-cost after init).
///
/// Returns the default (all flags off) if `init_access_log_options` was not
/// called, e.g. unit tests that don't boot the full Gateway.
#[inline]
pub fn get_access_log_options() -> &'static edgion_resources::observe::AccessLogOptions {
    ACCESS_LOG_OPTIONS.get_or_init(edgion_resources::observe::AccessLogOptions::default)
}

// ============================================
// LRU Cache Global Configuration
// ============================================

const DEFAULT_LRU_CACHE_CAPACITY: usize = 1024;
const DEFAULT_LRU_CACHE_SHARDS: usize = 0;

fn default_lru_cache_capacity() -> usize {
    DEFAULT_LRU_CACHE_CAPACITY
}

/// Global LRU cache configuration for plugins.
/// Per-plugin `capacity = 0` falls back to this value.
///
/// ## YAML Configuration Example:
/// ```yaml
/// lru_cache:
///   default_capacity: 1024
///   default_shards: 0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LruCacheGlobalConfig {
    /// Default LRU capacity for all plugins (default: 1024 entries).
    #[serde(default = "default_lru_cache_capacity")]
    pub default_capacity: usize,

    /// Default number of cache shards for all plugins (default: 0 = disabled).
    /// When > 1, the cache is split into N independent LRU shards to reduce
    /// lock contention under high concurrency. Each shard holds
    /// `ceil(capacity / shards)` entries.
    #[serde(default)]
    pub default_shards: usize,
}

impl Default for LruCacheGlobalConfig {
    fn default() -> Self {
        Self {
            default_capacity: DEFAULT_LRU_CACHE_CAPACITY,
            default_shards: DEFAULT_LRU_CACHE_SHARDS,
        }
    }
}

static LRU_CACHE_GLOBAL_CONFIG: OnceLock<LruCacheGlobalConfig> = OnceLock::new();

/// Initialize the global LRU cache configuration (call once at startup).
pub fn init_lru_cache_global_config(config: &LruCacheGlobalConfig) {
    let result = LRU_CACHE_GLOBAL_CONFIG.set(config.clone());
    debug_assert!(result.is_ok(), "init_lru_cache_global_config called more than once");
}

/// Get the default LRU cache capacity.
/// Returns the configured value, or compile-time fallback 1024 if not yet initialized.
/// Never returns 0 — safe to pass directly to `ShardedLruCache::new()`.
pub fn get_default_lru_cache_capacity() -> usize {
    LRU_CACHE_GLOBAL_CONFIG
        .get()
        .map(|c| c.default_capacity)
        .unwrap_or(DEFAULT_LRU_CACHE_CAPACITY)
}

/// Get the default LRU cache shard count.
/// Returns 0 (disabled) if not configured.
pub fn get_default_lru_cache_shards() -> usize {
    LRU_CACHE_GLOBAL_CONFIG
        .get()
        .map(|c| c.default_shards)
        .unwrap_or(DEFAULT_LRU_CACHE_SHARDS)
}

// ============================================
impl EdgionGatewayConfig {
    /// Load configuration from YAML file if config_file is specified,
    /// then merge with CLI arguments (CLI takes precedence)
    pub fn load(cli_config: Self) -> Result<Self> {
        // Load from file if specified
        let mut file_config = if let Some(ref config_path) = cli_config.config_file {
            let content =
                std::fs::read_to_string(config_path).context(format!("Failed to read config file: {}", config_path))?;

            {
                let de = serde_yaml::Deserializer::from_str(&content);
                serde_yaml::with::singleton_map_recursive::deserialize(de)
                    .context(format!("Failed to parse config file: {}", config_path))?
            }
        } else {
            EdgionGatewayConfig::default()
        };

        // Merge: CLI args override file config
        Self::merge(&mut file_config, &cli_config);

        Ok(file_config)
    }

    /// Merge CLI config into file config (CLI takes precedence)
    pub(crate) fn merge(base: &mut Self, cli: &Self) {
        // Work directory: CLI value takes precedence if provided
        if cli.work_dir.is_some() {
            base.work_dir = cli.work_dir.clone();
        }

        // Gateway config
        if cli.gateway.server_addr.is_some() {
            base.gateway.server_addr = cli.gateway.server_addr.clone();
        }
        if cli.gateway.admin_listen.is_some() {
            base.gateway.admin_listen = cli.gateway.admin_listen.clone();
        }
        if cli.gateway.probe_listen.is_some() {
            base.gateway.probe_listen = cli.gateway.probe_listen.clone();
        }
        // FIPS: `fips_policy` is Option, so `Some` means the operator actually
        // passed --fips-policy; None must leave the file's value alone.
        if cli.gateway.fips_policy.is_some() {
            base.gateway.fips_policy = cli.gateway.fips_policy;
        }
        // `require_fips` keeps its OR semantics: the CLI flag can only add the
        // requirement, never remove one set in the file.
        if cli.gateway.require_fips {
            base.gateway.require_fips = true;
        }

        // Logging config
        if cli.logging.log_dir.is_some() {
            base.logging.log_dir = cli.logging.log_dir.clone();
        }
        if cli.logging.log_level.is_some() {
            base.logging.log_level = cli.logging.log_level.clone();
        }
        if cli.logging.json_format.is_some() {
            base.logging.json_format = cli.logging.json_format;
        }
    }

    /// Get server_addr
    pub fn server_addr(&self) -> Option<String> {
        self.gateway.server_addr.clone()
    }

    /// Get admin_listen. Test-only: production always calls `admin_listen_or_default`.
    #[cfg(test)]
    pub fn admin_listen(&self) -> Option<String> {
        self.gateway.admin_listen.clone()
    }

    /// Admin API listen address, falling back to the default when unset.
    pub fn admin_listen_or_default(&self) -> String {
        self.gateway
            .admin_listen
            .clone()
            .unwrap_or_else(|| DEFAULT_GATEWAY_ADMIN_LISTEN.to_string())
    }

    /// Get probe_listen. Test-only: production always calls `probe_listen_or_default`.
    #[cfg(test)]
    pub fn probe_listen(&self) -> Option<String> {
        self.gateway.probe_listen.clone()
    }

    /// Probe API listen address, falling back to the default when unset.
    pub fn probe_listen_or_default(&self) -> String {
        self.gateway
            .probe_listen
            .clone()
            .unwrap_or_else(|| DEFAULT_GATEWAY_PROBE_LISTEN.to_string())
    }

    /// Get log_dir with default fallback
    pub fn log_dir(&self) -> String {
        self.logging.log_dir.clone().unwrap_or_else(default_log_dir)
    }

    /// Get log_level with default fallback
    pub fn log_level(&self) -> String {
        self.logging.log_level.clone().unwrap_or_else(default_log_level)
    }

    /// Get json_format with default fallback
    pub fn json_format(&self) -> bool {
        self.logging.json_format.unwrap_or(false)
    }

    /// Convert to SysLogConfig (for system logging)
    pub fn to_log_config(&self) -> crate::observe::SysLogConfig {
        use crate::observe::SysLogConfig;
        use edgion_resources::work_dir;

        // Use work_dir to resolve the log directory path
        // This ensures relative paths like "logs" are resolved to work_dir/logs
        let log_dir = work_dir().resolve(self.log_dir());

        SysLogConfig {
            log_dir,
            file_prefix: self.logging.log_prefix.clone(),
            json_format: self.json_format(),
            console: self.logging.console,
            level: self.log_level(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::rate_limit_config::{
        get_gateway_instance_count, set_gateway_instance_count, DEFAULT_ESTIMATOR_SLOTS_K,
        DEFAULT_MAX_ESTIMATOR_SLOTS_K,
    };

    #[test]
    fn test_gateway_instance_count_default() {
        // Default should be at least 1
        assert!(get_gateway_instance_count() >= 1);
    }

    #[test]
    fn test_gateway_instance_count_set_and_get() {
        set_gateway_instance_count(5);
        assert_eq!(get_gateway_instance_count(), 5);
        set_gateway_instance_count(1); // cleanup
    }

    #[test]
    fn test_gateway_instance_count_never_zero() {
        set_gateway_instance_count(0); // Try to set zero
        assert_eq!(get_gateway_instance_count(), 1); // Should be clamped to 1
    }

    #[test]
    fn test_lru_cache_global_config_default() {
        let config = LruCacheGlobalConfig::default();
        assert_eq!(config.default_capacity, 1024);
    }

    #[test]
    fn test_get_default_lru_cache_capacity_fallback() {
        // Before init (or after init with default), capacity must be >= 1
        // so ShardedLruCache::new() never panics
        let cap = get_default_lru_cache_capacity();
        assert!(cap >= 1, "capacity must be > 0");
    }

    #[test]
    fn test_rate_limit_global_config_default() {
        let config = RateLimitGlobalConfig::default();
        assert_eq!(config.default_estimator_slots_k, DEFAULT_ESTIMATOR_SLOTS_K);
        assert_eq!(config.max_estimator_slots_k, DEFAULT_MAX_ESTIMATOR_SLOTS_K);
        assert_eq!(config.gateway_instance_count, 1);
    }

    #[test]
    fn test_admin_listen_defaults_to_loopback() {
        // Security default: when admin_listen is unset, the admin API must bind
        // loopback so it is not exposed cluster-wide. Reverting this to 0.0.0.0
        // would reopen the unauthenticated admin-surface exposure (harden Task 2).
        let cfg = EdgionGatewayConfig::default();
        assert_eq!(cfg.admin_listen(), None);
        assert_eq!(cfg.admin_listen_or_default(), "127.0.0.1:12001");
    }

    #[test]
    fn test_admin_listen_explicit_value_is_respected() {
        // Operators opting into cluster exposure (e.g. pod-IP kubelet probes)
        // set admin_listen explicitly; the default must not override it.
        let mut cfg = EdgionGatewayConfig::default();
        cfg.gateway.admin_listen = Some("0.0.0.0:5810".to_string());
        assert_eq!(cfg.admin_listen_or_default(), "0.0.0.0:5810");
    }

    #[test]
    fn test_probe_listen_defaults_to_outward() {
        // The probe port carries only non-sensitive liveness/readiness signals and
        // binds 0.0.0.0 by default so a kubelet httpGet can reach it over the pod IP.
        let cfg = EdgionGatewayConfig::default();
        assert_eq!(cfg.probe_listen(), None);
        assert_eq!(cfg.probe_listen_or_default(), "0.0.0.0:12000");
    }

    #[test]
    fn test_probe_listen_explicit_value_is_respected() {
        let mut cfg = EdgionGatewayConfig::default();
        cfg.gateway.probe_listen = Some("127.0.0.1:28000".to_string());
        assert_eq!(cfg.probe_listen_or_default(), "127.0.0.1:28000");
    }

    #[test]
    fn test_probe_listen_cli_survives_config_file_merge() {
        // Guards the merge() wiring: without the merge branch, a CLI --probe-listen
        // is silently dropped whenever a config file is loaded.
        let mut base = EdgionGatewayConfig::default();
        let mut cli = EdgionGatewayConfig::default();
        cli.gateway.probe_listen = Some("0.0.0.0:39999".to_string());
        EdgionGatewayConfig::merge(&mut base, &cli);
        assert_eq!(base.gateway.probe_listen.as_deref(), Some("0.0.0.0:39999"));
    }

    #[test]
    fn test_access_log_sensitive_default_is_off() {
        let cfg: EdgionGatewayConfig = serde_yaml::from_str("{}").unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            edgion_resources::sensitive::SensitiveMode::Off
        ));
    }

    #[test]
    fn test_access_log_sensitive_replace_mode() {
        let yaml_str = r#"
access_log_sensitive:
  mode: replace
  replace_with: "{MSKED}"
"#;
        let cfg: EdgionGatewayConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            edgion_resources::sensitive::SensitiveMode::Replace
        ));
        assert_eq!(cfg.access_log_sensitive.replace_with, "{MSKED}");
    }

    #[test]
    fn test_access_log_sensitive_wrap_mode() {
        let yaml_str = r#"
access_log_sensitive:
  mode: wrap
  wrap_prefix: "<E>"
  wrap_suffix: "</E>"
"#;
        let cfg: EdgionGatewayConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            edgion_resources::sensitive::SensitiveMode::Wrap
        ));
        assert_eq!(cfg.access_log_sensitive.wrap_prefix, "<E>");
    }

    #[test]
    fn test_metrics_listen_defaults_to_management_block() {
        // The metrics listener default must use the Gateway management port block
        // (120xx). Changing this would silently break the port-unification plan.
        let cfg = MetricsConfig::default();
        assert_eq!(cfg.listen, "0.0.0.0:12090");
    }

    #[test]
    fn integration_testing_mode_env_accepts_truthy_values() {
        for v in ["1", "true", "TRUE", "Yes", "on", "  on  "] {
            assert!(
                integration_testing_mode_env_enabled(Some(v)),
                "expected {v:?} to enable integration testing mode"
            );
        }
    }

    #[test]
    fn integration_testing_mode_env_rejects_falsey_and_unset() {
        for v in ["0", "false", "no", "off", "", "   ", "enable", "2"] {
            assert!(
                !integration_testing_mode_env_enabled(Some(v)),
                "expected {v:?} NOT to enable integration testing mode"
            );
        }
        assert!(
            !integration_testing_mode_env_enabled(None),
            "unset env must not enable integration testing mode"
        );
    }

    #[test]
    fn resolve_admin_token_prefers_nonempty_env() {
        assert_eq!(
            resolve_admin_token(Some("env-token".into()), Some("config-token")),
            Some("env-token".to_string())
        );
    }

    #[test]
    fn resolve_admin_token_empty_env_falls_through_to_config() {
        // present-but-empty env must not shadow a real config value
        assert_eq!(
            resolve_admin_token(Some("   ".into()), Some("config-token")),
            Some("config-token".to_string())
        );
        assert_eq!(
            resolve_admin_token(Some("".into()), Some("config-token")),
            Some("config-token".to_string())
        );
    }

    #[test]
    fn resolve_admin_token_trims_and_treats_empty_as_none() {
        assert_eq!(resolve_admin_token(None, None), None);
        assert_eq!(resolve_admin_token(None, Some("  ")), None);
        assert_eq!(
            resolve_admin_token(Some("  spaced  ".into()), None),
            Some("spaced".to_string())
        );
    }

    #[test]
    fn admin_token_is_not_serialized() {
        let cfg = EdgionGatewayConfig {
            gateway: GatewayConfig {
                admin_token: Some("must-not-leak".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let serialized = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !serialized.contains("must-not-leak"),
            "admin_token must not serialize out"
        );
    }

    #[test]
    fn admin_token_is_redacted_in_debug() {
        let cfg = GatewayConfig {
            admin_token: Some("must-not-leak".to_string()),
            ..Default::default()
        };
        let dbg = format!("{:?}", cfg);
        assert!(!dbg.contains("must-not-leak"), "admin_token must be redacted in Debug");
        assert!(dbg.contains("[redacted]"));
    }

    #[test]
    fn require_fips_env_truthy_values() {
        assert!(require_fips_env_enabled(Some("1")));
        assert!(require_fips_env_enabled(Some(" TRUE ")));
        assert!(require_fips_env_enabled(Some("yes")));
        assert!(!require_fips_env_enabled(Some("0")));
        assert!(!require_fips_env_enabled(Some("")));
        assert!(!require_fips_env_enabled(None));
    }

    #[test]
    fn outbound_egress_defaults_are_permissive_and_short_lived() {
        let cfg = GatewayConfig::default();
        assert!(
            !cfg.outbound_egress_block_private,
            "private ranges must be allowed by default, matching classify_backend_addr"
        );
        assert_eq!(cfg.outbound_egress_dns_cache_ttl_ms, 5_000);
    }

    #[test]
    fn outbound_egress_parses_from_yaml() {
        let cfg: GatewayConfig =
            serde_yaml::from_str("outbound_egress_block_private: true\noutbound_egress_dns_cache_ttl_ms: 30000\n")
                .unwrap();
        assert!(cfg.outbound_egress_block_private);
        assert_eq!(cfg.outbound_egress_dns_cache_ttl_ms, 30_000);
    }

    #[test]
    fn wasm_oci_cache_fields_parse_and_default() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.wasm_oci_cache_dir, None);
        assert_eq!(cfg.wasm_oci_cache_ttl_secs, 86_400);
        assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 3_600);
        assert_eq!(cfg.wasm_oci_cache_max_bytes, 512 * 1024 * 1024);
        let cfg: GatewayConfig = serde_yaml::from_str(
            "wasm_oci_cache_dir: \"\"\nwasm_oci_cache_ttl_secs: 60\n\
             wasm_oci_cache_purge_interval_secs: 30\nwasm_oci_cache_max_bytes: 0\n",
        )
        .unwrap();
        assert_eq!(cfg.wasm_oci_cache_dir.as_deref(), Some(""));
        assert_eq!(cfg.wasm_oci_cache_ttl_secs, 60);
        assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 30);
        assert_eq!(cfg.wasm_oci_cache_max_bytes, 0);
        // Pull tuning defaults + YAML override.
        assert_eq!(GatewayConfig::default().wasm_oci_pull_timeout_secs, 60);
        assert_eq!(GatewayConfig::default().wasm_oci_pull_attempts, 3);
        let cfg: GatewayConfig =
            serde_yaml::from_str("wasm_oci_pull_timeout_secs: 15\nwasm_oci_pull_attempts: 5\n").unwrap();
        assert_eq!(cfg.wasm_oci_pull_timeout_secs, 15);
        assert_eq!(cfg.wasm_oci_pull_attempts, 5);
    }

    #[test]
    fn outbound_egress_fields_come_from_config_file_not_cli() {
        // Both fields are `#[arg(skip)]`: clap never populates them, and
        // `merge()` does not copy them from the CLI-parsed struct into the
        // file-loaded base config. Their only source is the config file (or
        // the corresponding env var, applied separately in cli/mod.rs). This
        // guards against a regression that re-adds a clap flag for these
        // fields without also wiring `merge()` to honor it.
        let yaml = "gateway:\n  outbound_egress_block_private: true\n  outbound_egress_dns_cache_ttl_ms: 30000\n";
        let mut base: EdgionGatewayConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(base.gateway.outbound_egress_block_private);
        assert_eq!(base.gateway.outbound_egress_dns_cache_ttl_ms, 30_000);

        // Simulate a CLI-parsed struct carrying different values (as would
        // happen if a clap flag were mistakenly reintroduced and passed).
        let mut cli = EdgionGatewayConfig::default();
        cli.gateway.outbound_egress_block_private = false;
        cli.gateway.outbound_egress_dns_cache_ttl_ms = 5_000;

        EdgionGatewayConfig::merge(&mut base, &cli);

        // merge() must leave these fields alone: the config-file values survive.
        assert!(base.gateway.outbound_egress_block_private);
        assert_eq!(base.gateway.outbound_egress_dns_cache_ttl_ms, 30_000);

        // And GatewayConfig::default() still gives the documented defaults.
        assert!(!GatewayConfig::default().outbound_egress_block_private);
        assert_eq!(GatewayConfig::default().outbound_egress_dns_cache_ttl_ms, 5_000);
    }

    #[test]
    fn outbound_block_private_env_truthy_values() {
        assert!(outbound_block_private_env_enabled(Some("1")));
        assert!(outbound_block_private_env_enabled(Some(" TRUE ")));
        assert!(outbound_block_private_env_enabled(Some("yes")));
        assert!(outbound_block_private_env_enabled(Some("On")));
        assert!(!outbound_block_private_env_enabled(Some("0")));
        assert!(!outbound_block_private_env_enabled(Some("")));
        assert!(!outbound_block_private_env_enabled(None));
    }

    #[test]
    fn outbound_egress_dns_cache_ttl_ms_env_overrides_when_valid() {
        assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some("30000")), 30_000);
        assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some(" 12 ")), 12);
    }

    #[test]
    fn outbound_egress_dns_cache_ttl_ms_falls_back_on_unparseable_env() {
        // An unparseable TTL must not panic or fail startup; the configured
        // value is kept and a WARN is logged.
        assert_eq!(
            resolve_outbound_egress_dns_cache_ttl_ms(5_000, Some("not-a-number")),
            5_000
        );
        assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(7_500, Some("")), 7_500);
        assert_eq!(resolve_outbound_egress_dns_cache_ttl_ms(9_000, None), 9_000);
    }

    #[test]
    fn check_fips_requirement_ok_when_not_required() {
        assert!(check_fips_requirement(false, false).is_ok());
        assert!(check_fips_requirement(false, true).is_ok());
    }

    #[test]
    fn check_fips_requirement_ok_when_required_and_present() {
        assert!(check_fips_requirement(true, true).is_ok());
    }

    #[test]
    fn check_fips_requirement_err_when_required_but_missing() {
        let err = check_fips_requirement(true, false).unwrap_err();
        assert!(err.contains("FIPS"));
    }

    #[test]
    fn test_access_log_otlp_yaml_deserializes_via_singleton_map() {
        // Verify the full production deserialization path (singleton_map_recursive)
        // parses the `otlp:` YAML key into StringOutput::Otlp. Without this test,
        // a casing mismatch silently falls back to StringOutput::LocalFile and the
        // OTLP arm in logger_factory.rs is never reached.
        use edgion_resources::{OtlpProtocol, StringOutput};

        let yaml = r#"
access_log:
  enabled: true
  output:
    otlp:
      endpoint: "http://127.0.0.1:14317"
      protocol: grpc
      timeoutMs: 5000
"#;
        let de = serde_yaml::Deserializer::from_str(yaml);
        let cfg: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
        assert!(cfg.access_log.enabled);
        match cfg.access_log.output {
            StringOutput::Otlp(o) => {
                assert_eq!(o.endpoint, "http://127.0.0.1:14317");
                assert!(matches!(o.protocol, OtlpProtocol::Grpc));
                assert_eq!(o.timeout_ms, Some(5000));
            }
            other => panic!("expected StringOutput::Otlp, got {:?}", other),
        }
    }

    #[test]
    fn test_access_log_otlp_grpc_is_default_protocol() {
        // `protocol` field defaults to grpc when omitted.
        use edgion_resources::{OtlpProtocol, StringOutput};

        let yaml = r#"
access_log:
  enabled: true
  output:
    otlp:
      endpoint: "http://otel-collector:4317"
"#;
        let de = serde_yaml::Deserializer::from_str(yaml);
        let cfg: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
        match cfg.access_log.output {
            StringOutput::Otlp(o) => {
                assert!(matches!(o.protocol, OtlpProtocol::Grpc));
                assert!(o.timeout_ms.is_none());
                assert!(o.headers.is_none());
            }
            other => panic!("expected StringOutput::Otlp, got {:?}", other),
        }
    }

    #[test]
    fn fips_policy_env_mode_parses_tristate() {
        use crate::tls::fips_profile::FipsPolicyMode;

        assert!(matches!(
            fips_policy_env_mode(Some("auto")),
            EnvPolicy::Set(FipsPolicyMode::Auto)
        ));
        assert!(matches!(
            fips_policy_env_mode(Some(" On ")),
            EnvPolicy::Set(FipsPolicyMode::On)
        ));
        assert!(matches!(
            fips_policy_env_mode(Some("off")),
            EnvPolicy::Set(FipsPolicyMode::Off)
        ));
        // No compatibility layer: the legacy truthy/falsey vocabulary of the old
        // boolean flag is no longer recognized.
        assert!(matches!(fips_policy_env_mode(Some("1")), EnvPolicy::Invalid(_)));
        assert!(matches!(fips_policy_env_mode(Some("false")), EnvPolicy::Invalid(_)));
    }

    #[test]
    fn fips_policy_env_mode_unset_and_empty_fall_through() {
        assert!(matches!(fips_policy_env_mode(None), EnvPolicy::Unset));
        assert!(matches!(fips_policy_env_mode(Some("")), EnvPolicy::Unset));
        assert!(matches!(fips_policy_env_mode(Some("   ")), EnvPolicy::Unset));
    }

    #[test]
    fn fips_policy_env_mode_reports_garbage_instead_of_guessing() {
        // A typo must not silently change policy in either direction; the caller
        // WARNs and falls through to the config file.
        match fips_policy_env_mode(Some("enabled")) {
            EnvPolicy::Invalid(raw) => assert_eq!(raw, "enabled"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn merge_propagates_fips_policy_from_cli_and_preserves_file_when_omitted() {
        use crate::tls::fips_profile::FipsPolicyMode;

        // Regression test for the pre-existing bug: --fips-policy was parsed and
        // then discarded by merge(), so the flag did nothing.
        let mut file = EdgionGatewayConfig::default();
        file.gateway.fips_policy = Some(FipsPolicyMode::On);

        let mut cli = EdgionGatewayConfig::default();
        cli.gateway.fips_policy = Some(FipsPolicyMode::Off);
        EdgionGatewayConfig::merge(&mut file, &cli);
        assert_eq!(
            file.gateway.fips_policy,
            Some(FipsPolicyMode::Off),
            "CLI must override the file"
        );

        // Omitted on the CLI (None) must NOT clobber the file's value. This is
        // why the field is Option: a bare enum with a clap default_value would
        // make "omitted" indistinguishable from "--fips-policy=auto".
        let mut file = EdgionGatewayConfig::default();
        file.gateway.fips_policy = Some(FipsPolicyMode::On);
        let cli = EdgionGatewayConfig::default();
        EdgionGatewayConfig::merge(&mut file, &cli);
        assert_eq!(
            file.gateway.fips_policy,
            Some(FipsPolicyMode::On),
            "omitted CLI must preserve the file"
        );
    }

    #[test]
    fn merge_propagates_require_fips_with_or_semantics() {
        let mut file = EdgionGatewayConfig::default();
        let mut cli = EdgionGatewayConfig::default();
        cli.gateway.require_fips = true;
        EdgionGatewayConfig::merge(&mut file, &cli);
        assert!(
            file.gateway.require_fips,
            "CLI --require-fips must reach the merged config"
        );

        // OR semantics: a file `true` survives an absent CLI flag.
        let mut file = EdgionGatewayConfig::default();
        file.gateway.require_fips = true;
        let cli = EdgionGatewayConfig::default();
        EdgionGatewayConfig::merge(&mut file, &cli);
        assert!(file.gateway.require_fips);
    }

    #[test]
    fn gateway_config_deserializes_fips_policy() {
        use crate::tls::fips_profile::FipsPolicyMode;

        let yaml = "fips_policy: auto\n";
        let parsed: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.fips_policy, Some(FipsPolicyMode::Auto));

        // Bare `on` resolves to a YAML boolean, so it must be quoted to reach
        // the string parser.
        let yaml = "fips_policy: \"on\"\n";
        let parsed: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.fips_policy, Some(FipsPolicyMode::On));

        // Omitted => None => the startup path applies Auto.
        let parsed: GatewayConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(parsed.fips_policy, None);

        // No compatibility layer: a YAML boolean is rejected outright.
        let yaml = "fips_policy: true\n";
        assert!(serde_yaml::from_str::<GatewayConfig>(yaml).is_err());
    }
}
