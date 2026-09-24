use crate::plugins::http::request_mirror::RequestMirrorGlobalConfig;
use anyhow::{Context, Result};
use clap::Args;
use edgion_resources::rate_limit_local_config::RateLimitLocalGlobalConfig;
use edgion_resources::resources::common::parse_byte_size;
use edgion_resources::resources::edgion_plugins::{
    validate_tiny_ufo_effective_limits, LruCacheConfig, TinyUfoTtlCacheConfig, MAX_LRU_CACHE_CAPACITY,
    MAX_LRU_CACHE_SHARDS,
};
use edgion_resources::LogConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(crate) const MAX_UNKNOWN_CONFIG_FIELDS: usize = 32;

/// Unknown Gateway process-config fields collected during YAML deserialization.
///
/// The paths are bounded because configuration is operator-controlled but still
/// reaches the startup log. Values and raw YAML are never retained or logged.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct UnknownConfigFields {
    paths: BTreeSet<String>,
    truncated: bool,
}

impl UnknownConfigFields {
    fn record(&mut self, path: &str) {
        let path = normalize_unknown_field_path(path);
        if self.paths.contains(&path) {
            return;
        }
        if self.paths.len() < MAX_UNKNOWN_CONFIG_FIELDS {
            self.paths.insert(path);
        } else {
            self.truncated = true;
        }
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths.iter().map(String::as_str)
    }

    pub(crate) fn is_truncated(&self) -> bool {
        self.truncated
    }
}

fn normalize_unknown_field_path(path: &str) -> String {
    let mut normalized = String::new();
    for segment in path.split('.').filter(|segment| !segment.is_empty() && *segment != "?") {
        if segment.bytes().all(|byte| byte.is_ascii_digit()) {
            normalized.push('[');
            normalized.push_str(segment);
            normalized.push(']');
        } else {
            if !normalized.is_empty() {
                normalized.push('.');
            }
            normalized.push_str(segment);
        }
    }
    normalized
}

