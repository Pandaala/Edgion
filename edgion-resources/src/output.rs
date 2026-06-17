//! Log output destination and rotation configuration types
//!
//! Used by the link_sys runtime (`core::gateway::link_sys`) to write
//! observability logs to local files / external sinks.
//!
//! NOTE: This module is unrelated to the `LinkSys` Kubernetes CRD
//! (`types::resources::link_sys`), which describes connections to external
//! systems like Redis/Etcd. For the CRD, use `crate::{LinkSys, LinkSysSpec}`.

use std::collections::HashMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a LinkSys resource by namespace + name.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysRef {
    pub namespace: String,
    pub name: String,
}

impl LinkSysRef {
    /// Returns the "namespace/name" key used by runtime stores.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
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

/// OTLP transport protocol.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OtlpProtocol {
    /// OTLP/gRPC — default port 4317, binary protobuf, HTTP/2.
    Grpc,
    /// OTLP/HTTP — default port 4318, binary protobuf.
    /// NOTE: not yet implemented at runtime; returns an error at startup.
    HttpProto,
}

impl Default for OtlpProtocol {
    fn default() -> Self {
        Self::Grpc
    }
}

/// Configuration for OTLP log output (self-contained, no LinkSys CRD needed).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpOutputConfig {
    /// OTLP receiver endpoint (e.g. "http://otel-collector:4317").
    pub endpoint: String,
    /// Transport protocol. Defaults to gRPC.
    #[serde(default)]
    pub protocol: OtlpProtocol,
    /// Extra request headers (e.g. auth tokens).
    /// NOTE: not yet supported at runtime in v1; logged as a warning and ignored
    /// due to a tonic version constraint (workspace tonic 0.12 vs otlp tonic 0.14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Per-request timeout in milliseconds. None = SDK default (10 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// String output destination configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StringOutput {
    /// Write to local file
    LocalFile(LocalFileWriterCfg),
    /// Ship to Elasticsearch via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Es { link_sys_ref: LinkSysRef },
    /// Ship to Kafka via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Kafka { link_sys_ref: LinkSysRef, topic: String },
    /// Ship to an OpenTelemetry Collector via OTLP.
    ///
    /// No LinkSys CRD is required. The Collector routes to any downstream backend
    /// (ES, Loki, ClickHouse, S3, Datadog, etc.) without further gateway changes.
    Otlp(OtlpOutputConfig),
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
mod tests {
    use super::*;

    #[test]
    fn test_link_sys_ref_key() {
        let r = LinkSysRef {
            namespace: "default".to_string(),
            name: "es-main".to_string(),
        };
        assert_eq!(r.key(), "default/es-main");
        assert_eq!(format!("{}", r), "default/es-main");
    }

    #[test]
    fn test_string_output_es_serde_roundtrip_struct() {
        let original = StringOutput::Es {
            link_sys_ref: LinkSysRef {
                namespace: "default".to_string(),
                name: "es-main".to_string(),
            },
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            json,
            r#"{"es":{"linkSysRef":{"namespace":"default","name":"es-main"}}}"#
        );
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Es { link_sys_ref } => {
                assert_eq!(link_sys_ref.namespace, "default");
                assert_eq!(link_sys_ref.name, "es-main");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_string_output_kafka_serde_roundtrip() {
        let original = StringOutput::Kafka {
            link_sys_ref: LinkSysRef {
                namespace: "default".to_string(),
                name: "kafka-main".to_string(),
            },
            topic: "edgion-access".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            json,
            r#"{"kafka":{"linkSysRef":{"namespace":"default","name":"kafka-main"},"topic":"edgion-access"}}"#
        );
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Kafka { link_sys_ref, topic } => {
                assert_eq!(link_sys_ref.key(), "default/kafka-main");
                assert_eq!(topic, "edgion-access");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_string_output_local_file_default_unchanged() {
        let s = StringOutput::default();
        assert!(matches!(s, StringOutput::LocalFile(_)));
    }

    #[test]
    fn test_string_output_otlp_grpc_serde_roundtrip() {
        let original = StringOutput::Otlp(OtlpOutputConfig {
            endpoint: "http://otel-collector:4317".to_string(),
            protocol: OtlpProtocol::Grpc,
            headers: None,
            timeout_ms: None,
        });
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            json,
            r#"{"otlp":{"endpoint":"http://otel-collector:4317","protocol":"grpc"}}"#
        );
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Otlp(cfg) => {
                assert_eq!(cfg.endpoint, "http://otel-collector:4317");
                assert!(matches!(cfg.protocol, OtlpProtocol::Grpc));
                assert!(cfg.headers.is_none());
                assert!(cfg.timeout_ms.is_none());
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_string_output_otlp_http_with_timeout_serde_roundtrip() {
        let original = StringOutput::Otlp(OtlpOutputConfig {
            endpoint: "https://otel.example.com:4318".to_string(),
            protocol: OtlpProtocol::HttpProto,
            headers: None,
            timeout_ms: Some(5000),
        });
        let json = serde_json::to_string(&original).unwrap();
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Otlp(cfg) => {
                assert_eq!(cfg.endpoint, "https://otel.example.com:4318");
                assert!(matches!(cfg.protocol, OtlpProtocol::HttpProto));
                assert_eq!(cfg.timeout_ms, Some(5000));
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_local_file_cfg_disk_threshold_default_and_roundtrip() {
        // Default is enabled at 90%.
        let def = LocalFileWriterCfg::default();
        assert_eq!(def.disk_usage_threshold_percent, Some(90));

        // Explicit 0 (disabled) survives a round-trip.
        let cfg = LocalFileWriterCfg {
            disk_usage_threshold_percent: Some(0),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LocalFileWriterCfg = serde_json::from_str(&json).unwrap();
        assert_eq!(back.disk_usage_threshold_percent, Some(0));
    }

    #[test]
    fn test_string_output_otlp_with_headers_serde_roundtrip() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        let original = StringOutput::Otlp(OtlpOutputConfig {
            endpoint: "http://otel-collector:4317".to_string(),
            protocol: OtlpProtocol::Grpc,
            headers: Some(headers),
            timeout_ms: None,
        });
        let json = serde_json::to_string(&original).unwrap();
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Otlp(cfg) => {
                let h = cfg.headers.expect("headers should round-trip");
                assert_eq!(h.get("Authorization").map(String::as_str), Some("Bearer token123"));
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }
}
