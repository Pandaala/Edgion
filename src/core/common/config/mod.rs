use crate::core::common::auth::AdminAuthConfig;
use crate::core::controller::conf_mgr::{ConfCenterConfig, FileSystemConfig};
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Global controller configuration for runtime access
static CONTROLLER_CONFIG: OnceLock<EdgionControllerConfig> = OnceLock::new();

/// Global test mode flag (Controller)
/// When enabled:
/// - metrics test_key/test_type annotations are processed
static GLOBAL_TEST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Global integration testing mode flag (Gateway)
/// When enabled via --integration-testing-mode:
/// - Access Log Store: stores complete access logs in memory, queryable via Admin API
/// - Metrics Test Data: collects test data labels on backend_requests_total metric
///   This flag MUST NEVER be used in production.
static GLOBAL_INTEGRATION_TESTING_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Initialize global test mode (called once at startup)
pub fn init_global_test_mode(enabled: bool) {
    GLOBAL_TEST_MODE.store(enabled, std::sync::atomic::Ordering::SeqCst);
    if enabled {
        tracing::info!(
            component = "test_mode",
            "Test mode enabled: metrics test features active"
        );
    }
}

/// Check if test mode is enabled
pub fn is_test_mode() -> bool {
    GLOBAL_TEST_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Initialize global integration testing mode (called once at gateway startup)
pub fn init_integration_testing_mode(enabled: bool) {
    GLOBAL_INTEGRATION_TESTING_MODE.store(enabled, std::sync::atomic::Ordering::SeqCst);
    if enabled {
        tracing::warn!(
            component = "integration_testing",
            "Integration testing mode enabled: Access Log Store + Metrics Test Data active. \
             DO NOT enable in production — `edgion_backend_requests_total` will carry \
             per-request `test_data` labels (e.g., backend IP, latency_ms) when Gateway \
             annotations opt in, causing Prometheus cardinality to scale with request count."
        );
    }
}

/// Check if integration testing mode is enabled
#[inline]
pub fn is_integration_testing_mode() -> bool {
    GLOBAL_INTEGRATION_TESTING_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

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
    /// Omit this section to keep plaintext gRPC (backward compatible).
    #[arg(skip)]
    #[serde(default)]
    pub conf_sync_security: ConfSyncSecurityConfig,

    /// Optional federated center connection. Absent = feature disabled.
    #[arg(skip)]
    #[serde(default)]
    pub center: Option<CenterClientConfig>,

    /// Optional Admin API authentication.
    /// Omit to disable auth (backward compatible). When present, Admin API
    /// endpoints (except health/ready) require a valid JWT Bearer token.
    #[arg(skip)]
    #[serde(default)]
    pub auth: Option<AdminAuthConfig>,

    /// Optional local authentication (built-in username/password).
    /// Credentials come from this section or the `EDGION_ADMIN_*` env vars;
    /// they are never auto-generated. When nothing valid is configured here,
    /// via `EDGION_ADMIN_*`, or via `[auth]` OIDC, the Admin API fails closed
    /// (503) until valid credentials are supplied.
    #[arg(skip)]
    #[serde(default)]
    pub local_auth: Option<crate::core::common::local_auth::LocalAuthConfig>,

    /// Static cli tokens for role-scoped API access.
    /// Each entry names a token (by its SHA-256 hex digest) and assigns it
    /// a fine-grained RBAC policy. Hot-reloadable.
    #[arg(skip)]
    #[serde(default)]
    pub cli_tokens: Vec<CliTokenConfig>,
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

    /// Enable console output
    #[arg(long, value_name = "BOOL")]
    #[serde(default)]
    pub console: Option<bool>,

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

/// Top-level metrics configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MetricsConfig {
    pub resource_abnormal: ResourceAbnormalMetricsConfig,
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
}

impl Default for ResourceAbnormalMetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_secs: 300,
            hard_cap: 1000,
        }
    }
}

/// Configuration synchronization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Args)]
pub struct ConfSyncConfig {
    /// Default EventStore capacity for all resource types
    /// This is used for all resource types unless a specific override is configured
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
            None => edgion_resources::DEFAULT_NO_SYNC_KINDS.to_vec(),
        }
    }

    /// Get the capacity for a specific resource kind.
    ///
    /// Resolution order:
    /// 1. `capacity_overrides` map (explicit per-resource override from config)
    /// 2. `default_capacity` (global default, default 200)
    pub fn get_capacity(&self, kind_name: &str) -> u32 {
        if let Some(v) = self
            .capacity_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(kind_name))
        {
            return *v;
        }
        self.default_capacity
    }
}

