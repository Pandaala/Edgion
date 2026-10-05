//! Etcd client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resources::common::{SecretSlot, TlsConfig};

/// Maximum explicitly configured protobuf message size, excluding its envelope.
pub const MAX_ETCD_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum bootstrap or discovered endpoint count per generation.
pub const MAX_ETCD_ENDPOINTS: usize = 64;
/// Bound endpoint parsing and retained discovery metadata.
pub const MAX_ETCD_ENDPOINT_BYTES: usize = 2048;

/// Etcd client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdClientConfig {
    /// etcd server endpoints (e.g., "http://127.0.0.1:2379", "https://etcd.example.com:2379")
    pub endpoints: Vec<String>,

    /// Authentication configuration (username/password)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EtcdAuth>,

    /// TLS/SSL configuration for secure connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Timeout configuration (dial and request)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<EtcdTimeout>,

    /// HTTP/2 keep-alive configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<EtcdKeepAlive>,

    /// Namespace prefix for all keys (etcd v3 feature)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Auto-sync cluster members interval (in seconds, 0 to disable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_sync_interval: Option<u64>,

    /// Maximum message size for gRPC calls (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_send_size: Option<usize>,

    /// Maximum message size for gRPC responses (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_recv_size: Option<usize>,

    /// User agent string for client identification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// Reject connections to old cluster versions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_old_cluster: Option<bool>,

    /// Stable major.minor.patch floor; required when rejectOldCluster is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_cluster_version: Option<String>,

    /// Observability configuration (logging, metrics)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<EtcdObservability>,
}

impl EtcdClientConfig {
    /// Parse the configured floor without duplicating version syntax in consumers.
    pub fn parsed_min_cluster_version(&self) -> Result<Option<[u64; 3]>, &'static str> {
        self.min_cluster_version.as_deref().map(parse_etcd_version).transpose()
    }

    /// Canonical pure validation, shared by ingest and runtime construction.
    /// Diagnostics identify fields, never endpoint text or credential material.
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.endpoints.is_empty() {
            errors.push("Etcd configuration has no endpoints".to_owned());
        } else if self.endpoints.len() > MAX_ETCD_ENDPOINTS {
            errors.push(format!(
                "Etcd endpoints must contain at most {MAX_ETCD_ENDPOINTS} entries"
            ));
        }
        for (index, endpoint) in self.endpoints.iter().enumerate() {
            if let Err(reason) = normalize_etcd_endpoint(endpoint) {
                errors.push(format!("Etcd endpoints[{index}]: {reason}"));
            }
        }
        for (field, limit) in [
            ("maxCallSendSize", self.max_call_send_size),
            ("maxCallRecvSize", self.max_call_recv_size),
        ] {
            if limit.is_some_and(|limit| !(1..=MAX_ETCD_MESSAGE_BYTES).contains(&limit)) {
                errors.push(format!("Etcd {field} must be in 1..={MAX_ETCD_MESSAGE_BYTES} bytes"));
            }
        }
        if self.auto_sync_interval.is_some_and(|interval| interval > 86_400) {
            errors.push("Etcd autoSyncInterval must be in 0..=86400 seconds".to_owned());
        }
        if let Some(agent) = &self.user_agent {
            if agent.is_empty()
                || agent.len() > 256
                || agent.trim().is_empty()
                || !agent.bytes().all(|byte| (b' '..=b'~').contains(&byte))
            {
                errors.push("Etcd userAgent must contain 1..=256 printable ASCII bytes and not only spaces".to_owned());
            }
        }
        if self.reject_old_cluster == Some(true) {
            if self.min_cluster_version.is_none() {
                errors.push("Etcd rejectOldCluster requires minClusterVersion".to_owned());
            }
        } else if self.min_cluster_version.is_some() {
            errors.push("Etcd minClusterVersion requires rejectOldCluster=true".to_owned());
        }
        if self.parsed_min_cluster_version().is_err() {
            errors.push("Etcd minClusterVersion must be stable major.minor.patch without leading zeros".to_owned());
        }
        if let Some(timeout) = &self.timeout {
            if timeout.dial == Some(0) {
                errors.push("Etcd timeout.dial must be positive".to_owned());
            }
            if timeout.request == Some(0) {
                errors.push("Etcd timeout.request must be positive".to_owned());
            }
        }
        if let Some(keep_alive) = &self.keep_alive {
            match (keep_alive.time, keep_alive.timeout) {
                (None, None) if keep_alive.permit_without_stream.is_none() => {},
                (Some(time), Some(timeout)) if (1..=86_400).contains(&time) && (1..=86_400).contains(&timeout) => {},
                _ => errors.push("Etcd keepAlive requires time and timeout in 1..=86400 seconds; permitWithoutStream requires the pair".to_owned()),
            }
        }
        if let Some(observability) = &self.observability {
            if observability
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.labels.as_ref())
                .is_some_and(|labels| !labels.is_empty())
            {
                errors.push(
                    "Etcd observability.metrics.labels must be empty; arbitrary metric labels are unsupported"
                        .to_owned(),
                );
            }
            if let Some(logging) = &observability.logging {
                if !logging.enabled && (logging.level.is_some() || logging.slow_log_threshold.is_some()) {
                    errors.push(
                        "Etcd logging level and slowLogThreshold require observability.logging.enabled=true".to_owned(),
                    );
                }
                if logging
                    .level
                    .as_deref()
                    .is_some_and(|level| !matches!(level, "error" | "warn" | "info" | "debug" | "trace"))
                {
                    errors
                        .push("Etcd observability.logging.level must be error, warn, info, debug, or trace".to_owned());
                }
                if logging.slow_log_threshold == Some(0) {
                    errors.push("Etcd observability.logging.slowLogThreshold must be positive".to_owned());
                }
            }
        }
        errors
    }
}

