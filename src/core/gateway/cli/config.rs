use crate::types::LogConfig;
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing;

/// Default admin API listen address. Binds loopback by default so the admin API
/// (which serves secret-bearing `/configclient` introspection) is not exposed to the
/// cluster unless the operator opts in via `admin_listen`. Liveness/readiness live on
/// the separate probe port (`DEFAULT_GATEWAY_PROBE_LISTEN`), so kubelet never needs to
/// reach the admin port and the shipped manifests keep this on loopback. (Port 5810
/// avoids the macOS Screen Sharing / VNC conflict on 5900.)
pub const DEFAULT_GATEWAY_ADMIN_LISTEN: &str = "127.0.0.1:5810";

/// Default probe API listen address. The probe API serves only the non-sensitive
/// liveness/readiness endpoints (`/health`, `/ready`) and binds `0.0.0.0` by default
/// so a kubelet `httpGet` probe can reach it over the pod IP. The sensitive admin API
/// stays loopback (`DEFAULT_GATEWAY_ADMIN_LISTEN`) and need never be `0.0.0.0`.
pub const DEFAULT_GATEWAY_PROBE_LISTEN: &str = "0.0.0.0:28000";

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

    /// Configuration file path (TOML format)
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
    /// TOML example (replace mode):
    ///   [access_log_sensitive]
    ///   mode = "replace"
    ///   replace_with = "***"
    ///
    /// TOML example (wrap mode):
    ///   [access_log_sensitive]
    ///   mode = "wrap"
    ///   wrap_prefix = "<E>"
    ///   wrap_suffix = "</E>"
    #[arg(skip)]
    #[serde(default, skip_serializing)]
    pub access_log_sensitive: crate::types::sensitive::SensitiveMaskConfig,

    /// Access log output options that don't fit into the shared `LogConfig`.
    ///
    /// TOML example:
    ///   [access_log_options]
    ///   include_raw_uri = true
    #[arg(skip)]
    #[serde(default)]
    pub access_log_options: crate::types::observe::AccessLogOptions,

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
    pub all_endpoint_status:
        crate::core::gateway::plugins::http::all_endpoint_status::plugin::AllEndpointStatusGlobalConfig,

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
    pub conf_sync_security: crate::core::common::config::ConfSyncSecurityConfig,

    /// URI path normalization configuration.
    ///
    /// Controls rejection behaviour for unusual but technically valid encodings.
    /// Both flags default to `false`; the BASE normalization algorithm always runs.
    ///
    /// TOML example:
    ///   [path_normalization]
    ///   reject_escaped_slashes = true
    ///   reject_invalid_percent_encoding = false
    #[arg(skip)]
    #[serde(default)]
    pub path_normalization: crate::core::gateway::path_normalization::PathNormalizationConfig,
}

/// Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
pub struct GatewayConfig {
    /// Operator gRPC address (e.g., http://127.0.0.1:50051)
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

/// Metrics configuration section (`[metrics]` in TOML)
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
            listen: "0.0.0.0:5901".to_string(),
            request_duration: RequestDurationConfig::default(),
            backend_requests: BackendRequestsConfig::default(),
        }
    }
}

/// Request duration histogram configuration (`[metrics.request_duration]` in TOML)
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

/// Get the global request duration config (zero-cost after init).
#[inline]
pub fn get_request_duration_config() -> &'static RequestDurationConfig {
    REQUEST_DURATION_CONFIG
        .get()
        .expect("RequestDurationConfig not initialized")
}

// ============================================
// Backend Requests Counter Configuration
// ============================================

/// `edgion_backend_requests_total` configuration (`[metrics.backend_requests]` in TOML).
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
// Path Normalization Global Configuration
// ============================================

/// Global path normalization config, initialized once at startup.
static PATH_NORMALIZATION_CONFIG: OnceLock<crate::core::gateway::path_normalization::PathNormalizationConfig> =
    OnceLock::new();

/// Initialize the global path normalization config (call once at startup).
pub fn init_path_normalization_config(config: crate::core::gateway::path_normalization::PathNormalizationConfig) {
    let result = PATH_NORMALIZATION_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_path_normalization_config called more than once");
}

/// Get the global path normalization config (zero-cost after init).
///
/// Returns the default config (both reject flags off) if
/// `init_path_normalization_config` was not called (e.g., unit tests that
/// don't boot the full Gateway). Production paths always initialize before
/// the first request.
#[inline]
pub fn get_path_normalization_config() -> &'static crate::core::gateway::path_normalization::PathNormalizationConfig {
    PATH_NORMALIZATION_CONFIG.get_or_init(crate::core::gateway::path_normalization::PathNormalizationConfig::default)
}

