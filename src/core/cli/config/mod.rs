use crate::core::conf_mgr::{ConfCenterConfig, FileSystemConfig};
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Global controller configuration for runtime access
static CONTROLLER_CONFIG: OnceLock<EdgionControllerConfig> = OnceLock::new();

/// Initialize global controller configuration (called once at startup)
pub fn init_controller_config(config: EdgionControllerConfig) {
    let _ = CONTROLLER_CONFIG.set(config);
}

/// Check if ReferenceGrant validation is enabled
/// This reads from the global controller configuration
pub fn is_reference_grant_validation_enabled() -> bool {
    CONTROLLER_CONFIG
        .get()
        .map(|c| c.validation.enable_reference_grant_validation)
        .unwrap_or(true) // Default: enabled if config not initialized
}

/// Get the list of resource kinds that should not be synced to Gateway.
/// This reads from the global controller configuration.
/// Returns DEFAULT_NO_SYNC_KINDS if config not initialized or no_sync_kinds not set.
pub fn get_no_sync_kinds() -> Vec<String> {
    CONTROLLER_CONFIG
        .get()
        .map(|c| c.conf_sync.get_no_sync_kinds().iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| {
            crate::types::DEFAULT_NO_SYNC_KINDS
                .iter()
                .map(|s| s.to_string())
                .collect()
        })
}

/// Get the cache capacity for a specific resource kind.
/// This reads from the global controller configuration.
/// Returns default capacity (1000) if config not initialized or no override set.
pub fn get_cache_capacity(kind_name: &str) -> usize {
    CONTROLLER_CONFIG
        .get()
        .map(|c| c.conf_sync.get_capacity(kind_name) as usize)
        .unwrap_or(1000) // Default if config not initialized
}

/// Edgion Controller configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
#[serde(default)]
pub struct EdgionControllerConfig {
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
        long,
        value_name = "FILE",
        default_value = "config/edgion-controller.toml"
    )]
    #[serde(skip)]
    pub config_file: Option<String>,

    /// Configuration directory (FileSystem mode only)
    /// CLI --conf-dir overrides conf_center.conf_dir in config file
    #[arg(long, value_name = "DIR")]
    #[serde(skip)]
    pub conf_dir: Option<PathBuf>,

    #[command(flatten)]
    #[serde(default)]
    pub server: ServerConfig,

    #[command(flatten)]
    #[serde(default)]
    pub logging: LoggingConfig,

    #[command(flatten)]
    #[serde(default)]
    pub debug: DebugConfig,

    #[command(flatten)]
    #[serde(default)]
    pub validation: ValidationConfig,

    #[command(flatten)]
    #[serde(default)]
    pub conf_sync: ConfSyncConfig,

    /// Configuration center config (FileSystem or Kubernetes)
    /// Determines which backend to use for configuration management
    #[arg(skip)]
    #[serde(default)]
    pub conf_center: ConfCenterConfig,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
pub struct ServerConfig {
    /// gRPC listen address for operator
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub grpc_listen: Option<String>,

    /// HTTP listen address for operator admin plane
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub admin_listen: Option<String>,
    // Note: gateway_class, watch_namespaces, label_selector moved to ConfCenterConfig::Kubernetes
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct LoggingConfig {
    /// Log directory
    #[arg(long, value_name = "DIR")]
    #[serde(default)]
    pub log_dir: Option<String>,

    #[arg(skip)]
    #[serde(default = "default_log_prefix")]
    pub log_prefix: String,

    /// Log level: trace, debug, info, warn, error
    #[arg(long, value_name = "LEVEL")]
    #[serde(default)]
    pub log_level: Option<String>,

    /// Enable JSON log format
    #[arg(long)]
    #[serde(default)]
    pub json_format: Option<bool>,

    #[arg(skip)]
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
}

// Note: ConfConfig removed, conf_dir now in ConfCenterConfig::FileSystem

/// Debug configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct DebugConfig {
    #[arg(skip)]
    #[serde(default = "default_debug_enabled")]
    pub enabled: bool,
}

/// Validation configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ValidationConfig {
    /// Enable ReferenceGrant validation for cross-namespace references
    /// When enabled, cross-namespace backend references without matching ReferenceGrant
    /// will be denied at Gateway level (ref_denied field set on BackendRef)
    /// Default: true (enabled)
    #[arg(skip)]
    #[serde(default = "default_reference_grant_validation")]
    pub enable_reference_grant_validation: bool,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            enable_reference_grant_validation: default_reference_grant_validation(),
        }
    }
}

fn default_reference_grant_validation() -> bool {
    true // Default: enabled
}

