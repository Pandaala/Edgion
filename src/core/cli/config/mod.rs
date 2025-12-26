use crate::types::DEFAULT_PREFIX_DIR;
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Edgion Controller configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
#[serde(default)]
pub struct EdgionControllerConfig {
    /// Prefix directory for Edgion (default: /usr/local/edgion)
    #[arg(short = 'p', long, value_name = "DIR", default_value = DEFAULT_PREFIX_DIR)]
    #[serde(default = "default_prefix_dir")]
    pub prefix_dir: PathBuf,

    /// Configuration file path (TOML format)
    #[arg(
        short = 'c',
        long,
        value_name = "FILE",
        default_value = "config/edgion-controller.toml"
    )]
    #[serde(skip)]
    pub config_file: Option<String>,

    #[command(flatten)]
    #[serde(default)]
    pub server: ServerConfig,

    #[command(flatten)]
    #[serde(default)]
    pub logging: LoggingConfig,

    #[command(flatten)]
    #[serde(default)]
    pub conf: ConfConfig,

    #[command(flatten)]
    #[serde(default)]
    pub debug: DebugConfig,

    #[command(flatten)]
    #[serde(default)]
    pub conf_sync: ConfSyncConfig,

    /// K8s mode (explicit CLI override)
    #[arg(long)]
    #[serde(default)]
    pub k8s_mode: Option<bool>,
}

fn default_prefix_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PREFIX_DIR)
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ServerConfig {
    /// gRPC listen address for operator
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub grpc_listen: Option<String>,

    /// HTTP listen address for operator admin plane
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub admin_listen: Option<String>,

    /// Gateway class name that this operator instance will handle
    #[arg(long = "gateway-class", value_name = "NAME")]
    #[serde(default)]
    pub gateway_class: Option<String>,
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

/// Configuration directory settings
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ConfConfig {
    /// Configuration directory path
    #[arg(long = "conf-dir", value_name = "DIR")]
    #[serde(default)]
    pub dir: Option<String>,
}

/// Debug configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct DebugConfig {
    #[arg(skip)]
    #[serde(default = "default_debug_enabled")]
    pub enabled: bool,
}

/// Configuration synchronization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ConfSyncConfig {
    /// EventStore capacity for HTTPRoute resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub routes_capacity: u32,

    /// EventStore capacity for GRPCRoute resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub grpc_routes_capacity: u32,

    /// EventStore capacity for TCPRoute resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub tcp_routes_capacity: u32,

    /// EventStore capacity for UDPRoute resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub udp_routes_capacity: u32,

    /// EventStore capacity for TLSRoute resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub tls_routes_capacity: u32,

    /// EventStore capacity for LinkSys resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub link_sys_capacity: u32,

    /// EventStore capacity for Service resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub services_capacity: u32,

    /// EventStore capacity for EndpointSlice resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub endpoint_slices_capacity: u32,

    /// EventStore capacity for EdgionTls resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub edgion_tls_capacity: u32,

    /// EventStore capacity for EdgionPlugins resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub edgion_plugins_capacity: u32,

    /// EventStore capacity for EdgionStreamPlugins resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub edgion_stream_plugins_capacity: u32,

    /// EventStore capacity for ReferenceGrant resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub reference_grants_capacity: u32,

    /// EventStore capacity for BackendTLSPolicy resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub backend_tls_policies_capacity: u32,

    /// EventStore capacity for PluginMetadata resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub plugin_metadata_capacity: u32,

    /// EventStore capacity for Secret resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub secrets_capacity: u32,
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

// Default capacity value for EventStore
fn default_capacity() -> u32 {
    200
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            grpc_listen: None,
            admin_listen: None,
            gateway_class: None,
        }
    }
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

impl Default for ConfConfig {
    fn default() -> Self {
        Self {
            dir: None,
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
            routes_capacity: default_capacity(),
            grpc_routes_capacity: default_capacity(),
            tcp_routes_capacity: default_capacity(),
            udp_routes_capacity: default_capacity(),
            tls_routes_capacity: default_capacity(),
            link_sys_capacity: default_capacity(),
            services_capacity: default_capacity(),
            endpoint_slices_capacity: default_capacity(),
            edgion_tls_capacity: default_capacity(),
            edgion_plugins_capacity: default_capacity(),
            edgion_stream_plugins_capacity: default_capacity(),
            reference_grants_capacity: default_capacity(),
            backend_tls_policies_capacity: default_capacity(),
            plugin_metadata_capacity: default_capacity(),
            secrets_capacity: default_capacity(),
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
        // Prefix directory: only override if CLI value differs from default
        // This allows config file to set prefix_dir
        if cli.prefix_dir != PathBuf::from(DEFAULT_PREFIX_DIR) {
            base.prefix_dir = cli.prefix_dir.clone();
        }

        // Server config
        if cli.server.grpc_listen.is_some() {
            base.server.grpc_listen = cli.server.grpc_listen.clone();
        }
        if cli.server.admin_listen.is_some() {
            base.server.admin_listen = cli.server.admin_listen.clone();
        }
        if cli.server.gateway_class.is_some() {
            base.server.gateway_class = cli.server.gateway_class.clone();
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

        // Conf config
        if cli.conf.dir.is_some() {
            base.conf.dir = cli.conf.dir.clone();
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

    /// Get gateway_class
    pub fn gateway_class(&self) -> Option<String> {
        self.server.gateway_class.clone()
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

    /// Get configuration directory with default fallback
    pub fn conf_dir(&self) -> String {
        self.conf.dir.clone().unwrap_or_else(|| "examples/conf".to_string())
    }

    /// Convert to LogConfig
    pub fn to_log_config(&self) -> crate::core::observe::LogConfig {
        use crate::core::observe::LogConfig;

        LogConfig {
            log_dir: PathBuf::from(self.log_dir()),
            file_prefix: self.logging.log_prefix.clone(),
            json_format: self.json_format(),
            console: true,
            level: self.log_level(),
        }
    }
}