/// conf_sync gRPC channel security configuration.
/// The entire section is optional; omitting it = no security enhancement (backward compatible).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConfSyncSecurityConfig {
    /// Name of the active certificate set. Must match `name` of one entry in `certs`.
    pub active: Option<String>,
    /// Certificate sets (YAML `conf_sync_security.certs` list).
    pub certs: Vec<ConfSyncTlsConfig>,
    /// Legacy single-cert format [conf_sync_security.tls] — kept for backward compat.
    pub tls: Option<ConfSyncTlsConfig>,
    // Reserved: token authentication can be added here later
}

impl ConfSyncSecurityConfig {
    /// Resolve the active TLS config.
    ///
    /// Priority:
    /// 1. `certs` array: find entry where `name == active`
    /// 2. `certs` has exactly one entry and `active` is None: use it
    /// 3. Fallback to legacy `tls` field
    /// 4. `None` = plaintext gRPC
    pub fn resolve_active_tls(&self) -> Option<&ConfSyncTlsConfig> {
        if !self.certs.is_empty() {
            if let Some(active) = &self.active {
                return self.certs.iter().find(|c| c.name.as_deref() == Some(active.as_str()));
            }
            if self.certs.len() == 1 {
                return self.certs.first();
            }
            return None;
        }
        self.tls.as_ref()
    }

    /// Federation requires real mTLS. Returns the active cert set only when one
    /// is configured AND `skip_tls` is not set; otherwise refuses with a reason.
    /// Plaintext is never representable through this path.
    ///
    /// Precondition: assumes `validate()` has already passed — it does NOT
    /// re-check cert path emptiness. conf_sync does NOT use this; it keeps
    /// `resolve_active_tls()` + `skip_tls`.
    pub fn resolve_mtls_or_refuse(&self) -> Result<&ConfSyncTlsConfig, MtlsRefusal> {
        match self.resolve_active_tls() {
            None => Err(MtlsRefusal::NoTls),
            Some(t) if t.skip_tls => Err(MtlsRefusal::SkipTls),
            Some(t) => Ok(t),
        }
    }

    /// Validate configuration at startup.
    ///
    /// Returns error for:
    /// - duplicate names in `certs`
    /// - multiple certs without `active`
    /// - `active` pointing to nonexistent name
    /// - cert path fields empty on active cert (unless `skip_tls`)
    pub fn validate(&self) -> anyhow::Result<()> {
        // Check duplicate names
        let mut seen = std::collections::HashSet::new();
        for cert in &self.certs {
            if let Some(name) = &cert.name {
                if !seen.insert(name.as_str()) {
                    anyhow::bail!("conf_sync_security: duplicate cert name '{}'", name);
                }
            }
        }
        // Multiple certs require active selector
        if self.certs.len() > 1 && self.active.is_none() {
            anyhow::bail!("conf_sync_security: multiple certs configured but no 'active' selector");
        }
        // active must match an existing name
        if let Some(active) = &self.active {
            if !self.certs.is_empty() {
                let found = self.certs.iter().any(|c| c.name.as_deref() == Some(active.as_str()));
                if !found {
                    anyhow::bail!("conf_sync_security: active='{}' does not match any cert name", active);
                }
            }
        }
        // Validate the active cert's paths (skip if skip_tls)
        if let Some(tls) = self.resolve_active_tls() {
            if !tls.skip_tls {
                tls.validate()?;
            }
        }
        Ok(())
    }
}

/// mTLS certificate file paths for conf_sync gRPC channel.
#[derive(Clone, Serialize, Deserialize)]
pub struct ConfSyncTlsConfig {
    /// Unique name for this cert set. Required when using [[certs]] array format.
    #[serde(default)]
    pub name: Option<String>,
    /// Certificate file path (relative to work_dir or absolute)
    pub cert: String,
    /// Private key file path
    pub key: String,
    /// CA certificate path (used to verify the peer)
    pub ca: String,
    /// Emergency bypass: skip TLS and start in plaintext with ERROR log.
    /// Only for emergency use during cert rotation failures. Must not stay enabled.
    #[serde(default)]
    pub skip_tls: bool,
}