/// Configuration synchronization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ConfSyncConfig {
    /// Default EventStore capacity for all resource types
    /// This is used when no specific override is configured
    #[arg(skip)]
    #[serde(default = "default_cache_capacity")]
    pub default_capacity: u32,

    /// Override capacity for specific resource kinds
    /// Key: resource kind name (e.g., "GatewayClass", "HTTPRoute")
    /// Value: capacity for that resource type
    #[arg(skip)]
    #[serde(default)]
    pub capacity_overrides: Option<HashMap<String, u32>>,

    /// Resource kinds that should NOT be synced to Gateway.
    /// If not configured, uses DEFAULT_NO_SYNC_KINDS from types module.
    /// When configured, completely overrides the default list.
    #[arg(skip)]
    #[serde(default)]
    pub no_sync_kinds: Option<Vec<String>>,
}

impl ConfSyncConfig {
    /// Get the list of resource kinds that should not be synced to Gateway.
    /// Returns configured list if set, otherwise returns DEFAULT_NO_SYNC_KINDS.
    pub fn get_no_sync_kinds(&self) -> Vec<&str> {
        match &self.no_sync_kinds {
            Some(kinds) => kinds.iter().map(|s| s.as_str()).collect(),
            None => crate::types::DEFAULT_NO_SYNC_KINDS.to_vec(),
        }
    }

    /// Get the capacity for a specific resource kind
    ///
    /// Checks capacity_overrides first, falls back to default_capacity.
    /// The kind_name should match the ResourceKind variant name (e.g., "HTTPRoute", "GatewayClass").
    pub fn get_capacity(&self, kind_name: &str) -> u32 {
        self.capacity_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(kind_name))
            .copied()
            .unwrap_or(self.default_capacity)
    }
}

// Default values
fn default_grpc_listen() -> String {
    "0.0.0.0:50051".to_string()
}

fn default_admin_listen() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_log_dir() -> String {
    "logs".to_string()
}

fn default_log_prefix() -> String {
    "edgion-controller".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_buffer_size() -> usize {
    10_000
}

fn default_debug_enabled() -> bool {
    true
}

/// Default cache capacity for ResourceProcessor EventStore
/// This matches the DEFAULT_CACHE_CAPACITY in controllers
fn default_cache_capacity() -> u32 {
    1000
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_dir: None,
            log_prefix: default_log_prefix(),
            log_level: None,
            json_format: None,
            buffer_size: default_buffer_size(),
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: default_debug_enabled(),
        }
    }
}

impl Default for ConfSyncConfig {
    fn default() -> Self {
        Self {
            default_capacity: default_cache_capacity(),
            capacity_overrides: None,
            no_sync_kinds: None, // Use DEFAULT_NO_SYNC_KINDS
        }
    }
}

// Default implementation is now derived

impl EdgionControllerConfig {
    /// Load configuration from TOML file if config_file is specified,
    /// then merge with CLI arguments (CLI takes precedence)
    pub fn load(cli_config: Self) -> Result<Self> {
        // Load from file if specified
        let mut file_config = if let Some(ref config_path) = cli_config.config_file {
            let content =
                std::fs::read_to_string(config_path).context(format!("Failed to read config file: {}", config_path))?;

            toml::from_str::<EdgionControllerConfig>(&content)
                .context(format!("Failed to parse config file: {}", config_path))?
        } else {
            EdgionControllerConfig::default()
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

        // Configuration directory: CLI --conf-dir overrides conf_center.conf_dir
        if let Some(conf_dir) = &cli.conf_dir {
            // Preserve existing endpoint_mode if we're switching from FileSystem to FileSystem
            let endpoint_mode = base.conf_center.endpoint_mode();
            base.conf_center =
                ConfCenterConfig::FileSystem(FileSystemConfig::new(conf_dir.clone()).with_endpoint_mode(endpoint_mode));
        }

        // Server config
        if cli.server.grpc_listen.is_some() {
            base.server.grpc_listen = cli.server.grpc_listen.clone();
        }
        if cli.server.admin_listen.is_some() {
            base.server.admin_listen = cli.server.admin_listen.clone();
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

    /// Get grpc_listen with default fallback
    pub fn grpc_listen(&self) -> String {
        self.server.grpc_listen.clone().unwrap_or_else(default_grpc_listen)
    }

    /// Get admin_listen with default fallback
    pub fn admin_listen(&self) -> String {
        self.server.admin_listen.clone().unwrap_or_else(default_admin_listen)
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

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }

    /// Get the ConfCenterConfig
    pub fn conf_center_config(&self) -> &ConfCenterConfig {
        &self.conf_center
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
            console: true,
            level: self.log_level(),
        }
    }
}
