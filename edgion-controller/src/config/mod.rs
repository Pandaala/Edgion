use crate::config::conf_center::{ConfCenterConfig, FileSystemConfig};
use anyhow::{Context, Result};
use clap::Args;
use edgion_common::conf_sync::config::{ConfSyncConfig, ConfSyncSecurityConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Configuration Center schema types (backend-agnostic), shared by the
/// controller runtime and the shared config loader below.
pub mod conf_center;

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
            edgion_resources::DEFAULT_NO_SYNC_KINDS
                .iter()
                .map(|s| s.to_string())
                .collect()
        })
}

/// Get the control-plane DNS resolver configuration (background loop).
/// Returns the default (enabled, 15s, no default resolver) when config is
/// uninitialized.
pub fn dns_resolver_config() -> DnsResolverConfig {
    CONTROLLER_CONFIG
        .get()
        .map(|c| c.dns_resolver.clone())
        .unwrap_or_default()
}

/// Get the cache capacity for a specific resource kind.
/// This reads from the global controller configuration.
/// Falls back to per-resource macro default, then global default (1000).
pub fn get_cache_capacity(kind_name: &str) -> usize {
    CONTROLLER_CONFIG
        .get()
        .map(|c| c.conf_sync.get_capacity(kind_name) as usize)
        .unwrap_or_else(|| edgion_resources::default_capacity_for_kind(kind_name).unwrap_or(1000) as usize)
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

    /// Configuration file path (YAML format)
    #[arg(
        short = 'c',
        long,
        value_name = "FILE",
        default_value = "config/edgion-controller.yaml"
    )]
    #[serde(skip)]
    pub config_file: Option<String>,

    /// Configuration directory (FileSystem mode only)
    /// CLI --conf-dir overrides conf_center.conf_dir in config file
    #[arg(long, value_name = "DIR")]
    #[serde(skip)]
    pub conf_dir: Option<PathBuf>,

    /// PollWatcher poll interval override in milliseconds (file-system mode only).
    /// Skips automatic file-count-based calculation. Min: 100 ms. For integration testing.
    #[arg(long, value_name = "MS")]
    #[serde(skip)]
    pub fs_poll_interval_ms: Option<u64>,

    /// Enable test mode for integration testing
    /// When enabled:
    /// - metrics test_key/test_type Gateway annotations are processed
    #[arg(long, help = "Enable test mode (metrics test features)")]
    #[serde(skip)]
    pub test_mode: bool,

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

    #[arg(skip)]
    #[serde(default)]
    pub metrics: MetricsConfig,

    #[command(flatten)]
    #[serde(default)]
    pub conf_sync: ConfSyncConfig,

    /// Configuration center config (FileSystem or Kubernetes)
    /// Determines which backend to use for configuration management
    #[arg(skip)]
    #[serde(default)]
    pub conf_center: ConfCenterConfig,

    /// conf_sync gRPC channel security (mTLS).
    ///
    /// REQUIRED: this section carries the transport choice for a channel that streams
    /// resolved Secrets to the data plane, so it must be made explicitly. Omitting it
    /// makes the Controller REFUSE TO START (see
    /// [`ConfSyncSecurityConfig::ensure_transport_configured`]). Configure exactly one of:
    /// `auto_ca: true` (Kubernetes), selected `certs` paths (file-mode mTLS), or
    /// `skip_tls: true` on the selected entry — the explicit opt-in for plaintext.
    #[arg(skip)]
    #[serde(default)]
    pub conf_sync_security: ConfSyncSecurityConfig,

    /// Optional federated center connection. Absent = feature disabled.
    #[arg(skip)]
    #[serde(default)]
    pub center: Option<CenterClientConfig>,

    /// Static cli tokens for role-scoped API access.
    /// Each entry names a token (by its SHA-256 hex digest) and assigns it
    /// a fine-grained RBAC policy. Hot-reloadable.
    #[arg(skip)]
    #[serde(default)]
    pub cli_tokens: Vec<CliTokenConfig>,

    /// Local admin token (Istio-style): the raw bearer token granting allow-all
    /// access to the Admin API, used by `edgion-cli` and local automation. The
    /// `EDGION_ADMIN_TOKEN` env var takes precedence over this field. Never
    /// auto-generated; a blank value is treated as "not configured".
    #[arg(skip)]
    #[serde(default)]
    pub admin_token: Option<String>,

    /// Control-plane DNS resolution for ExternalName backends (background loop).
    #[arg(skip)]
    #[serde(default)]
    pub dns_resolver: DnsResolverConfig,
}

