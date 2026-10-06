//! Etcd client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretSlot, TlsConfig};

/// Maximum configured endpoint count per generation.
pub const MAX_ETCD_ENDPOINTS: usize = 64;
/// Bound endpoint parsing and retained endpoint metadata.
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
}

impl EtcdClientConfig {
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
        errors
    }
}

/// Validate and normalize an authority-only client URL. Does not resolve DNS or
/// make an egress decision; SDK-owned dialing is a separate policy boundary.
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
    /// Keep-alive time interval in seconds (time between pings)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,

    /// Keep-alive timeout in seconds (max time waiting for ping ack)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Permit keep-alive pings without active streams
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permit_without_stream: Option<bool>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/etcd_test.rs"]
mod tests;