impl std::fmt::Debug for ConfSyncTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfSyncTlsConfig")
            .field("name", &self.name)
            .field("cert", &self.cert)
            .field("key", &"***")
            .field("ca", &self.ca)
            .field("skip_tls", &self.skip_tls)
            .finish()
    }
}

impl ConfSyncTlsConfig {
    /// Validate that all required paths are non-empty.
    /// Called before attempting to load certificates.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cert.trim().is_empty() {
            anyhow::bail!("conf_sync_security.tls.cert must not be empty");
        }
        if self.key.trim().is_empty() {
            anyhow::bail!("conf_sync_security.tls.key must not be empty");
        }
        if self.ca.trim().is_empty() {
            anyhow::bail!("conf_sync_security.tls.ca must not be empty");
        }
        Ok(())
    }

    /// Resolve a path: absolute paths used as-is, relative paths joined with work_dir.
    fn resolve_path(path: &str) -> PathBuf {
        edgion_resources::work_dir().resolve(path)
    }

    pub fn cert_path(&self) -> PathBuf {
        Self::resolve_path(&self.cert)
    }

    pub fn key_path(&self) -> PathBuf {
        Self::resolve_path(&self.key)
    }

    pub fn ca_path(&self) -> PathBuf {
        Self::resolve_path(&self.ca)
    }
}

/// Server-side TLS certificate file paths for the admin HTTP API.
///
/// Unlike `ConfSyncTlsConfig` (mTLS — requires `ca`), this is server-only TLS:
/// only `cert` + `key` are needed, with no client-certificate verification. Lives
/// in `common` so the Controller can reuse it later (see admin-api-03).
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminTlsConfig {
    /// PEM server certificate path (relative to work_dir or absolute).
    pub cert: String,
    /// PEM private key path (relative to work_dir or absolute).
    pub key: String,
}

impl std::fmt::Debug for AdminTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminTlsConfig")
            .field("cert", &self.cert)
            .field("key", &"***")
            .finish()
    }
}

impl AdminTlsConfig {
    /// Validate that the cert and key paths are non-empty.
    /// Called before attempting to load certificates at startup.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cert.trim().is_empty() {
            anyhow::bail!("admin_tls.cert must not be empty");
        }
        if self.key.trim().is_empty() {
            anyhow::bail!("admin_tls.key must not be empty");
        }
        Ok(())
    }

    /// Resolve a path: absolute paths used as-is, relative paths joined with work_dir.
    fn resolve_path(path: &str) -> PathBuf {
        edgion_resources::work_dir().resolve(path)
    }

    /// Resolve the cert path: absolute as-is, relative joined with work_dir.
    pub fn cert_path(&self) -> PathBuf {
        Self::resolve_path(&self.cert)
    }

    /// Resolve the key path: absolute as-is, relative joined with work_dir.
    pub fn key_path(&self) -> PathBuf {
        Self::resolve_path(&self.key)
    }
}

/// Why a federation channel refused the configured security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtlsRefusal {
    /// resolve_active_tls() == None — no TLS block configured.
    NoTls,
    /// The active cert set has skip_tls = true.
    SkipTls,
}

// Default values
fn default_grpc_listen() -> String {
    "0.0.0.0:12151".to_string()
}

