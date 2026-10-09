//! Log output destination and rotation configuration types
//!
//! Used by the link_sys runtime (`core::gateway::link_sys`) to write
//! observability logs to local files / external sinks.
//!
//! Output configuration selects local storage or a reference to a `LinkSys`
//! connection. The resource schema itself lives in `crate::resources::link_sys`;
//! use `crate::{LinkSys, LinkSysSpec}` for the CRD.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a LinkSys resource by namespace + name.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkSysRef {
    pub namespace: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkSysRefInput {
    namespace: String,
    name: String,
}

impl<'de> Deserialize<'de> for LinkSysRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = LinkSysRefInput::deserialize(deserializer)?;
        let reference = Self {
            namespace: input.namespace,
            name: input.name,
        };
        reference.validate().map_err(serde::de::Error::custom)?;
        Ok(reference)
    }
}

impl LinkSysRef {
    /// Validate the bounded identity before it enters logs or metrics.
    pub fn validate(&self) -> Result<(), &'static str> {
        crate::resources::refs::targets::process_output_link_sys_target(self)
            .map(|_| ())
            .map_err(|_| "invalid LinkSys reference")
    }

    /// Returns the validated "namespace/name" key used by runtime stores.
    pub fn key(&self) -> Result<String, &'static str> {
        self.validate()?;
        Ok(format!("{}/{}", self.namespace, self.name))
    }
}

impl fmt::Display for LinkSysRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

/// Log file rotation configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RotationConfig {
    /// Rotation strategy
    #[serde(default)]
    pub strategy: RotationStrategy,

    /// Maximum number of rotated files to keep. Set `0` explicitly for unlimited
    /// retention. Defaults to 10 — note this is a *named* serde default so a
    /// partially-specified `rotation` block (e.g. only `strategy`) still gets the
    /// bounded 10, matching the struct `Default`, rather than serde's bare-`usize`
    /// `0` (which would silently disable cleanup and grow without bound).
    #[serde(default = "default_max_files")]
    pub max_files: usize,

    /// Interval in seconds to check for rotation (default: 30)
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
}

fn default_max_files() -> usize {
    10
}

fn default_check_interval_secs() -> u64 {
    30
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            strategy: RotationStrategy::default(), // Size(100MB)
            max_files: 10,
            check_interval_secs: default_check_interval_secs(),
        }
    }
}

/// Log rotation strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RotationStrategy {
    /// Rotate when file size exceeds limit (in bytes)
    Size(u64),
    /// Rotate daily (at midnight)
    Daily,
    /// Rotate hourly
    Hourly,
    /// Never rotate
    Never,
}

/// Default: 100MB per file
impl Default for RotationStrategy {
    fn default() -> Self {
        Self::Size(100 * 1024 * 1024)
    }
}

/// Local file writer configuration (for YAML/JSON config)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileWriterCfg {
    /// Relative path for log file (e.g. "logs/edgion_access.log")
    #[serde(default = "default_access_log_path")]
    pub path: String,

    /// Queue size for the write queue (optional)
    /// If not set, defaults to `cpu_cores * 10_000`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_size: Option<usize>,

    /// Rotation configuration (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RotationConfig>,

    /// Mark the sink unhealthy when filesystem usage reaches this percent
    /// (0-100). Drives `DataSender::healthy()` so `AccessLogger` first-healthy
    /// fallback can shed to another sink *before* writes start failing.
    ///
    /// Default 90. Set `0` (or omit programmatically) to disable the disk check
    /// entirely. WARNING: if LocalFile is the LAST sink in the first-healthy
    /// chain, crossing this threshold makes it unhealthy and access log entries
    /// are dropped (counted in `edgion_*_log_dropped_total`). To keep writing a
    /// near-full disk to the last drop, raise this or set it to `0`. The probe
    /// is sampled on the rotation-check cadence (default 30s); it is
    /// bounded-stale, not real-time. Recovery is automatic and traffic-free: the
    /// writer wakes on that same cadence even when idle, so once the disk drops
    /// back below the threshold the sink returns to healthy without needing new
    /// log traffic.
    #[serde(default = "default_disk_usage_threshold_percent")]
    pub disk_usage_threshold_percent: Option<u8>,
}

fn default_access_log_path() -> String {
    "logs/edgion_access.log".to_string()
}

fn default_disk_usage_threshold_percent() -> Option<u8> {
    Some(90)
}

impl Default for LocalFileWriterCfg {
    fn default() -> Self {
        Self {
            path: default_access_log_path(),
            queue_size: None,
            rotation: None,
            disk_usage_threshold_percent: default_disk_usage_threshold_percent(),
        }
    }
}

/// String output destination configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum StringOutput {
    /// Write to local file
    LocalFile(LocalFileWriterCfg),
    /// Ship to Elasticsearch via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Es { link_sys_ref: LinkSysRef },
    /// Ship to Kafka via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Kafka { link_sys_ref: LinkSysRef, topic: String },
    /// Ship OTLP/gRPC logs through a shared LinkSys provider.
    #[serde(rename_all = "camelCase")]
    Otlp { link_sys_ref: LinkSysRef },
}

impl Default for StringOutput {
    fn default() -> Self {
        Self::LocalFile(LocalFileWriterCfg::default())
    }
}

/// Runtime local file writer configuration (internal use)
#[derive(Debug, Clone)]
pub struct LocalFileWriterConfig {
    /// Relative path for log file
    pub path: String,
    /// Queue size for the write queue (optional)
    /// If None, will use cpu_cores * 10_000 at runtime
    pub queue_size: Option<usize>,
    /// Rotation configuration
    pub rotation: RotationConfig,
    /// Disk-usage health threshold (percent). `None`/`Some(0)` = disabled.
    pub disk_usage_threshold_percent: Option<u8>,
}

impl LocalFileWriterConfig {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            queue_size: None,
            rotation: RotationConfig::default(),
            disk_usage_threshold_percent: None,
        }
    }

    pub fn with_queue_size(mut self, size: usize) -> Self {
        self.queue_size = Some(size);
        self
    }

    pub fn with_rotation(mut self, rotation: RotationConfig) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_disk_threshold(mut self, threshold: Option<u8>) -> Self {
        self.disk_usage_threshold_percent = threshold;
        self
    }
}

impl From<LocalFileWriterCfg> for LocalFileWriterConfig {
    fn from(cfg: LocalFileWriterCfg) -> Self {
        Self {
            path: cfg.path,
            queue_size: cfg.queue_size,
            rotation: cfg.rotation.unwrap_or_default(),
            disk_usage_threshold_percent: cfg.disk_usage_threshold_percent,
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/output_test.rs"]
mod tests;