/// Control-plane DNS resolver configuration.
///
/// Governs the background resolution loop (data-plane resolution is configured
/// separately via `EdgionGatewayConfig.spec.dnsResolver` + annotations). The
/// default resolver is either an httpdns LinkSys reference (`link_sys_ref`) OR
/// an inline standard-DNS server list (`servers`) — the two are mutually
/// exclusive (validated at load).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DnsResolverConfig {
    /// Control-plane resolution on/off.
    pub enabled: bool,
    /// Background refresh interval (duration string, e.g. "15s").
    pub refresh_interval: String,
    /// Default resolver: httpdns LinkSys reference `"namespace/name"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_sys_ref: Option<String>,
    /// Default resolver: inline standard-DNS server list (`ip[:port]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<String>,
}

impl Default for DnsResolverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            refresh_interval: "15s".to_string(),
            link_sys_ref: None,
            servers: Vec::new(),
        }
    }
}

impl DnsResolverConfig {
    /// Validate the default-resolver configuration. `link_sys_ref` and `servers`
    /// are mutually exclusive (a single default resolver, not both), and
    /// `refresh_interval` must be a parseable duration.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .link_sys_ref
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
            && !self.servers.is_empty()
        {
            return Err("dns_resolver: `link_sys_ref` and `servers` are mutually exclusive (set only one)".to_string());
        }
        // Reject an unparseable interval at load rather than silently defaulting
        // it at runtime. (A sub-second value is accepted here but clamped up to a
        // safe minimum by the loop — see `MIN_REFRESH`.)
        if crate::services::dns_resolver::service::parse_duration_str(&self.refresh_interval).is_none() {
            return Err(format!(
                "dns_resolver: refresh_interval '{}' is not a valid duration (e.g. \"15s\", \"500ms\", \"2m\")",
                self.refresh_interval
            ));
        }
        Ok(())
    }
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

    /// HTTP listen address for the dedicated liveness/readiness probe listener
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub probe_listen: Option<String>,

    /// HTTP listen address for the dedicated Prometheus metrics listener
    #[arg(long, value_name = "ADDR")]
    #[serde(default)]
    pub metrics_listen: Option<String>,

    /// CIDR allowlist for the Admin API listener. Empty/unset = allow all
    /// (backward compatible). Matched against the TCP peer address only;
    /// X-Forwarded-For is never trusted. Comma-separated on the CLI.
    #[arg(long, value_name = "CIDR", value_delimiter = ',')]
    #[serde(default)]
    pub allow_admin_ips: Vec<String>,
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

    /// Enable console output
    #[arg(long, value_name = "BOOL")]
    #[serde(default)]
    pub console: Option<bool>,

    #[arg(skip)]
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
}

/// Debug configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct DebugConfig {
    #[arg(skip)]
    #[serde(default = "default_debug_enabled")]
    pub enabled: bool,
}

/// Validation configuration. Plugin execution policy belongs to EdgionGatewayConfig.
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
#[serde(deny_unknown_fields)]
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

/// Top-level metrics configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub resource_abnormal: ResourceAbnormalMetricsConfig,
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
            resource_abnormal: ResourceAbnormalMetricsConfig::default(),
            cardinality_cap: default_cardinality_cap(),
            cardinality_mode: Default::default(),
        }
    }
}

