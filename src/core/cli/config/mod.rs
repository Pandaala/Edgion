use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
#[derive(Default)]
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

    /// Namespaces to watch (empty = all namespaces, single = one namespace, multiple = multi-namespace mode)
    /// Examples:
    ///   --watch-namespaces ""           # All namespaces (default)
    ///   --watch-namespaces "default"    # Single namespace
    ///   --watch-namespaces "ns1,ns2"    # Multiple namespaces
    #[arg(long = "watch-namespaces", value_name = "NS", value_delimiter = ',')]
    #[serde(default)]
    pub watch_namespaces: Vec<String>,

    /// Label selector for filtering resources (applies to all watched resources)
    /// Example: --label-selector "app.kubernetes.io/managed-by=edgion"
    #[arg(long = "label-selector", value_name = "SELECTOR")]
    #[serde(default)]
    pub label_selector: Option<String>,
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
#[derive(Default)]
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
    /// EventStore capacity for GatewayClass resources
    #[arg(skip)]
    #[serde(default = "default_small_capacity")]
    pub gateway_classes_capacity: u32,

    /// EventStore capacity for Gateway resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub gateways_capacity: u32,

    /// EventStore capacity for EdgionGatewayConfig resources
    #[arg(skip)]
    #[serde(default = "default_small_capacity")]
    pub edgion_gateway_configs_capacity: u32,

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

    /// EventStore capacity for Endpoints resources
    #[arg(skip)]
    #[serde(default = "default_capacity")]
    pub endpoints_capacity: u32,

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

fn default_small_capacity() -> u32 {
    50 // GatewayClass and EdgionGatewayConfig are typically few in number
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
            gateway_classes_capacity: default_small_capacity(),
            gateways_capacity: default_capacity(),
            edgion_gateway_configs_capacity: default_small_capacity(),
            routes_capacity: default_capacity(),
            grpc_routes_capacity: default_capacity(),
            tcp_routes_capacity: default_capacity(),
            udp_routes_capacity: default_capacity(),
            tls_routes_capacity: default_capacity(),
            link_sys_capacity: default_capacity(),
            services_capacity: default_capacity(),
            endpoint_slices_capacity: default_capacity(),
            endpoints_capacity: default_capacity(),
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
        // Work directory: CLI value takes precedence if provided
        if cli.work_dir.is_some() {
            base.work_dir = cli.work_dir.clone();
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
        if !cli.server.watch_namespaces.is_empty() {
            base.server.watch_namespaces = cli.server.watch_namespaces.clone();
        }
        if cli.server.label_selector.is_some() {
            base.server.label_selector = cli.server.label_selector.clone();
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

    /// Get watch_namespaces (empty = all namespaces)
    pub fn watch_namespaces(&self) -> &[String] {
        &self.server.watch_namespaces
    }

    /// Get label_selector
    pub fn label_selector(&self) -> Option<&str> {
        self.server.label_selector.as_deref()
    }

    /// Convert to SysLogConfig (for system logging)
    pub fn to_log_config(&self) -> crate::core::observe::SysLogConfig {
        use crate::core::observe::SysLogConfig;

        SysLogConfig {
            log_dir: PathBuf::from(self.log_dir()),
            file_prefix: self.logging.log_prefix.clone(),
            json_format: self.json_format(),
            console: true,
            level: self.log_level(),
        }
    }
}