// ============================================
// Access Log Options Global Configuration
// ============================================

/// Global access log options, initialized once at startup.
static ACCESS_LOG_OPTIONS: OnceLock<crate::types::observe::AccessLogOptions> = OnceLock::new();

/// Initialize the global access log options (call once at startup).
pub fn init_access_log_options(options: crate::types::observe::AccessLogOptions) {
    let result = ACCESS_LOG_OPTIONS.set(options);
    debug_assert!(result.is_ok(), "init_access_log_options called more than once");
}

/// Get the global access log options (zero-cost after init).
///
/// Returns the default (all flags off) if `init_access_log_options` was not
/// called, e.g. unit tests that don't boot the full Gateway.
#[inline]
pub fn get_access_log_options() -> &'static crate::types::observe::AccessLogOptions {
    ACCESS_LOG_OPTIONS.get_or_init(crate::types::observe::AccessLogOptions::default)
}

// ============================================
// RateLimit Global Configuration
// ============================================

/// Unit multiplier: 1K = 1024 slots
pub const SLOTS_K: usize = 1024;

/// Minimum CMS estimator slots in K (1K = 1024 slots, ~64KB)
pub const MIN_ESTIMATOR_SLOTS_K: usize = 1;

/// Default CMS estimator slots in K (64K = 65536 slots, ~4MB)
pub const DEFAULT_ESTIMATOR_SLOTS_K: usize = 64;

/// Maximum CMS estimator slots in K (1024K = 1M slots, ~64MB)
pub const DEFAULT_MAX_ESTIMATOR_SLOTS_K: usize = 1024;

/// RateLimit plugin global configuration
///
/// Controls the Count-Min Sketch (CMS) estimator settings for all RateLimit plugins.
/// The CMS algorithm provides memory-efficient rate limiting with configurable precision.
///
/// All slot values are in K units (1K = 1024 slots).
/// Memory per Rate instance ≈ slots_k × 64KB
///
/// ## TOML Configuration Example:
/// ```toml
/// [rate_limit]
/// default_estimator_slots_k = 64     # 64K slots = ~4MB
/// max_estimator_slots_k = 1024       # 1024K slots = ~64MB
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitGlobalConfig {
    /// Default CMS estimator slots in K units (default: 64)
    ///
    /// Controls the precision and memory usage of the rate limiting algorithm.
    /// Higher values reduce hash collisions but increase memory usage.
    /// Memory per Rate instance ≈ slots_k × 64KB
    ///
    /// | Value | Slots | Memory | Use Case |
    /// |-------|-------|--------|----------|
    /// | 1     | 1K    | 64KB   | Minimum  |
    /// | 8     | 8K    | 512KB  | Low cardinality |
    /// | 64    | 64K   | 4MB    | Default, most scenarios |
    /// | 256   | 256K  | 16MB   | High cardinality |
    /// | 1024  | 1M    | 64MB   | Maximum precision |
    #[serde(default = "default_estimator_slots_k")]
    pub default_estimator_slots_k: usize,

    /// Maximum allowed CMS estimator slots in K units (default: 1024)
    ///
    /// Limits the maximum slots that can be configured per RateLimit plugin.
    /// Maximum memory per Rate instance ≈ max_slots_k × 64KB
    #[serde(default = "default_max_estimator_slots_k")]
    pub max_estimator_slots_k: usize,

    /// Static fallback for gateway instance count (default: 1)
    ///
    /// Used when controller is not available to push dynamic count.
    /// Set this to your expected replica count for best-effort
    /// cluster rate limiting without controller.
    #[serde(default = "default_gateway_instance_count")]
    pub gateway_instance_count: u32,
}

fn default_estimator_slots_k() -> usize {
    DEFAULT_ESTIMATOR_SLOTS_K
}

fn default_max_estimator_slots_k() -> usize {
    DEFAULT_MAX_ESTIMATOR_SLOTS_K
}

fn default_gateway_instance_count() -> u32 {
    1
}

impl Default for RateLimitGlobalConfig {
    fn default() -> Self {
        Self {
            default_estimator_slots_k: DEFAULT_ESTIMATOR_SLOTS_K,
            max_estimator_slots_k: DEFAULT_MAX_ESTIMATOR_SLOTS_K,
            gateway_instance_count: default_gateway_instance_count(),
        }
    }
}

// ============================================
// Global RateLimit Config Store
// ============================================

use parking_lot::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;

// ============================================
// Gateway Instance Count (for Cluster scope)
// ============================================

