use crate::types::LogConfig;
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing;

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
        default_value = "config/edgion-gateway.toml"
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

    #[arg(skip)]
    #[serde(default = "default_ssl_log_config")]
    pub ssl_log: LogConfig,

    #[arg(skip)]
    #[serde(default)]
    pub tcp_log: LogConfig,

    #[arg(skip)]
    #[serde(default)]
    pub udp_log: LogConfig,

    #[command(flatten)]
    #[serde(default)]
    pub server: ServerConfig,

    /// RateLimiter plugin global configuration
    #[arg(skip)]
    #[serde(default)]
    pub rate_limiter: RateLimiterGlobalConfig,
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
}

// Default log configurations
fn default_access_log_config() -> LogConfig {
    LogConfig::enabled_default("logs/edgion_access.log")
}

fn default_ssl_log_config() -> LogConfig {
    LogConfig::enabled_default("logs/ssl.log")
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
// RateLimiter Global Configuration
// ============================================

/// Unit multiplier: 1K = 1024 slots
pub const SLOTS_K: usize = 1024;

/// Minimum CMS estimator slots in K (1K = 1024 slots, ~64KB)
pub const MIN_ESTIMATOR_SLOTS_K: usize = 1;

/// Default CMS estimator slots in K (64K = 65536 slots, ~4MB)
pub const DEFAULT_ESTIMATOR_SLOTS_K: usize = 64;

/// Maximum CMS estimator slots in K (1024K = 1M slots, ~64MB)
pub const DEFAULT_MAX_ESTIMATOR_SLOTS_K: usize = 1024;

/// RateLimiter plugin global configuration
///
/// Controls the Count-Min Sketch (CMS) estimator settings for all RateLimiter plugins.
/// The CMS algorithm provides memory-efficient rate limiting with configurable precision.
///
/// All slot values are in K units (1K = 1024 slots).
/// Memory per Rate instance ≈ slots_k × 64KB
///
/// ## TOML Configuration Example:
/// ```toml
/// [rate_limiter]
/// default_estimator_slots_k = 64     # 64K slots = ~4MB
/// max_estimator_slots_k = 1024       # 1024K slots = ~64MB
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimiterGlobalConfig {
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
    /// Limits the maximum slots that can be configured per RateLimiter plugin.
    /// Maximum memory per Rate instance ≈ max_slots_k × 64KB
    #[serde(default = "default_max_estimator_slots_k")]
    pub max_estimator_slots_k: usize,
}

fn default_estimator_slots_k() -> usize {
    DEFAULT_ESTIMATOR_SLOTS_K
}

fn default_max_estimator_slots_k() -> usize {
    DEFAULT_MAX_ESTIMATOR_SLOTS_K
}

impl Default for RateLimiterGlobalConfig {
    fn default() -> Self {
        Self {
            default_estimator_slots_k: DEFAULT_ESTIMATOR_SLOTS_K,
            max_estimator_slots_k: DEFAULT_MAX_ESTIMATOR_SLOTS_K,
        }
    }
}

// ============================================
// Global RateLimiter Config Store
// ============================================

use std::sync::{LazyLock, RwLock};

/// Global store for RateLimiter configuration
static RATE_LIMITER_GLOBAL_CONFIG: LazyLock<RwLock<RateLimiterGlobalConfig>> =
    LazyLock::new(|| RwLock::new(RateLimiterGlobalConfig::default()));

/// Initialize the global RateLimiter configuration
///
/// This should be called once during application startup with the loaded config.
pub fn init_rate_limiter_global_config(config: &RateLimiterGlobalConfig) {
    if let Ok(mut global) = RATE_LIMITER_GLOBAL_CONFIG.write() {
        *global = config.clone();
        tracing::info!(
            default_slots_k = config.default_estimator_slots_k,
            max_slots_k = config.max_estimator_slots_k,
            default_slots = config.default_estimator_slots_k * SLOTS_K,
            max_slots = config.max_estimator_slots_k * SLOTS_K,
            "RateLimiter global config initialized"
        );
    }
}

/// Get the default estimator slots from global config (returns actual slot count)
pub fn get_default_estimator_slots() -> usize {
    RATE_LIMITER_GLOBAL_CONFIG
        .read()
        .map(|c| c.default_estimator_slots_k * SLOTS_K)
        .unwrap_or(DEFAULT_ESTIMATOR_SLOTS_K * SLOTS_K)
}

/// Get the maximum estimator slots from global config (returns actual slot count)
pub fn get_max_estimator_slots() -> usize {
    RATE_LIMITER_GLOBAL_CONFIG
        .read()
        .map(|c| c.max_estimator_slots_k * SLOTS_K)
        .unwrap_or(DEFAULT_MAX_ESTIMATOR_SLOTS_K * SLOTS_K)
}

/// Get the minimum estimator slots (returns actual slot count)
pub fn get_min_estimator_slots() -> usize {
    MIN_ESTIMATOR_SLOTS_K * SLOTS_K
}

impl EdgionGatewayConfig {
    /// Load configuration from TOML file if config_file is specified,
    /// then merge with CLI arguments (CLI takes precedence)
    pub fn load(cli_config: Self) -> Result<Self> {
        // Load from file if specified
        let mut file_config = if let Some(ref config_path) = cli_config.config_file {
            let content =
                std::fs::read_to_string(config_path).context(format!("Failed to read config file: {}", config_path))?;

            toml::from_str::<EdgionGatewayConfig>(&content)
                .context(format!("Failed to parse config file: {}", config_path))?
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
    pub fn to_log_config(&self) -> crate::core::observe::SysLogConfig {
        use crate::core::observe::SysLogConfig;
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
