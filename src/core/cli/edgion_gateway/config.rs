use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::types::DEFAULT_PREFIX_DIR;

/// Edgion Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args, Default)]
#[serde(default)]
pub struct EdgionGatewayConfig {
    /// Prefix directory for Edgion (default: /usr/local/edgion)
    #[arg(short = 'p', long, value_name = "DIR", default_value = DEFAULT_PREFIX_DIR)]
    #[serde(default = "default_prefix_dir")]
    pub prefix_dir: PathBuf,

    /// Configuration file path (TOML format)
    #[arg(short = 'c', long = "config-file", value_name = "FILE")]
    #[serde(skip)]
    pub config_file: Option<String>,

    #[command(flatten)]
    #[serde(default)]
    pub gateway: GatewayConfig,

    #[command(flatten)]
    #[serde(default)]
    pub logging: LoggingConfig,
}

fn default_prefix_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PREFIX_DIR)
}

/// Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct GatewayConfig {
    /// Gateway class name
    #[arg(long = "gateway-class", value_name = "NAME")]
    #[serde(default)]
    pub gateway_class: Option<String>,

    /// Operator gRPC address (e.g., http://127.0.0.1:50051)
    #[arg(long = "server-addr", value_name = "ADDR")]
    #[serde(default)]
    pub server_addr: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long = "admin-listen", value_name = "ADDR")]
    #[serde(default)]
    pub admin_listen: Option<String>,
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

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            gateway_class: None,
            server_addr: None,
            admin_listen: None,
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
            console: default_console(),
            buffer_size: default_buffer_size(),
        }
    }
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
        // Prefix directory: only override if CLI value differs from default
        // This allows config file to set prefix_dir
        if cli.prefix_dir != PathBuf::from(DEFAULT_PREFIX_DIR) {
            base.prefix_dir = cli.prefix_dir.clone();
        }
        
        // Gateway config
        if cli.gateway.gateway_class.is_some() {
            base.gateway.gateway_class = cli.gateway.gateway_class.clone();
        }
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
    }

    /// Get gateway_class
    pub fn gateway_class(&self) -> Option<String> {
        self.gateway.gateway_class.clone()
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

    /// Convert to LogConfig
    pub fn to_log_config(&self) -> crate::core::observe::LogConfig {
        use crate::core::observe::LogConfig;

        LogConfig {
            log_dir: PathBuf::from(self.log_dir()),
            file_prefix: self.logging.log_prefix.clone(),
            json_format: self.json_format(),
            console: self.logging.console,
            level: self.log_level(),
        }
    }
}