fn default_cardinality_cap() -> usize {
    10_000
}

/// Configuration for the resource-abnormal gauge
/// (`edgion_controller_resource_abnormal`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceAbnormalMetricsConfig {
    pub enabled: bool,
    pub tick_secs: u64,
    /// Per-snapshot safety ceiling. When the rendered snapshot would
    /// exceed this value it is truncated and a `tracing::error!` is
    /// emitted. Default 1000; validated range is `[10, 100000]`.
    pub hard_cap: usize,
    /// Largest single `.status.conditions` array length at which a resource
    /// is emitted as an `edgion_controller_status_conditions_max` series (the
    /// status-write-loop / condition-pollution sentinel). Only resources at or
    /// above this bound produce a series, so healthy fleets emit nothing.
    /// Default 20; validated range is `[5, 100000]`.
    pub status_conditions_threshold: usize,
}

impl Default for ResourceAbnormalMetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_secs: 300,
            hard_cap: 1000,
            status_conditions_threshold: 20,
        }
    }
}

// Default values
fn default_grpc_listen() -> String {
    "0.0.0.0:12151".to_string()
}

fn default_admin_listen() -> String {
    // Secure default: bind the privileged admin plane to loopback. Cluster /
    // remote access must be an explicit operator choice (set 0.0.0.0 plus
    // allow_admin_ips / a NetworkPolicy). K8s deploy configs set 0.0.0.0
    // explicitly and are unaffected.
    "127.0.0.1:12101".to_string()
}

fn default_probe_listen() -> String {
    "0.0.0.0:12100".to_string()
}

