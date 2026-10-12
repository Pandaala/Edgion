use crate::plugins::http::request_mirror::RequestMirrorGlobalConfig;
use crate::runtime::server::downstream_h2::DownstreamH2Policy;
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
    /// Restart-only physical limits for the single optional response cache.
    #[arg(skip)]
    #[serde(default)]
    pub response_cache: ResponseCacheLimits,
    #[arg(skip)]
    #[serde(default)]
    pub ai: crate::ai::config::AiConfig,
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
    #[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ResponseCacheLimits {
    pub max_entries: usize,
    pub shards: usize,
    pub max_body_bytes: usize,
    pub max_metadata_bytes: usize,
    pub total_bytes: usize,
    pub pending_bytes: usize,
}

impl Default for ResponseCacheLimits {
    fn default() -> Self {
        Self {
            max_entries: 64,
            shards: 16,
            max_body_bytes: 1_048_576,
            max_metadata_bytes: 16_384,
            total_bytes: 134_217_728,
            pending_bytes: 16_777_216,
        }
    }
}

impl ResponseCacheLimits {
    pub(crate) fn validate(&self) -> Result<usize, &'static str> {
        let maxima = Self::default();
        if [
            self.max_entries,
            self.shards,
            self.max_body_bytes,
            self.max_metadata_bytes,
            self.total_bytes,
            self.pending_bytes,
        ]
        .contains(&0)
            || self.max_entries > maxima.max_entries
            || self.shards > maxima.shards
            || self.max_body_bytes > maxima.max_body_bytes
            || self.max_metadata_bytes > maxima.max_metadata_bytes
            || self.total_bytes > maxima.total_bytes
            || self.pending_bytes > maxima.pending_bytes
            || validate_effective_lru_cache_limits(self.max_entries, self.shards).is_err()
        {
            return Err("response_cache physical limits are invalid");
        }
        let infrastructure = self
            .max_entries
            .checked_mul(4096)
            .and_then(|n| self.shards.checked_mul(4096).and_then(|shards| n.checked_add(shards)))
            .and_then(|n| n.checked_add(4096))
            .and_then(|n| n.checked_add(crate::response_cache::epoch_control_bytes()))
            .ok_or("response_cache infrastructure limit overflow")?;
        let population = self
            .max_body_bytes
            .checked_add(self.max_metadata_bytes)
            .and_then(|n| n.checked_add(1024))
            .and_then(|n| n.checked_mul(self.max_entries))
            .ok_or("response_cache population limit overflow")?;
        let required = infrastructure
            .checked_add(population)
            .and_then(|n| n.checked_add(self.pending_bytes))
            .ok_or("response_cache total limit overflow")?;
        if required > self.total_bytes {
            return Err("response_cache total_bytes cannot admit its configured index and pending budgets");
        }
        Ok(infrastructure)
    }
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

    /// Process-local IP used for Gateway listener sockets. External Gateway
    /// addresses belong to the Service or load balancer, never to this bind.
    #[arg(long = "bind-ip", value_name = "IP")]
    #[serde(default)]
    pub bind_ip: Option<std::net::IpAddr>,

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
            bind_ip: None,
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

    /// Downstream HTTP/2 stream receive window in bytes (65535..=16777216).
    #[arg(long = "downstream-h2-stream-window-size", value_name = "BYTES")]
    #[serde(default)]
    pub downstream_h2_stream_window_size: Option<u32>,

    /// Downstream HTTP/2 connection receive window in bytes (65535..=16777216).
    #[arg(long = "downstream-h2-connection-window-size", value_name = "BYTES")]
    #[serde(default)]
    pub downstream_h2_connection_window_size: Option<u32>,

    /// Downstream HTTP/2 concurrent streams per connection (1..=1024).
    #[arg(long = "downstream-h2-max-concurrent-streams", value_name = "NUM")]
    #[serde(default)]
    pub downstream_h2_max_concurrent_streams: Option<u32>,

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

        DownstreamH2Policy::try_from(&file_config.server)?;

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

        if cli.server.downstream_h2_stream_window_size.is_some() {
            base.server.downstream_h2_stream_window_size = cli.server.downstream_h2_stream_window_size;
        }
        if cli.server.downstream_h2_connection_window_size.is_some() {
            base.server.downstream_h2_connection_window_size = cli.server.downstream_h2_connection_window_size;
        }
        if cli.server.downstream_h2_max_concurrent_streams.is_some() {
            base.server.downstream_h2_max_concurrent_streams = cli.server.downstream_h2_max_concurrent_streams;
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
        if cli.gateway.bind_ip.is_some() {
            base.gateway.bind_ip = cli.gateway.bind_ip;
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
#[path = "../../tests/unit/cli/config_test.rs"]
mod tests;