fn default_admin_listen() -> String {
    "0.0.0.0:12101".to_string()
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

/// Default cache capacity for ResourceProcessor EventStore
fn default_cache_capacity() -> u32 {
    200
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
        crate::core::common::api::ip_allowlist::validate_admin_ips(&file_config.server.allow_admin_ips)?;

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

    /// Get the ConfCenterConfig
    pub fn conf_center_config(&self) -> &ConfCenterConfig {
        &self.conf_center
    }

    /// Convert to SysLogConfig (for system logging)
    pub fn to_log_config(&self) -> crate::core::gateway::observe::SysLogConfig {
        use crate::core::gateway::observe::SysLogConfig;
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
    /// Heartbeat interval in seconds (default: 30)
    #[serde(default = "default_ping_interval_secs")]
    pub ping_interval_secs: u64,
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

fn default_ping_interval_secs() -> u64 {
    30
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
            ping_interval_secs: default_ping_interval_secs(),
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
mod tests {
    use super::*;

    // ── Helper ───────────────────────────────────────────────────────────────

    fn make_tls(name: Option<&str>, cert: &str) -> ConfSyncTlsConfig {
        ConfSyncTlsConfig {
            name: name.map(|s| s.to_string()),
            cert: cert.to_string(),
            key: "k".to_string(),
            ca: "ca".to_string(),
            skip_tls: false,
        }
    }

    // ── Group A: get_capacity (3 → 1 table-driven) ──────────────────────────

    #[test]
    fn test_get_capacity_table_driven() {
        struct Case {
            name: &'static str,
            default_capacity: u32,
            override_entry: Option<(&'static str, u32)>, // (kind, cap) to insert
            query_kind: &'static str,
            expected: u32,
        }

        let cases = [
            Case {
                name: "override wins for known kind",
                default_capacity: 200,
                override_entry: Some(("HTTPRoute", 999)),
                query_kind: "HTTPRoute",
                expected: 999,
            },
            Case {
                name: "falls back to default when no overrides",
                default_capacity: 200,
                override_entry: None,
                query_kind: "GatewayClass",
                expected: 200,
            },
            Case {
                name: "unknown kind falls back to global default",
                default_capacity: 200,
                override_entry: None,
                query_kind: "SomeUnknownKind",
                expected: 200,
            },
        ];

        for case in &cases {
            let capacity_overrides = case.override_entry.map(|(kind, cap)| {
                let mut m = HashMap::new();
                m.insert(kind.to_string(), cap);
                m
            });
            let config = ConfSyncConfig {
                default_capacity: case.default_capacity,
                capacity_overrides,
                no_sync_kinds: None,
            };
            assert_eq!(
                config.get_capacity(case.query_kind),
                case.expected,
                "case: {}",
                case.name
            );
        }
    }

    // ── Group B: independent tests (4, kept as-is) ──────────────────────────

    #[test]
    fn test_conf_sync_default() {
        let config = ConfSyncConfig::default();
        assert_eq!(config.default_capacity, 200);
        assert!(config.capacity_overrides.is_none());
    }

    #[test]
    fn test_to_log_config_respects_console_flag() {
        let mut cfg = EdgionControllerConfig::default();
        cfg.logging.console = Some(false);
        let log_cfg = cfg.to_log_config();
        assert!(!log_cfg.console);
    }

    #[test]
    fn merge_keeps_yaml_allowlist_when_cli_omits_flag() {
        // Simulates the K8s case: allowlist from YAML, flag absent on argv (clap → empty Vec).
        let mut base = EdgionControllerConfig::default();
        base.server.allow_admin_ips = vec!["10.0.0.0/8".to_string()];
        let cli = EdgionControllerConfig::default(); // allow_admin_ips empty
        EdgionControllerConfig::merge(&mut base, &cli);
        assert_eq!(
            base.server.allow_admin_ips,
            vec!["10.0.0.0/8".to_string()],
            "an empty CLI allowlist must not clobber the YAML allowlist"
        );
    }

    #[test]
    fn merge_cli_allowlist_overrides_when_present() {
        let mut base = EdgionControllerConfig::default();
        base.server.allow_admin_ips = vec!["10.0.0.0/8".to_string()];
        let mut cli = EdgionControllerConfig::default();
        cli.server.allow_admin_ips = vec!["192.168.0.0/16".to_string()];
        EdgionControllerConfig::merge(&mut base, &cli);
        assert_eq!(base.server.allow_admin_ips, vec!["192.168.0.0/16".to_string()]);
    }

    #[test]
    fn test_center_client_config_parses_from_yaml() {
        let yaml = r#"
center:
  address: "http://center.example.com:50052"
  name: "ctrl-01"
  cluster: "prod-cn"
  env: [production]
  tag: [team-infra]
"#;
        let config: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
        let center = config.center.unwrap();
        assert_eq!(center.address, "http://center.example.com:50052");
        assert_eq!(center.name, "ctrl-01");
        assert_eq!(center.cluster, "prod-cn");
        assert_eq!(center.env, vec!["production"]);
        assert_eq!(center.ping_interval_secs, 30); // default
    }

    // ── Group C: resolve_active_tls (5 → 1 table-driven) ───────────────────

    #[test]
    fn test_resolve_active_tls_scenarios() {
        struct Case {
            name: &'static str,
            cfg: ConfSyncSecurityConfig,
            expected_cert: Option<&'static str>,
        }

        let cases = [
            Case {
                name: "legacy tls field falls back",
                cfg: ConfSyncSecurityConfig {
                    tls: Some(make_tls(None, "legacy.crt")),
                    ..Default::default()
                },
                expected_cert: Some("legacy.crt"),
            },
            Case {
                name: "single cert entry with no active selector",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("primary"), "primary.crt")],
                    ..Default::default()
                },
                expected_cert: Some("primary.crt"),
            },
            Case {
                name: "multi certs with matching active selector",
                cfg: ConfSyncSecurityConfig {
                    active: Some("new".to_string()),
                    certs: vec![make_tls(Some("old"), "old.crt"), make_tls(Some("new"), "new.crt")],
                    ..Default::default()
                },
                expected_cert: Some("new.crt"),
            },
            Case {
                name: "multi certs without active selector returns None",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                    ..Default::default()
                },
                expected_cert: None,
            },
            Case {
                name: "empty config returns None",
                cfg: ConfSyncSecurityConfig::default(),
                expected_cert: None,
            },
        ];

        for case in &cases {
            assert_eq!(
                case.cfg.resolve_active_tls().map(|t| t.cert.as_str()),
                case.expected_cert,
                "case: {}",
                case.name
            );
        }
    }

    // ── Group D: validate_active_tls (5 → 1 table-driven) ──────────────────

    #[test]
    fn test_validate_active_tls_scenarios() {
        enum Expected {
            Ok,
            Err(&'static str),
        }

        struct Case {
            name: &'static str,
            cfg: ConfSyncSecurityConfig,
            expected: Expected,
        }

        let cases = [
            Case {
                name: "empty config is valid",
                cfg: ConfSyncSecurityConfig::default(),
                expected: Expected::Ok,
            },
            Case {
                name: "single entry with skip_tls is valid",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![ConfSyncTlsConfig {
                        name: Some("p".to_string()),
                        cert: String::new(),
                        key: String::new(),
                        ca: String::new(),
                        skip_tls: true,
                    }],
                    ..Default::default()
                },
                expected: Expected::Ok,
            },
            Case {
                name: "duplicate cert names errors",
                cfg: ConfSyncSecurityConfig {
                    active: Some("dup".to_string()),
                    certs: vec![make_tls(Some("dup"), "a.crt"), make_tls(Some("dup"), "b.crt")],
                    ..Default::default()
                },
                expected: Expected::Err(""),
            },
            Case {
                name: "multi certs without active selector errors",
                cfg: ConfSyncSecurityConfig {
                    certs: vec![make_tls(Some("a"), "a.crt"), make_tls(Some("b"), "b.crt")],
                    ..Default::default()
                },
                expected: Expected::Err("no 'active' selector"),
            },
            Case {
                name: "active selector not found in certs errors",
                cfg: ConfSyncSecurityConfig {
                    active: Some("missing".to_string()),
                    certs: vec![make_tls(Some("primary"), "primary.crt")],
                    ..Default::default()
                },
                expected: Expected::Err("does not match any cert name"),
            },
        ];

        for case in &cases {
            let result = case.cfg.validate();
            match &case.expected {
                Expected::Ok => {
                    assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
                }
                Expected::Err(substr) => {
                    assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                    if !substr.is_empty() {
                        let msg = format!("{:?}", result.unwrap_err());
                        assert!(
                            msg.contains(substr),
                            "case '{}': error message {:?} does not contain {:?}",
                            case.name,
                            msg,
                            substr
                        );
                    }
                }
            }
        }
    }

    // ── Group E: metrics_resource_abnormal defaults (2 independent, kept) ───

    #[test]
    fn test_metrics_resource_abnormal_defaults() {
        let yaml = "work_dir: /tmp\n";
        let cfg: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.metrics.resource_abnormal.enabled);
        assert_eq!(cfg.metrics.resource_abnormal.tick_secs, 300);
    }

    #[test]
    fn test_metrics_resource_abnormal_explicit_values_parse() {
        let yaml = r#"
metrics:
  resource_abnormal:
    enabled: false
    tick_secs: 60
"#;
        let cfg: EdgionControllerConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.metrics.resource_abnormal.enabled);
        assert_eq!(cfg.metrics.resource_abnormal.tick_secs, 60);
    }

    // ── Group E cont.: tick validation (4 → 1 table-driven) ─────────────────

    #[test]
    fn test_metrics_resource_abnormal_tick_scenarios() {
        struct Case {
            name: &'static str,
            enabled: bool,
            tick_secs: u64,
            expect_err_substr: Option<&'static str>,
        }

        let cases = [
            Case {
                name: "tick 29 is too small",
                enabled: true,
                tick_secs: 29,
                expect_err_substr: Some("tick_secs"),
            },
            Case {
                name: "tick 30 is the minimum and is accepted",
                enabled: true,
                tick_secs: 30,
                expect_err_substr: None,
            },
            Case {
                name: "tick 3601 is too large",
                enabled: true,
                tick_secs: 3601,
                expect_err_substr: Some("tick_secs"),
            },
            Case {
                name: "disabled skips range check even with tick 0",
                enabled: false,
                tick_secs: 0,
                expect_err_substr: None,
            },
        ];

        for case in &cases {
            let mut cfg = EdgionControllerConfig::default();
            cfg.metrics.resource_abnormal.enabled = case.enabled;
            cfg.metrics.resource_abnormal.tick_secs = case.tick_secs;
            let result = cfg.validate_metrics();
            match case.expect_err_substr {
                None => {
                    assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
                }
                Some(substr) => {
                    assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                    let msg = format!("{:?}", result.unwrap_err());
                    assert!(
                        msg.contains(substr),
                        "case '{}': error {:?} does not contain {:?}",
                        case.name,
                        msg,
                        substr
                    );
                }
            }
        }
    }

    // ── Group G: resolve_mtls_or_refuse ─────────────────────────────────────

    #[test]
    fn resolve_mtls_or_refuse_no_tls() {
        let sec = ConfSyncSecurityConfig::default();
        assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::NoTls);
    }

    #[test]
    fn resolve_mtls_or_refuse_skip_tls() {
        let sec = ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: true,
            }),
            ..Default::default()
        };
        assert_eq!(sec.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::SkipTls);
    }

    #[test]
    fn resolve_mtls_or_refuse_ok_for_mtls() {
        let sec = ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: "c".into(),
                key: "k".into(),
                ca: "a".into(),
                skip_tls: false,
            }),
            ..Default::default()
        };
        assert!(sec.resolve_mtls_or_refuse().is_ok());
    }

    // ── Group F: hard_cap validation (3 → 1 table-driven) ───────────────────

    #[test]
    fn test_metrics_resource_abnormal_hard_cap_scenarios() {
        struct Case {
            name: &'static str,
            hard_cap: usize,
            expect_err_substr: Option<&'static str>,
        }

        let cases = [
            Case {
                name: "default hard_cap is 1000 and is accepted",
                hard_cap: 1000,
                expect_err_substr: None,
            },
            Case {
                name: "hard_cap 9 is too small",
                hard_cap: 9,
                expect_err_substr: Some("hard_cap"),
            },
            Case {
                name: "hard_cap 100_001 is too large",
                hard_cap: 100_001,
                expect_err_substr: Some("hard_cap"),
            },
        ];

        for case in &cases {
            let mut cfg = EdgionControllerConfig::default();
            cfg.metrics.resource_abnormal.hard_cap = case.hard_cap;
            let result = cfg.validate_metrics();
            match case.expect_err_substr {
                None => {
                    assert!(result.is_ok(), "case '{}' expected Ok, got {:?}", case.name, result);
                }
                Some(substr) => {
                    assert!(result.is_err(), "case '{}' expected Err, got Ok", case.name);
                    let msg = format!("{:?}", result.unwrap_err());
                    assert!(
                        msg.contains(substr),
                        "case '{}': error {:?} does not contain {:?}",
                        case.name,
                        msg,
                        substr
                    );
                }
            }
        }
    }

    // ── Group I: cli_tokens parse + validate ─────────────────────────────────

    #[test]
    fn cli_tokens_parse_and_validate() {
        let cfg: EdgionControllerConfig = serde_yaml::from_str(
            "cli_tokens:\n  - name: bot\n    token_sha256: \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n    rbac:\n      allow:\n        - verbs: [get, list]\n          kinds: [PluginMetaData]\n",
        )
        .unwrap();
        assert_eq!(cfg.cli_tokens.len(), 1);
        assert_eq!(cfg.cli_tokens[0].name, "bot");
        assert!(cfg.validate_cli_tokens().is_ok());
    }

    #[test]
    fn cli_tokens_validate_rejects_bad_hash_dup_name_dup_hash() {
        let bad_hex = CliTokenConfig {
            name: "a".into(),
            token_sha256: "xyz".into(),
            rbac: FedRbacConfig::default(),
        };
        assert!(EdgionControllerConfig {
            cli_tokens: vec![bad_hex],
            ..Default::default()
        }
        .validate_cli_tokens()
        .is_err());

        let h = "0".repeat(64);
        let dup_name = vec![
            CliTokenConfig {
                name: "a".into(),
                token_sha256: h.clone(),
                rbac: FedRbacConfig::default(),
            },
            CliTokenConfig {
                name: "a".into(),
                token_sha256: "1".repeat(64),
                rbac: FedRbacConfig::default(),
            },
        ];
        assert!(EdgionControllerConfig {
            cli_tokens: dup_name,
            ..Default::default()
        }
        .validate_cli_tokens()
        .is_err());

        let dup_hash = vec![
            CliTokenConfig {
                name: "a".into(),
                token_sha256: h.clone(),
                rbac: FedRbacConfig::default(),
            },
            CliTokenConfig {
                name: "b".into(),
                token_sha256: h,
                rbac: FedRbacConfig::default(),
            },
        ];
        assert!(EdgionControllerConfig {
            cli_tokens: dup_hash,
            ..Default::default()
        }
        .validate_cli_tokens()
        .is_err());
    }

    // ── Group H: CenterClientConfig enabled + rbac ───────────────────────────

    #[test]
    fn center_enabled_defaults_to_true() {
        let cfg: CenterClientConfig =
            serde_yaml::from_str("address: \"https://c:12251\"\nname: n\ncluster: c\n").unwrap();
        assert!(cfg.enabled, "enabled must default to true (backward compatible)");
        assert!(cfg.rbac.is_none(), "absent rbac => None => deny-all downstream");
    }

    #[test]
    fn center_rbac_parses_allow_rules() {
        let cfg: CenterClientConfig = serde_yaml::from_str(
            r#"
address: a
name: n
cluster: c
enabled: false
rbac:
  allow:
    - verbs: [get, list]
      kinds: [PluginMetaData]
    - verbs: [reload]
      kinds: ["*"]
"#,
        )
        .unwrap();
        assert!(!cfg.enabled);
        let rbac = cfg.rbac.expect("rbac present");
        assert_eq!(rbac.allow.len(), 2);
        assert_eq!(rbac.allow[0].verbs, vec!["get", "list"]);
        assert_eq!(rbac.allow[1].kinds, vec!["*"]);
    }

    #[test]
    fn center_rbac_validate_rejects_empty_rule() {
        let rbac = FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec![],
                kinds: vec!["PluginMetaData".into()],
            }],
        };
        assert!(rbac.validate().is_err(), "a rule with no verbs is malformed");
    }

    // ── Group J: listen address defaults ────────────────────────────────────

    #[test]
    fn test_listen_address_defaults() {
        let cfg = EdgionControllerConfig::default();
        assert_eq!(cfg.grpc_listen(), "0.0.0.0:12151");
        assert_eq!(cfg.admin_listen(), "0.0.0.0:12101");
        assert_eq!(cfg.probe_listen(), "0.0.0.0:12100");
        assert_eq!(cfg.metrics_listen(), "0.0.0.0:12190");
    }

    // ── Group K: AdminTlsConfig ──────────────────────────────────────────────

    #[test]
    fn test_admin_tls_validate() {
        let ok = AdminTlsConfig {
            cert: "c".into(),
            key: "k".into(),
        };
        assert!(ok.validate().is_ok());

        let no_cert = AdminTlsConfig {
            cert: "   ".into(),
            key: "k".into(),
        };
        assert!(no_cert.validate().is_err());

        let no_key = AdminTlsConfig {
            cert: "c".into(),
            key: "".into(),
        };
        assert!(no_key.validate().is_err());
    }

    #[test]
    fn test_admin_tls_debug_redacts_key() {
        let c = AdminTlsConfig {
            cert: "certs/admin/server.crt".into(),
            key: "certs/admin/private.key".into(),
        };
        let dbg = format!("{c:?}");
        assert!(dbg.contains("***"), "key must be redacted: {dbg}");
        assert!(!dbg.contains("private.key"), "key path leaked: {dbg}");
        assert!(dbg.contains("server.crt"), "cert path should be visible: {dbg}");
    }
}