fn default_metrics_listen() -> String {
    "0.0.0.0:12190".to_string()
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

fn default_console() -> bool {
    true
}

fn default_buffer_size() -> usize {
    10_000
}

fn default_debug_enabled() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_dir: None,
            log_prefix: default_log_prefix(),
            log_level: None,
            json_format: None,
            console: None,
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

impl EdgionControllerConfig {
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
            EdgionControllerConfig::default()
        };

        // Merge: CLI args override file config
        Self::merge(&mut file_config, &cli_config);

        file_config.validate_metrics()?;
        crate::api::ip_allowlist::validate_admin_ips(&file_config.server.allow_admin_ips)?;
        file_config.dns_resolver.validate().map_err(anyhow::Error::msg)?;

        Ok(file_config)
    }

    /// Validate the `metrics` section. When `resource_abnormal.enabled` is
    /// true, `tick_secs` must fall within [30, 3600] and `hard_cap` must
    /// fall within [10, 100000]. When disabled, both values are ignored.
    pub fn validate_metrics(&self) -> anyhow::Result<()> {
        let cfg = &self.metrics.resource_abnormal;
        if cfg.enabled {
            if cfg.tick_secs < 30 || cfg.tick_secs > 3600 {
                anyhow::bail!(
                    "metrics.resource_abnormal.tick_secs must be in [30, 3600], got {}",
                    cfg.tick_secs
                );
            }
            if cfg.hard_cap < 10 || cfg.hard_cap > 100_000 {
                anyhow::bail!(
                    "metrics.resource_abnormal.hard_cap must be in [10, 100000], got {}",
                    cfg.hard_cap
                );
            }
            if cfg.status_conditions_threshold < 5 || cfg.status_conditions_threshold > 100_000 {
                anyhow::bail!(
                    "metrics.resource_abnormal.status_conditions_threshold must be in [5, 100000], got {}",
                    cfg.status_conditions_threshold
                );
            }
        }
        if let ConfCenterConfig::FileSystem(fs_cfg) = &self.conf_center {
            if let Some(ms) = fs_cfg.poll_interval_ms {
                if ms < 100 {
                    anyhow::bail!("conf_center.poll_interval_ms must be >= 100 ms, got {}", ms);
                }
            }
        }
        Ok(())
    }

    /// Validate all `cli_tokens` entries.
    ///
    /// Per-entry checks:
    /// - `name` must be non-empty and match `[A-Za-z0-9_.-]+`
    /// - `name` must be unique across entries
    /// - `token_sha256` must be exactly 64 lowercase hex characters
    /// - `token_sha256` must be unique across entries (duplicate ⇒ same token reused)
    /// - `rbac` must pass `FedRbacConfig::validate()`
    ///
    /// Returns the first error encountered, or `Ok(())` when all entries pass.
    pub fn validate_cli_tokens(&self) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        let mut hashes = std::collections::HashSet::new();
        for t in &self.cli_tokens {
            if t.name.is_empty()
                || !t
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            {
                return Err(format!("cli_token name invalid: {:?}", t.name));
            }
            if !names.insert(t.name.as_str()) {
                return Err(format!("duplicate cli_token name: {}", t.name));
            }
            if t.token_sha256.len() != 64
                || !t
                    .token_sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err(format!(
                    "cli_token {} token_sha256 must be 64 lowercase hex chars",
                    t.name
                ));
            }
            if !hashes.insert(t.token_sha256.as_str()) {
                return Err(format!("duplicate token_sha256 (name {})", t.name));
            }
            t.rbac.validate()?;
        }
        Ok(())
    }

    /// Merge CLI config into file config (CLI takes precedence)
    fn merge(base: &mut Self, cli: &Self) {
        // Work directory: CLI value takes precedence if provided
        if cli.work_dir.is_some() {
            base.work_dir = cli.work_dir.clone();
        }

        // Configuration directory: CLI --conf-dir overrides conf_center.conf_dir
        // Case A: --conf-dir provided — build a new FileSystemConfig and apply poll interval override
        if let Some(conf_dir) = &cli.conf_dir {
            let mut fs_cfg = FileSystemConfig::new(conf_dir.clone());
            fs_cfg.controller_name = match &base.conf_center {
                ConfCenterConfig::FileSystem(config) => config.controller_name.clone(),
                ConfCenterConfig::Kubernetes(config) => config.controller_name.clone(),
                ConfCenterConfig::Etcd(config) => config.controller_name.clone(),
            };
            fs_cfg.poll_interval_ms = cli.fs_poll_interval_ms;
            base.conf_center = ConfCenterConfig::FileSystem(fs_cfg);
        }
        // Case B: conf_center already set from config file — still apply the CLI poll interval override
        if let Some(ms) = cli.fs_poll_interval_ms {
            if let ConfCenterConfig::FileSystem(fs_cfg) = &mut base.conf_center {
                fs_cfg.poll_interval_ms = Some(ms);
            }
        }

        // Server config
        if cli.server.grpc_listen.is_some() {
            base.server.grpc_listen = cli.server.grpc_listen.clone();
        }
        if cli.server.admin_listen.is_some() {
            base.server.admin_listen = cli.server.admin_listen.clone();
        }
        if cli.server.probe_listen.is_some() {
            base.server.probe_listen = cli.server.probe_listen.clone();
        }
        if cli.server.metrics_listen.is_some() {
            base.server.metrics_listen = cli.server.metrics_listen.clone();
        }
        // Vec field: an empty CLI value (flag absent on argv) must NOT clobber the
        // YAML allowlist — that would silently disable the allowlist (fail-open).
        if !cli.server.allow_admin_ips.is_empty() {
            base.server.allow_admin_ips = cli.server.allow_admin_ips.clone();
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
        if cli.logging.console.is_some() {
            base.logging.console = cli.logging.console;
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

    /// Get probe_listen with default fallback
    pub fn probe_listen(&self) -> String {
        self.server.probe_listen.clone().unwrap_or_else(default_probe_listen)
    }

    /// Get metrics_listen with default fallback
    pub fn metrics_listen(&self) -> String {
        self.server
            .metrics_listen
            .clone()
            .unwrap_or_else(default_metrics_listen)
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

    /// Get console flag with default fallback
    pub fn console(&self) -> bool {
        self.logging.console.unwrap_or_else(default_console)
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }

    /// Convert to SysLogConfig (for system logging)
    pub fn to_log_config(&self) -> edgion_common::observe::SysLogConfig {
        use edgion_common::observe::SysLogConfig;
        use edgion_resources::work_dir;

        // Use work_dir to resolve the log directory path
        // This ensures relative paths like "logs" are resolved to work_dir/logs
        let log_dir = work_dir().resolve(self.log_dir());

        SysLogConfig {
            log_dir,
            file_prefix: self.logging.log_prefix.clone(),
            json_format: self.json_format(),
            console: self.console(),
            level: self.log_level(),
        }
    }
}

/// A single static cli token entry: maps a sha256-hashed token to an RBAC policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CliTokenConfig {
    /// Human-readable name for audit / log fields. Must match `[A-Za-z0-9_.-]+`.
    pub name: String,
    /// SHA-256 hex digest (64 lowercase hex chars) of the raw bearer token.
    /// The actual token is never stored; only the digest is persisted.
    pub token_sha256: String,
    /// RBAC policy for this token. Absent ⇒ deny-all.
    #[serde(default)]
    pub rbac: FedRbacConfig,
}

/// Default-deny allowlist for Center (federation) operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct FedRbacConfig {
    /// Allow rules; a request is permitted iff some rule matches its (verb, kind).
    #[serde(default)]
    pub allow: Vec<FedAllowRule>,
}

/// One allow rule: the cartesian product of `verbs` x `kinds`. `*` wildcards either.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct FedAllowRule {
    #[serde(default)]
    pub verbs: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
}