/// Parse stable Etcd version numbers used by configuration and admission policy.
pub fn parse_etcd_version(value: &str) -> Result<[u64; 3], &'static str> {
    let mut parts = value.split('.');
    let mut version = [0; 3];
    for component in &mut version {
        let part = parts.next().ok_or("expected three version components")?;
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err("expected decimal version components without leading zeros");
        }
        *component = part.parse().map_err(|_| "version component exceeds u64")?;
    }
    if parts.next().is_some() {
        return Err("expected three version components");
    }
    Ok(version)
}

/// Validate and normalize an authority-only client URL. Does not resolve DNS or
/// make an egress decision; callers must classify addresses on every actual dial.
pub fn normalize_etcd_endpoint(value: &str) -> Result<String, &'static str> {
    if value.len() > MAX_ETCD_ENDPOINT_BYTES || value.chars().any(char::is_whitespace) || value.contains('\\') {
        return Err("endpoint must be at most 2048 bytes without whitespace or backslashes");
    }
    let (scheme, remainder) = value
        .split_once("://")
        .ok_or("endpoint requires an explicit HTTP(S) scheme")?;
    if !matches!(scheme, "http" | "https") {
        return Err("endpoint scheme must be http or https");
    }
    if remainder.split_once('/').is_some_and(|(_, path)| !path.is_empty()) {
        return Err("endpoint must not contain a non-root path");
    }
    let parsed = url::Url::parse(value).map_err(|_| "invalid endpoint URL")?;
    if parsed.host_str().is_none()
        || value.contains('@')
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err("endpoint requires a host and nonzero port without userinfo, query, or fragment");
    }
    Ok(format!(
        "{}://{}",
        parsed.scheme(),
        &parsed[url::Position::BeforeHost..url::Position::AfterPort]
    ))
}

/// Etcd authentication configuration.
///
/// Credentials must be stored in a Kubernetes Secret and referenced via `secretRef`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EtcdAuth {
    /// Embedded Secret slot (`secretRef` + resolved `secret`).
    /// `#[serde(flatten)]` keeps the YAML form unchanged at the parent level.
    #[serde(flatten)]
    pub slot: SecretSlot,
}

/// Etcd timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTimeout {
    /// Dial timeout in milliseconds (connection establishment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial: Option<u64>,

    /// Request timeout in milliseconds (per-request)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,
}

/// Etcd HTTP/2 keep-alive configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdKeepAlive {
    /// Keep-alive time interval in seconds (1..=86400, time between pings)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,

    /// Keep-alive timeout in seconds (1..=86400, max time waiting for ping ack)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Permit keep-alive pings without active streams
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permit_without_stream: Option<bool>,
}

/// Etcd observability configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdObservability {
    /// Metrics configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<EtcdMetrics>,

    /// Logging configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<EtcdLogging>,
}

/// Etcd metrics configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdMetrics {
    /// Enable metrics collection
    #[serde(default)]
    pub enabled: bool,

    /// Metrics labels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Etcd logging configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdLogging {
    /// Enable logging
    #[serde(default)]
    pub enabled: bool,

    /// Log level (e.g., "debug", "info", "warn", "error")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,

    /// Log slow operations (threshold in milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_log_threshold: Option<u64>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/etcd_test.rs"]
mod tests;