/// Global gateway instance count.
///
/// Updated by the WatchServerMeta background task (control plane).
/// Read by RateLimit plugins on every request (data plane hot path).
///
/// Default is 1: when controller is unavailable, Cluster scope
/// degrades to Instance scope behavior (with skewTolerance applied).
static GATEWAY_INSTANCE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Update the gateway instance count (called by control plane)
pub fn set_gateway_instance_count(count: u32) {
    let count = count.max(1); // Prevent division by zero
    let old = GATEWAY_INSTANCE_COUNT.swap(count, Ordering::Relaxed);
    if old != count {
        tracing::info!(old_count = old, new_count = count, "Gateway instance count updated");
    }
}

/// Get the current gateway instance count (called by data plane hot path)
///
/// Cost: ~0.3ns on x86 (single MOV instruction, no memory fence)
#[inline]
pub fn get_gateway_instance_count() -> u32 {
    GATEWAY_INSTANCE_COUNT.load(Ordering::Relaxed)
}

/// Global store for RateLimit configuration
static RATE_LIMIT_GLOBAL_CONFIG: LazyLock<RwLock<RateLimitGlobalConfig>> =
    LazyLock::new(|| RwLock::new(RateLimitGlobalConfig::default()));

/// Initialize the global RateLimit configuration
///
/// This should be called once during application startup with the loaded config.
pub fn init_rate_limit_global_config(config: &RateLimitGlobalConfig) {
    let mut global = RATE_LIMIT_GLOBAL_CONFIG.write();
    *global = config.clone();

    // Initialize instance count from static config
    // (may be overridden by WatchServerMeta later)
    set_gateway_instance_count(config.gateway_instance_count);

    tracing::info!(
        default_slots_k = config.default_estimator_slots_k,
        max_slots_k = config.max_estimator_slots_k,
        default_slots = config.default_estimator_slots_k * SLOTS_K,
        max_slots = config.max_estimator_slots_k * SLOTS_K,
        gateway_instance_count = config.gateway_instance_count,
        "RateLimit global config initialized"
    );
}

/// Get the default estimator slots from global config (returns actual slot count)
pub fn get_default_estimator_slots() -> usize {
    RATE_LIMIT_GLOBAL_CONFIG.read().default_estimator_slots_k * SLOTS_K
}

/// Get the maximum estimator slots from global config (returns actual slot count)
pub fn get_max_estimator_slots() -> usize {
    RATE_LIMIT_GLOBAL_CONFIG.read().max_estimator_slots_k * SLOTS_K
}

/// Get the minimum estimator slots (returns actual slot count)
pub fn get_min_estimator_slots() -> usize {
    MIN_ESTIMATOR_SLOTS_K * SLOTS_K
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
/// ## TOML Configuration Example:
/// ```toml
/// [lru_cache]
/// default_capacity = 1024
/// default_shards = 0
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
    /// Load configuration from TOML file if config_file is specified,
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
    fn merge(base: &mut Self, cli: &Self) {
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

        // Access log config (CLI doesn't support overriding, only from file)
        // No merge needed as there are no CLI args for access_log

        // SSL log config (CLI doesn't support overriding, only from file)
        // No merge needed as there are no CLI args for ssl_log
    }

    /// Get server_addr
    pub fn server_addr(&self) -> Option<String> {
        self.gateway.server_addr.clone()
    }

    /// Get admin_listen
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

    /// Get probe_listen
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
    pub fn to_log_config(&self) -> crate::core::gateway::observe::SysLogConfig {
        use crate::core::gateway::observe::SysLogConfig;
        use crate::types::work_dir;

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
        assert_eq!(cfg.admin_listen_or_default(), "127.0.0.1:5810");
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
        assert_eq!(cfg.probe_listen_or_default(), "0.0.0.0:28000");
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
        let cfg: EdgionGatewayConfig = toml::from_str("").unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            crate::types::sensitive::SensitiveMode::Off
        ));
    }

    #[test]
    fn test_access_log_sensitive_replace_mode() {
        let toml_str = r#"
[access_log_sensitive]
mode = "replace"
replace_with = "{MSKED}"
"#;
        let cfg: EdgionGatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            crate::types::sensitive::SensitiveMode::Replace
        ));
        assert_eq!(cfg.access_log_sensitive.replace_with, "{MSKED}");
    }

    #[test]
    fn test_access_log_sensitive_wrap_mode() {
        let toml_str = r#"
[access_log_sensitive]
mode = "wrap"
wrap_prefix = "<E>"
wrap_suffix = "</E>"
"#;
        let cfg: EdgionGatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            cfg.access_log_sensitive.mode,
            crate::types::sensitive::SensitiveMode::Wrap
        ));
        assert_eq!(cfg.access_log_sensitive.wrap_prefix, "<E>");
    }
}