impl FedRbacConfig {
    /// Reject malformed rules (a rule must list at least one verb and one kind).
    pub fn validate(&self) -> Result<(), String> {
        for (i, rule) in self.allow.iter().enumerate() {
            if rule.verbs.is_empty() {
                return Err(format!("rbac.allow[{i}]: verbs must not be empty"));
            }
            if rule.kinds.is_empty() {
                return Err(format!("rbac.allow[{i}]: kinds must not be empty"));
            }
        }
        Ok(())
    }
}

/// Configuration for connecting this controller to a federated center.
/// When absent, the federation client is not started.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterClientConfig {
    /// gRPC address of the federated center (e.g. "http://center.example.com:50052")
    pub address: String,
    /// Stable controller name; combined with `cluster` to form a stable controller_id across restarts
    pub name: String,
    /// Cluster this controller belongs to (used for center-side grouping)
    pub cluster: String,
    /// Environment tags (e.g. ["production"])
    pub env: Vec<String>,
    /// Arbitrary extension tags
    pub tag: Vec<String>,
    /// TLS config for the federation gRPC channel to Center.
    #[serde(default)]
    pub security: ConfSyncSecurityConfig,
    /// Hot-reloadable kill-switch. When false, the federation client is not run.
    #[serde(default = "default_center_enabled")]
    pub enabled: bool,
    /// Default-deny RBAC for Center operations. Absent => Center authorizes nothing.
    #[serde(default)]
    pub rbac: Option<FedRbacConfig>,
}

fn default_center_enabled() -> bool {
    true
}

impl Default for CenterClientConfig {
    fn default() -> Self {
        Self {
            address: String::new(),
            name: String::new(),
            cluster: String::new(),
            env: Vec::new(),
            tag: Vec::new(),
            security: ConfSyncSecurityConfig::default(),
            enabled: default_center_enabled(),
            rbac: None,
        }
    }
}

impl CenterClientConfig {
    /// Build the stable controller_id from cluster + name
    pub fn controller_id(&self) -> String {
        format!("{}/{}", self.cluster, self.name)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/config/mod_test.rs"]
mod tests;