/// Result of loading local Gateway process configuration.
pub(crate) struct LoadedGatewayConfig {
    pub(crate) config: EdgionGatewayConfig,
    pub(crate) unknown_fields: UnknownConfigFields,
}

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

    /// Sensitive data masking for access logs. Defaults to `replace`
    /// (privacy-by-default); set `mode: off` to opt out.
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
    ///     includeRawUri: true
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

    /// RateLimitLocal plugin global configuration
    #[arg(skip)]
    #[serde(default)]
    pub rate_limit_local: RateLimitLocalGlobalConfig,

    /// Startup-only RequestMirror task and retained request-body limits.
    #[arg(skip)]
    #[serde(default)]
    pub request_mirror: RequestMirrorGlobalConfig,

    /// Global configuration for the retained LRU implementation
    #[arg(skip)]
    #[serde(default, alias = "auth_cache")]
    pub lru_cache: LruCacheGlobalConfig,

    /// Global bounded TinyUFO TTL-cache configuration
    #[arg(skip)]
    #[serde(default)]
    pub tiny_ufo_ttl_cache: TinyUfoTtlCacheGlobalConfig,

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
    ///
    /// REQUIRED: this section carries the transport choice for a channel that delivers
    /// resolved Secrets to this data plane, so it must be made explicitly. Omitting it
    /// makes the Gateway REFUSE TO START (see
    /// `ConfSyncSecurityConfig::ensure_transport_configured`). Configure exactly one of:
    /// `auto_ca: true` (Kubernetes), `certs`/legacy `tls` cert paths (file-mode mTLS), or
    /// `tls.skip_tls: true` — the explicit opt-in for plaintext. Must match the Controller.
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
///
/// `Debug` is derived: `admin_token` is a `RedactedString` (see
/// `crate::util::redacted`), whose own `Debug` always prints `[redacted]`, so
/// every other field stays visible by default and a future field needs no
/// opt-in to show up in diagnostics — see the hazard note on
/// `crate::geoip::GeoIpConfig`, whose hand-written `Debug` impl was the
/// original instance of the pattern this inverts.
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct GatewayConfig {
    /// Controller conf_sync gRPC addresses (repeatable). One address is exactly
    /// the previous single-address behavior; multiple addresses enable etcd-mode
    /// HA failover — the gateway rotates through the list to find the leader
    /// (spec §7.1). E.g. `--server-addr 10.0.0.11:12151 --server-addr 10.0.0.12:12151`.
    #[arg(long = "server-addr", value_name = "ADDR")]
    #[serde(default)]
    pub server_addrs: Vec<String>,

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
    /// out, and redacted in `Debug` (`RedactedString` — see the struct doc).
    #[arg(skip)]
    #[serde(default, skip_serializing)]
    pub admin_token: Option<crate::util::RedactedString>,

    /// Require BoringSSL FIPS mode at startup. Default false.
    /// When true and the module is absent (a non-FIPS build), the gateway logs
    /// a clear error and exits non-zero. Also settable via `EDGION_REQUIRE_FIPS`
    /// (truthy: 1/true/yes/on). See the FIPS design spec.
    #[arg(long = "require-fips")]
    #[serde(default)]
    pub require_fips: bool,

    /// GeoIP database and updater settings. Process-wide and restart-only.
    #[arg(skip)]
    #[serde(default)]
    pub geoip: crate::geoip::GeoIpConfig,

    /// How to select the FIPS-approved TLS policy (approved cipher list +
    /// curves) on downstream listeners. `auto` (the default) follows the host
    /// kernel's FIPS mode; `on` always enforces; `off` never does. A real FIPS
    /// build enforces the policy unconditionally regardless of this setting.
    ///
    /// Accepts only `auto`/`on`/`off`. Also settable via `EDGION_FIPS_POLICY`
    /// (same three values), which takes precedence over this field. `None`
    /// means "not specified".
    /// See skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md.
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

    /// Maximum decoded HTTP/2 response-header-list size, in bytes, for every
    /// canonical reqwest client in this Gateway process. Restart-only.
    #[arg(skip)]
    #[serde(default = "default_outbound_http2_max_header_list_size")]
    pub outbound_http2_max_header_list_size: u32,

    /// Seconds since last use before an OCI memory-cache entry expires (default 24h).
    /// Zero is clamped to one second at startup.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_ttl_secs")]
    pub wasm_oci_cache_ttl_secs: u64,

    /// Seconds between sweeps of the OCI memory cache (default 60s).
    /// Values below 1 second (including `0`) are clamped up to 1 second at
    /// startup to avoid a busy purge loop; `0` carries no "disable" meaning.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_purge_interval_secs")]
    pub wasm_oci_cache_purge_interval_secs: u64,

    /// Retained OCI payload allocation budget (default 256MiB), including
    /// evicted payloads still borrowed by loaders. Zero disables retention.
    /// In-flight acquisition and compiled VM memory have separate bounds.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_max_bytes")]
    pub wasm_oci_cache_max_bytes: u64,

    /// Maximum retained OCI manifest/blob entries (default 256, at most 65536).
    /// Zero disables retention; this is not a limit on compiled modules.
    #[arg(skip)]
    #[serde(default = "default_wasm_oci_cache_max_entries")]
    pub wasm_oci_cache_max_entries: usize,

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

fn default_outbound_http2_max_header_list_size() -> u32 {
    edgion_common::link_sys_core::http_client::DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE
}

fn default_wasm_oci_cache_ttl_secs() -> u64 {
    86_400
}
fn default_wasm_oci_cache_purge_interval_secs() -> u64 {
    60
}
fn default_wasm_oci_cache_max_bytes() -> u64 {
    256 * 1024 * 1024
}
fn default_wasm_oci_cache_max_entries() -> usize {
    256
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
            server_addrs: Vec::new(),
            admin_listen: None,
            probe_listen: None,
            admin_token: None,
            require_fips: false,
            geoip: crate::geoip::GeoIpConfig::default(),
            fips_policy: None,
            outbound_egress_block_private: false,
            outbound_egress_dns_cache_ttl_ms: default_outbound_egress_dns_cache_ttl_ms(),
            outbound_http2_max_header_list_size: default_outbound_http2_max_header_list_size(),
            wasm_oci_cache_ttl_secs: default_wasm_oci_cache_ttl_secs(),
            wasm_oci_cache_purge_interval_secs: default_wasm_oci_cache_purge_interval_secs(),
            wasm_oci_cache_max_bytes: default_wasm_oci_cache_max_bytes(),
            wasm_oci_cache_max_entries: default_wasm_oci_cache_max_entries(),
            wasm_oci_pull_timeout_secs: default_wasm_oci_pull_timeout_secs(),
            wasm_oci_pull_attempts: default_wasm_oci_pull_attempts(),
        }
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

/// Startup gate: when FIPS mode is required but absent,
/// return an error string for the caller to log before exiting non-zero.
pub fn check_fips_requirement(require_fips: bool, module_available: bool) -> Result<(), String> {
    if require_fips && !module_available {
        return Err("FIPS required (require_fips / EDGION_REQUIRE_FIPS) but this binary \
             does not run BoringSSL in FIPS mode; deploy a FIPS-mode build \
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

    /// Dedicated downstream TLS handshake pools per terminating listener (1..=4).
    #[arg(long = "downstream-tls-offload-threadpools", value_name = "NUM")]
    #[serde(default)]
    pub downstream_tls_offload_threadpools: Option<usize>,

    /// Threads per downstream TLS handshake pool (1..=4).
    #[arg(long = "downstream-tls-offload-thread-per-pool", value_name = "NUM")]
    #[serde(default)]
    pub downstream_tls_offload_thread_per_pool: Option<usize>,

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
    /// Optional bounded GenAI token trend counter.
    pub genai_tokens: GenAiTokensConfig,
    /// Max distinct series before the runtime cardinality guard acts (0 = off).
    #[serde(default = "default_cardinality_cap")]
    pub cardinality_cap: usize,
    /// Guard behavior on breach: enforce (drop new series) | observe (flag only).
    #[serde(default)]
    pub cardinality_mode: edgion_common::observe::cardinality_guard::CardinalityMode,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:12090".to_string(),
            request_duration: RequestDurationConfig::default(),
            backend_requests: BackendRequestsConfig::default(),
            genai_tokens: GenAiTokensConfig::default(),
            cardinality_cap: default_cardinality_cap(),
            cardinality_mode: Default::default(),
        }
    }
}

fn default_cardinality_cap() -> usize {
    10_000
}

/// `edgion_gateway_genai_tokens_total` configuration
/// (`metrics.genai_tokens` in YAML).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GenAiTokensConfig {
    /// Enable the bounded token rate/trend counter. Per-request access-log
    /// usage is always recorded independently of this switch.
    pub enabled: bool,
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
// GenAI Token Counter Configuration
// ============================================

/// Global restart-bound GenAI token counter configuration.
static GENAI_TOKENS_CONFIG: OnceLock<GenAiTokensConfig> = OnceLock::new();

/// Initialize the global GenAI token counter configuration once at startup.
pub fn init_genai_tokens_config(config: GenAiTokensConfig) {
    let result = GENAI_TOKENS_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_genai_tokens_config called more than once");
}

/// Get the GenAI token counter configuration. Tests and tooling that do not
/// boot the Gateway default to disabled without registering the metric family.
#[inline]
pub fn get_genai_tokens_config() -> &'static GenAiTokensConfig {
    GENAI_TOKENS_CONFIG.get_or_init(GenAiTokensConfig::default)
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
// GeoIP Log Options Global Configuration
// ============================================

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

fn validate_effective_lru_cache_limits(capacity: usize, shards: usize) -> Result<(), String> {
    if capacity == 0 {
        return Err("LRU cache capacity must be greater than zero".to_string());
    }
    if capacity > MAX_LRU_CACHE_CAPACITY {
        return Err(format!(
            "LRU cache capacity {capacity} exceeds the safety ceiling {MAX_LRU_CACHE_CAPACITY}"
        ));
    }
    let effective_shards = shards.max(1);
    if effective_shards > MAX_LRU_CACHE_SHARDS {
        return Err(format!(
            "LRU cache shards {effective_shards} exceeds the safety ceiling {MAX_LRU_CACHE_SHARDS}"
        ));
    }
    if effective_shards > capacity {
        return Err("LRU cache shards must not exceed capacity".to_string());
    }
    Ok(())
}

/// Initialize the validated global LRU cache configuration once at startup.
pub fn init_lru_cache_global_config(config: &LruCacheGlobalConfig) -> Result<(), String> {
    validate_effective_lru_cache_limits(config.default_capacity, config.default_shards)?;
    let result = LRU_CACHE_GLOBAL_CONFIG.set(config.clone());
    debug_assert!(result.is_ok(), "init_lru_cache_global_config called more than once");
    Ok(())
}

pub fn resolve_lru_cache_limits(config: &LruCacheConfig) -> Result<(usize, usize), String> {
    config
        .validation_error()
        .map_or(Ok(()), |error| Err(error.to_string()))?;
    let (capacity, shards) = if config.capacity == 0 {
        (get_default_lru_cache_capacity(), get_default_lru_cache_shards())
    } else {
        (config.capacity, config.shards)
    };
    validate_effective_lru_cache_limits(capacity, shards)?;
    Ok((capacity, shards))
}

/// Get the default LRU cache capacity.
/// Returns the configured value, or compile-time fallback 1024 if not yet initialized.
/// Never returns 0 — safe to pass directly to `ShardedLruCache::new()`.
pub fn get_default_lru_cache_capacity() -> usize {
    LRU_CACHE_GLOBAL_CONFIG
        .get()
        .map(|config| config.default_capacity)
        .unwrap_or(DEFAULT_LRU_CACHE_CAPACITY)
}

/// Get the default LRU cache shard count.
/// Returns 0 (disabled) if not configured.
pub fn get_default_lru_cache_shards() -> usize {
    LRU_CACHE_GLOBAL_CONFIG
        .get()
        .map(|config| config.default_shards)
        .unwrap_or(DEFAULT_LRU_CACHE_SHARDS)
}

// ============================================
// TinyUFO TTL Cache Global Configuration
// ============================================

const DEFAULT_TINY_UFO_TTL_CACHE_MAX_ENTRIES: usize = 1024;
const DEFAULT_TINY_UFO_TTL_CACHE_MAX_LOGICAL_BYTES: &str = "4MiB";
const DEFAULT_REFRESH_SINGLEFLIGHT_MAX_REGISTERED_KEYS: usize = 1024;

fn default_tiny_ufo_ttl_cache_max_entries() -> usize {
    DEFAULT_TINY_UFO_TTL_CACHE_MAX_ENTRIES
}

fn default_tiny_ufo_ttl_cache_max_logical_bytes() -> String {
    DEFAULT_TINY_UFO_TTL_CACHE_MAX_LOGICAL_BYTES.to_string()
}

/// Global defaults for bounded TinyUFO TTL caches.
///
/// ## YAML Configuration Example:
/// ```yaml
/// tiny_ufo_ttl_cache:
///   default_max_entries: 1024
///   default_max_logical_bytes: 4MiB
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TinyUfoTtlCacheGlobalConfig {
    #[serde(default = "default_tiny_ufo_ttl_cache_max_entries")]
    pub default_max_entries: usize,

    #[serde(default = "default_tiny_ufo_ttl_cache_max_logical_bytes")]
    pub default_max_logical_bytes: String,
}

impl Default for TinyUfoTtlCacheGlobalConfig {
    fn default() -> Self {
        Self {
            default_max_entries: DEFAULT_TINY_UFO_TTL_CACHE_MAX_ENTRIES,
            default_max_logical_bytes: default_tiny_ufo_ttl_cache_max_logical_bytes(),
        }
    }
}

static TINY_UFO_TTL_CACHE_GLOBAL_CONFIG: OnceLock<TinyUfoTtlCacheGlobalConfig> = OnceLock::new();

/// Initialize validated TinyUFO TTL-cache defaults once at Gateway startup.
pub fn init_tiny_ufo_ttl_cache_global_config(config: &TinyUfoTtlCacheGlobalConfig) -> Result<(), String> {
    resolve_tiny_ufo_ttl_cache_limits_with(config, &TinyUfoTtlCacheConfig::default())?;
    let result = TINY_UFO_TTL_CACHE_GLOBAL_CONFIG.set(config.clone());
    debug_assert!(
        result.is_ok(),
        "init_tiny_ufo_ttl_cache_global_config called more than once"
    );
    Ok(())
}

fn resolve_tiny_ufo_ttl_cache_limits_with(
    global: &TinyUfoTtlCacheGlobalConfig,
    plugin: &TinyUfoTtlCacheConfig,
) -> Result<crate::cache::TinyUfoCacheLimits, String> {
    if let Some(error) = plugin.validation_error() {
        return Err(error.to_string());
    }
    let max_entries = if plugin.max_entries == 0 {
        global.default_max_entries
    } else {
        plugin.max_entries
    };
    let logical_bytes = match plugin.parsed_max_logical_bytes().map_err(str::to_string)? {
        Some(bytes) => bytes,
        None => parse_byte_size(&global.default_max_logical_bytes)
            .ok_or_else(|| "defaultMaxLogicalBytes must be a valid byte size".to_string())?,
    };
    let logical_bytes = usize::try_from(logical_bytes)
        .map_err(|_| "TinyUFO TTL cache logical budget does not fit this platform".to_string())?;
    validate_tiny_ufo_effective_limits(max_entries, logical_bytes).map_err(str::to_string)?;
    let limits = crate::cache::TinyUfoCacheLimits::try_new(max_entries, logical_bytes).map_err(|e| e.to_string())?;
    crate::cache::TinyUfoTtlCache::<String, String>::validate_limits(limits).map_err(|e| e.to_string())?;
    Ok(limits)
}

pub fn resolve_tiny_ufo_ttl_cache_limits(
    config: &TinyUfoTtlCacheConfig,
) -> Result<crate::cache::TinyUfoCacheLimits, String> {
    let global = TINY_UFO_TTL_CACHE_GLOBAL_CONFIG.get().cloned().unwrap_or_default();
    resolve_tiny_ufo_ttl_cache_limits_with(&global, config)
}

pub fn get_refresh_singleflight_max_registered_keys() -> usize {
    DEFAULT_REFRESH_SINGLEFLIGHT_MAX_REGISTERED_KEYS
}

// ============================================
impl EdgionGatewayConfig {
    /// Load configuration from YAML file if config_file is specified,
    /// then merge with CLI arguments (CLI takes precedence)
    pub(crate) fn load(cli_config: Self) -> Result<LoadedGatewayConfig> {
        // Load from file if specified
        let (mut file_config, unknown_fields) = if let Some(ref config_path) = cli_config.config_file {
            let content =
                std::fs::read_to_string(config_path).context(format!("Failed to read config file: {}", config_path))?;

            deserialize_process_config(&content).context(format!("Failed to parse config file: {}", config_path))?
        } else {
            (EdgionGatewayConfig::default(), UnknownConfigFields::default())
        };

        // Merge: CLI args override file config
        Self::merge(&mut file_config, &cli_config);

        Ok(LoadedGatewayConfig {
            config: file_config,
            unknown_fields,
        })
    }

    /// Merge CLI config into file config (CLI takes precedence)
    pub(crate) fn merge(base: &mut Self, cli: &Self) {
        // Work directory: CLI value takes precedence if provided
        if cli.work_dir.is_some() {
            base.work_dir = cli.work_dir.clone();
        }

        if cli.server.downstream_tls_offload_threadpools.is_some() {
            base.server.downstream_tls_offload_threadpools = cli.server.downstream_tls_offload_threadpools;
        }
        if cli.server.downstream_tls_offload_thread_per_pool.is_some() {
            base.server.downstream_tls_offload_thread_per_pool = cli.server.downstream_tls_offload_thread_per_pool;
        }

        // Gateway config. `server_addrs` is a Vec: an EMPTY CLI vec means the
        // flag was not passed and must not clobber the file value (spec §7.1).
        if !cli.gateway.server_addrs.is_empty() {
            base.gateway.server_addrs = cli.gateway.server_addrs.clone();
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

    /// Controller conf_sync addresses. One element == the previous
    /// single-address behavior; empty == not configured.
    pub fn server_addrs(&self) -> Vec<String> {
        self.gateway.server_addrs.clone()
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

fn deserialize_process_config(
    content: &str,
) -> std::result::Result<(EdgionGatewayConfig, UnknownConfigFields), serde_yaml::Error> {
    struct ProcessConfigInput(EdgionGatewayConfig);

    impl<'de> Deserialize<'de> for ProcessConfigInput {
        fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            serde_yaml::with::singleton_map_recursive::deserialize(deserializer).map(Self)
        }
    }

    let de = serde_yaml::Deserializer::from_str(content);
    let mut unknown_fields = UnknownConfigFields::default();
    let ProcessConfigInput(config) = serde_ignored::deserialize(de, |path| unknown_fields.record(&path.to_string()))?;
    Ok((config, unknown_fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::rate_limit_local_config::{
        get_gateway_instance_count, set_gateway_instance_count, DEFAULT_ESTIMATOR_SLOTS_K,
        DEFAULT_MAX_ESTIMATOR_SLOTS_K,
    };

    fn unknown_paths(yaml: &str) -> UnknownConfigFields {
        deserialize_process_config(yaml).unwrap().1
    }

    #[test]
    fn clean_process_config_has_no_unknown_fields() {
        let yaml = r#"
gateway:
  server_addrs: ["http://127.0.0.1:12151"]
metrics:
  request_duration:
    enabled: true
    idle_timeout_secs: 30
  backend_requests:
    labels:
      gateway: true
access_log:
  enabled: true
  output:
    localFile:
      path: logs/access.log
      queueSize: 1024
      rotation:
        strategy: daily
        maxFiles: 7
  fallback:
    localFile:
      path: logs/fallback.log
conf_sync_security:
  active: insecure
  certs:
    - name: insecure
      skip_tls: true
  rotation:
    policy: lifetime
auth_cache:
  default_capacity: 128
  default_shards: 2
"#;

        let (config, unknown) = deserialize_process_config(yaml).unwrap();
        assert!(unknown.paths().next().is_none());
        assert!(!unknown.is_truncated());
        assert_eq!(config.lru_cache.default_capacity, 128);
        assert_eq!(config.lru_cache.default_shards, 2);
    }

    #[test]
    fn shipped_gateway_process_config_has_no_unknown_fields() {
        let (_, unknown) = deserialize_process_config(include_str!("../../../config/edgion-gateway.yaml")).unwrap();
        assert_eq!(unknown.paths().collect::<Vec<_>>(), Vec::<&str>::new());
        assert!(!unknown.is_truncated());
    }

    #[test]
    fn process_config_collects_root_and_nested_unknown_field_paths() {
        let yaml = r#"
geoip:
  db_path: /var/lib/edgion/geo.mmdb
metrics:
  request_duration:
    idle_timeout_sec: 30
access_log:
  output:
    localFile:
      queue_size: 1024
      rotation:
        max_files: 7
  fallback:
    localFile:
      fallback_typo: true
conf_sync_security:
  certs:
    - name: insecure
      skip_tls: true
      cert_typo: ignored
  rotation:
    lifetime_ratio_typo: 0.4
future_section:
  nested_secret_value: must-not-be-reported
"#;

        let (config, unknown) = deserialize_process_config(yaml).unwrap();
        assert_eq!(config.metrics.request_duration.idle_timeout_secs, 600);
        assert!(matches!(
            config.access_log.output,
            edgion_resources::StringOutput::LocalFile(_)
        ));
        assert_eq!(
            unknown.paths().collect::<Vec<_>>(),
            vec![
                "access_log.fallback.localFile.fallback_typo",
                "access_log.output.localFile.queue_size",
                "access_log.output.localFile.rotation.max_files",
                "conf_sync_security.certs[0].cert_typo",
                "conf_sync_security.rotation.lifetime_ratio_typo",
                "future_section",
                "geoip",
                "metrics.request_duration.idle_timeout_sec",
            ]
        );
        assert!(!unknown.is_truncated());
    }

    #[test]
    fn process_config_unknown_field_diagnostics_are_bounded() {
        let yaml = (0..MAX_UNKNOWN_CONFIG_FIELDS + 5)
            .map(|index| format!("unknown_{index:02}: true\n"))
            .collect::<String>();
        let unknown = unknown_paths(&yaml);
        assert_eq!(unknown.paths().count(), MAX_UNKNOWN_CONFIG_FIELDS);
        assert!(unknown.is_truncated());
    }

    #[test]
    fn process_config_keeps_strict_substructures_strict() {
        for yaml in [
            "request_mirror:\n  max_concurent: 12\n",
            "access_log_options:\n  include_raw_uri: true\n",
            "path_normalization:\n  reject_escape_slashes: true\n",
            "gateway:\n  geoip:\n    dbPath: /tmp/geo.mmdb\n",
            "access_log:\n  output:\n    unknownSink: {}\n",
        ] {
            assert!(deserialize_process_config(yaml).is_err(), "must remain strict: {yaml}");
        }
    }

    #[test]
    fn process_config_without_a_file_has_no_unknown_diagnostics() {
        let loaded = EdgionGatewayConfig::load(EdgionGatewayConfig::default()).unwrap();
        assert!(loaded.unknown_fields.paths().next().is_none());
        assert!(!loaded.unknown_fields.is_truncated());
    }

    #[test]
    fn process_config_load_preserves_cli_precedence_and_unknown_diagnostics() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"gateway:\n  admin_listen: 127.0.0.1:12002\nroot_typo: ignored\n")
            .unwrap();
        let cli = EdgionGatewayConfig {
            config_file: Some(file.path().to_string_lossy().into_owned()),
            gateway: GatewayConfig {
                admin_listen: Some("127.0.0.1:12003".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let loaded = EdgionGatewayConfig::load(cli).unwrap();
        assert_eq!(loaded.config.gateway.admin_listen.as_deref(), Some("127.0.0.1:12003"));
        assert_eq!(loaded.unknown_fields.paths().collect::<Vec<_>>(), vec!["root_typo"]);
    }

    /// The gateway instance count is a PROCESS-GLOBAL, and `cargo test` runs the tests in
    /// this binary on parallel threads. Without serialization, `set_and_get` writing 5 can
    /// land between `never_zero`'s write of 0 and its read, so `never_zero` observes 5 and
    /// fails an assertion about clamping that has nothing to do with what actually broke.
    /// (Observed exactly once under a full `cargo test --all`, where cross-binary CPU
    /// contention widens the window; the three tests pass on their own.)
    ///
    /// `parking_lot::Mutex` per the workspace lint: it does not poison, so a sibling test
    /// panicking while holding the lock cannot cascade into unrelated failures here.
    static INSTANCE_COUNT_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn test_gateway_instance_count_default() {
        let _guard = INSTANCE_COUNT_LOCK.lock();
        // Default should be at least 1
        assert!(get_gateway_instance_count() >= 1);
    }

    #[test]
    fn test_gateway_instance_count_set_and_get() {
        let _guard = INSTANCE_COUNT_LOCK.lock();
        set_gateway_instance_count(5);
        assert_eq!(get_gateway_instance_count(), 5);
        set_gateway_instance_count(1); // cleanup
    }

    #[test]
    fn test_gateway_instance_count_never_zero() {
        let _guard = INSTANCE_COUNT_LOCK.lock();
        set_gateway_instance_count(0); // Try to set zero
        assert_eq!(get_gateway_instance_count(), 1); // Should be clamped to 1
        set_gateway_instance_count(1); // cleanup: leave the global in its default state
    }

    #[test]
    fn test_tiny_ufo_ttl_cache_global_config_default() {
        let config = TinyUfoTtlCacheGlobalConfig::default();
        assert_eq!(config.default_max_entries, 1024);
        assert_eq!(config.default_max_logical_bytes, "4MiB");
    }

    #[test]
    fn test_tiny_ufo_ttl_cache_default_limits_resolve() {
        let limits = resolve_tiny_ufo_ttl_cache_limits_with(
            &TinyUfoTtlCacheGlobalConfig::default(),
            &TinyUfoTtlCacheConfig::default(),
        )
        .expect("default limits must be valid");
        assert_eq!(limits.max_entries.get(), 1024);
        assert_eq!(limits.max_logical_bytes.get(), 4 * 1024 * 1024);
    }

    #[test]
    fn test_lru_cache_global_config_remains_available() {
        let config = LruCacheGlobalConfig::default();
        assert_eq!(config.default_capacity, 1024);
        assert_eq!(config.default_shards, 0);
        assert!(get_default_lru_cache_capacity() > 0);
    }

    #[test]
    fn lru_cache_limits_reject_zero_oversized_and_capacity_expanding_shards() {
        assert!(validate_effective_lru_cache_limits(0, 1).is_err());
        assert!(validate_effective_lru_cache_limits(MAX_LRU_CACHE_CAPACITY + 1, 1).is_err());
        assert!(validate_effective_lru_cache_limits(4, 5).is_err());
        assert!(validate_effective_lru_cache_limits(1024, MAX_LRU_CACHE_SHARDS + 1).is_err());
        assert!(validate_effective_lru_cache_limits(1024, 8).is_ok());
    }

    #[test]
    fn test_rate_limit_local_global_config_default() {
        let config = RateLimitLocalGlobalConfig::default();
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
    fn request_mirror_process_limits_survive_cli_merge() {
        let yaml = "request_mirror:\n  max_concurrent: 17\n  max_buffered_bytes: 8MiB\n";
        let de = serde_yaml::Deserializer::from_str(yaml);
        let mut file: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
        EdgionGatewayConfig::merge(&mut file, &EdgionGatewayConfig::default());
        let resolved = file.request_mirror.resolve().unwrap();
        assert_eq!(resolved.max_concurrent, 17);
        assert_eq!(resolved.max_buffered_bytes, 8 * 1024 * 1024);
        assert!(serde_yaml::from_str::<EdgionGatewayConfig>("request_mirror:\n  max_concurent: 17\n").is_err());
    }

    #[test]
    fn access_log_options_use_the_canonical_key_in_root_gateway_yaml() {
        let canonical: EdgionGatewayConfig =
            serde_yaml::from_str("access_log_options:\n  includeRawUri: true\n").unwrap();
        assert!(canonical.access_log_options.include_raw_uri);

        assert!(serde_yaml::from_str::<EdgionGatewayConfig>("access_log_options:\n  include_raw_uri: true\n").is_err());
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
    fn server_addrs_merge_and_default() {
        let mut base = EdgionGatewayConfig::default();
        base.gateway.server_addrs = vec!["10.0.0.1:9090".into()];
        let mut cli = EdgionGatewayConfig::default(); // empty server_addrs
                                                      // An empty CLI vec must NOT clobber the file value (spec §7.1).
        EdgionGatewayConfig::merge(&mut base, &cli);
        assert_eq!(base.server_addrs(), vec!["10.0.0.1:9090".to_string()]);
        // A non-empty CLI vec overrides.
        cli.gateway.server_addrs = vec!["10.0.0.2:9090".into(), "10.0.0.3:9090".into()];
        EdgionGatewayConfig::merge(&mut base, &cli);
        assert_eq!(base.server_addrs().len(), 2);
    }

    #[test]
    fn test_access_log_sensitive_default_is_replace() {
        // Privacy-by-default: `ext` rendering (and the built-in sensitive
        // fields) follow this mode, so shipping `off` would leave the
        // EdgionGatewayConfig whitelist inert out of the box.
        let cfg: EdgionGatewayConfig = serde_yaml::from_str("{}").unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            edgion_resources::sensitive::SensitiveMode::Replace
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
    fn genai_token_metric_is_disabled_by_default_and_yaml_can_enable_it() {
        assert!(!MetricsConfig::default().genai_tokens.enabled);

        let cfg: MetricsConfig = serde_yaml::from_str(
            r#"
genai_tokens:
  enabled: true
"#,
        )
        .unwrap();
        assert!(cfg.genai_tokens.enabled);
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
                admin_token: Some("must-not-leak".into()),
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
            admin_token: Some("must-not-leak".into()),
            ..Default::default()
        };
        let dbg = format!("{:?}", cfg);
        assert!(!dbg.contains("must-not-leak"), "admin_token must be redacted in Debug");
        assert!(dbg.contains("[redacted]"));
    }

    /// `GatewayConfig` derives `Debug` (see the struct doc): this is now a
    /// smoke test that every field, including a nested config struct like
    /// `geoip`, actually reaches the derived output — not a guard against a
    /// hand-maintained `.field(...)` list going stale, which no longer exists
    /// for this struct.
    #[test]
    fn debug_output_includes_geoip() {
        let cfg = GatewayConfig::default();
        let dbg = format!("{:?}", cfg);
        assert!(
            dbg.contains("geoip"),
            "GatewayConfig Debug output must include the geoip field"
        );
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
        assert_eq!(cfg.outbound_http2_max_header_list_size, 16 * 1024);
    }

    #[test]
    fn outbound_egress_parses_from_yaml() {
        let cfg: GatewayConfig = serde_yaml::from_str(
            "outbound_egress_block_private: true\noutbound_egress_dns_cache_ttl_ms: 30000\n\
             outbound_http2_max_header_list_size: 65536\n",
        )
        .unwrap();
        assert!(cfg.outbound_egress_block_private);
        assert_eq!(cfg.outbound_egress_dns_cache_ttl_ms, 30_000);
        assert_eq!(cfg.outbound_http2_max_header_list_size, 64 * 1024);
    }

    #[test]
    fn wasm_oci_cache_fields_parse_and_default() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.wasm_oci_cache_ttl_secs, 86_400);
        assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 60);
        assert_eq!(cfg.wasm_oci_cache_max_bytes, 256 * 1024 * 1024);
        assert_eq!(cfg.wasm_oci_cache_max_entries, 256);
        let cfg: GatewayConfig = serde_yaml::from_str(
            "wasm_oci_cache_max_entries: 0\nwasm_oci_cache_ttl_secs: 60\n\
             wasm_oci_cache_purge_interval_secs: 30\nwasm_oci_cache_max_bytes: 0\n",
        )
        .unwrap();
        assert_eq!(cfg.wasm_oci_cache_max_entries, 0);
        assert_eq!(cfg.wasm_oci_cache_ttl_secs, 60);
        assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 30);
        assert_eq!(cfg.wasm_oci_cache_max_bytes, 0);
        // A zero purge interval parses as-is (no "disable" semantics); the
        // busy-loop guard is a startup clamp in `configure_oci_memory_cache`,
        // not a parse-time rejection.
        let cfg: GatewayConfig = serde_yaml::from_str("wasm_oci_cache_purge_interval_secs: 0\n").unwrap();
        assert_eq!(cfg.wasm_oci_cache_purge_interval_secs, 0);
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
    fn otlp_reference_uses_production_yaml_deserialization() {
        let yaml = "access_log:\n  output:\n    otlp:\n      linkSysRef:\n        namespace: observability\n        name: collector\n";
        let de = serde_yaml::Deserializer::from_str(yaml);
        let cfg: EdgionGatewayConfig = serde_yaml::with::singleton_map_recursive::deserialize(de).unwrap();
        let edgion_resources::StringOutput::Otlp { link_sys_ref } = cfg.access_log.output else {
            panic!("expected OTLP reference output");
        };
        assert_eq!(link_sys_ref.key(), "observability/collector");
    }

    #[test]
    fn otlp_invalid_references_fail_every_process_log_output_before_startup() {
        // Disabled logs and fallback positions are still configuration inputs;
        // validation must precede logger construction and identity emission.
        for log in ["access_log", "ssl_log", "tcp_log", "tls_log", "udp_log"] {
            for position in ["output", "fallback"] {
                for field in ["namespace", "name"] {
                    for invalid in ["".to_owned(), "private-sentinel/invalid".to_owned(), "x".repeat(254)] {
                        let mut reference = serde_json::json!({"namespace": "default", "name": "collector"});
                        reference[field] = serde_json::Value::String(invalid.clone());
                        let mut config = serde_json::json!({});
                        config[log] = serde_json::json!({"enabled": false});
                        config[log][position] = serde_json::json!({"otlp": {"linkSysRef": reference}});
                        let yaml = serde_yaml::to_string(&config).unwrap();
                        let de = serde_yaml::Deserializer::from_str(&yaml);
                        let result: Result<EdgionGatewayConfig, _> =
                            serde_yaml::with::singleton_map_recursive::deserialize(de);
                        let error = result
                            .expect_err("invalid OTLP reference must fail config loading")
                            .to_string();
                        if !invalid.is_empty() {
                            assert!(
                                !error.contains(&invalid),
                                "reference was echoed: {log}.{position}.{field}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn otlp_inline_process_configuration_is_rejected() {
        for inline in [
            "endpoint: https://private-sentinel.example:4317",
            "protocol: grpc",
            "headers: {authorization: private-sentinel}",
        ] {
            let yaml = format!(
                "access_log:\n  output:\n    otlp:\n      linkSysRef: {{namespace: default, name: collector}}\n      {inline}\n"
            );
            let de = serde_yaml::Deserializer::from_str(&yaml);
            let result: Result<EdgionGatewayConfig, _> = serde_yaml::with::singleton_map_recursive::deserialize(de);
            let error = result.expect_err("inline OTLP fields must be rejected").to_string();
            assert!(!error.contains("private-sentinel"));
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
