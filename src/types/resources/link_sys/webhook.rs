//! Webhook service configuration for LinkSys
//!
//! Defines the connection parameters and health check settings for an external
//! HTTP webhook service. Referenced by KeyGet::Webhook via "namespace/name".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::SecretReference;

// ============================================================
// WebhookServiceConfig — main configuration
// ============================================================

/// Webhook HTTP service configuration.
///
/// Defines how to connect to an external HTTP service used for key resolution,
/// identity mapping, or other webhook-based lookups.
///
/// ## YAML Example (LinkSys CRD)
///
/// ```yaml
/// apiVersion: edgion.io/v1
/// kind: LinkSys
/// metadata:
///   name: key-resolver
///   namespace: prod
/// spec:
///   type: webhook
///   config:
///     uri: "https://key-service.internal/resolve"
///     requestMethod: POST
///     timeoutMs: 3000
///     requestHeaders:
///       - Authorization
///       - X-Request-ID
///     successStatusCodes: [200]
///     allowDegradation: true
///     statusOnError: 503
///     healthCheck:
///       active:
///         path: "/healthz"
///         intervalSec: 10
///         unhealthyThreshold: 3
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookServiceConfig {
    // ============================================================
    // Connection
    // ============================================================
    /// Webhook service URL (required).
    /// Example: "https://key-service.internal/resolve"
    pub uri: String,

    /// HTTP method for the webhook request.
    /// Default: GET
    #[serde(default = "default_method")]
    pub request_method: String,

    /// Request timeout in milliseconds.
    /// Default: 5000 (5 seconds)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    // ============================================================
    // TLS
    // ============================================================
    /// TLS configuration for connecting to the webhook service.
    ///
    /// Supports custom CA certificates and mutual TLS (mTLS).
    /// If not set, uses the system CA bundle and no client certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<WebhookTls>,

    // ============================================================
    // Request forwarding
    // ============================================================
    /// Headers to forward from the original request to the webhook service.
    ///
    /// Behavior:
    /// - None: forward ONLY standard X-Forwarded-* headers
    /// - Some(list): forward the listed headers + X-Forwarded-* headers
    ///
    /// Hop-by-hop headers are always excluded.
    #[serde(default)]
    pub request_headers: Option<Vec<String>>,

    // ============================================================
    // Response handling
    // ============================================================
    /// Status codes considered as success.
    /// Default: any 2xx status code
    #[serde(default)]
    pub success_status_codes: Option<Vec<u16>>,

    /// Maximum response body size to read in bytes.
    /// Prevents memory exhaustion from unexpectedly large webhook responses.
    ///
    /// Default: 1024 (1KB)
    /// Hard upper limit: controlled by WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES (16KB)
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,

    // ============================================================
    // Degradation
    // ============================================================
    /// Whether to skip webhook resolution when the service is unavailable.
    ///
    /// - false (default): webhook failure causes key resolution to fail (returns None)
    /// - true: webhook failure is silently skipped, key_get returns None and the
    ///   fallback chain continues to the next source
    #[serde(default)]
    pub allow_degradation: bool,

    /// HTTP status code to return when the webhook is unreachable and
    /// allow_degradation is false. Only relevant when webhook is the sole key source.
    /// Default: 503
    #[serde(default = "default_status_on_error")]
    pub status_on_error: u16,

    // ============================================================
    // Retry
    // ============================================================
    /// Retry configuration for failed webhook calls.
    /// Default: no retry (fail immediately on first error).
    #[serde(default)]
    pub retry: Option<WebhookRetry>,

    // ============================================================
    // Rate limit (outbound call protection)
    // ============================================================
    /// Global rate limit for outbound webhook calls.
    ///
    /// Protects the external webhook service from being overwhelmed by the gateway.
    /// Uses a simple AtomicU64 sliding window counter.
    /// When the limit is reached, key_get returns None immediately (same as degradation).
    #[serde(default)]
    pub rate_limit: Option<WebhookRateLimit>,

    // ============================================================
    // Health check
    // ============================================================
    /// Health check configuration for the webhook service.
    /// Supports active probing, passive monitoring, or both (recommended).
    #[serde(default)]
    pub health_check: Option<WebhookHealthCheck>,
}

// ============================================================
// TLS
// ============================================================

/// TLS configuration for webhook service connections.
///
/// Consistent with RedisTls / EtcdTls in LinkSys.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTls {
    /// Enable TLS for the webhook connection.
    /// When the URI starts with `https://`, TLS is always used regardless of this flag.
    /// This flag controls custom TLS settings (CA, client cert, etc.).
    #[serde(default)]
    pub enabled: bool,

    /// TLS certificates configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<WebhookTlsCerts>,

    /// Skip certificate verification (DANGEROUS, only for development/testing).
    /// Default: false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_skip_verify: Option<bool>,
}

/// TLS certificate configuration for webhook connections.
///
/// Supports both inline PEM content and Kubernetes Secret references.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTlsCerts {
    /// CA certificate for verifying the webhook server (PEM format).
    /// If not set, uses the system CA bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,

    /// Client certificate for mutual TLS (PEM format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,

    /// Client private key for mutual TLS (PEM format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,

    /// Alternative: reference to a Kubernetes Secret containing TLS certs.
    /// The Secret should contain `ca.crt`, `tls.crt`, `tls.key` keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
}

// ============================================================
// Retry
// ============================================================

/// Retry configuration for failed webhook calls.
///
/// Only retries on transient errors. A webhook response with a non-success
/// status code that is NOT in `retry_on_status` will not be retried.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRetry {
    /// Maximum retry attempts (not including the initial call).
    /// Default: 1
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Delay between retries in milliseconds.
    /// Default: 100
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,

    /// Retry on timeout errors.
    /// Default: true
    #[serde(default = "default_true")]
    pub retry_on_timeout: bool,

    /// Retry on connection errors (refused, reset, etc.).
    /// Default: true
    #[serde(default = "default_true")]
    pub retry_on_connect_error: bool,

    /// Retry on specific HTTP status codes from the webhook.
    /// Default: [] (do not retry on status codes by default)
    #[serde(default)]
    pub retry_on_status: Vec<u16>,
}

// ============================================================
// Rate Limit
// ============================================================

/// Global rate limit for outbound webhook calls.
///
/// A simple sliding window counter that limits the total number of outbound
/// HTTP requests to this webhook service within a time window.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRateLimit {
    /// Maximum number of webhook calls allowed within the time window.
    pub rate: u64,

    /// Time window in seconds.
    /// Default: 1 (per-second limiting)
    #[serde(default = "default_rl_window")]
    pub window_sec: u64,
}

// ============================================================
// Health Check
// ============================================================

/// Health check configuration for a webhook service.
///
/// Supports two complementary mechanisms:
/// - Active: periodic GET probe to a health endpoint (safe recovery)
/// - Passive: monitors actual webhook call results (fast failure detection)
/// - Both (recommended): passive detects fast, active recovers safely
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookHealthCheck {
    /// Active health check: periodic GET probe to a health endpoint.
    #[serde(default)]
    pub active: Option<ActiveHealthCheck>,

    /// Passive health check: monitors actual webhook call results.
    #[serde(default)]
    pub passive: Option<PassiveHealthCheck>,
}

/// Active health check: periodic GET probe to a health endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHealthCheck {
    /// Health check endpoint path (appended to base URI).
    /// Example: "/healthz", "/ping"
    #[serde(default)]
    pub path: Option<String>,

    /// Interval between probes in seconds.
    /// Default: 10
    #[serde(default = "default_active_interval")]
    pub interval_sec: u64,

    /// Probe request timeout in milliseconds.
    /// Default: 2000
    #[serde(default = "default_active_timeout")]
    pub timeout_ms: u64,

    /// Consecutive successes before marking healthy.
    /// Default: 1
    #[serde(default = "default_healthy_threshold")]
    pub healthy_threshold: u32,

    /// Consecutive failures before marking unhealthy.
    /// Only used when passive is NOT enabled.
    /// Default: 3
    #[serde(default = "default_active_unhealthy_threshold")]
    pub unhealthy_threshold: u32,
}

/// Passive health check: monitors real webhook call results.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassiveHealthCheck {
    /// Consecutive failures to mark unhealthy.
    /// Default: 3
    #[serde(default = "default_passive_unhealthy_threshold")]
    pub unhealthy_threshold: u32,

    /// HTTP status codes from the webhook that count as failure.
    /// Default: [500, 502, 503, 504]
    #[serde(default = "default_failure_status_codes")]
    pub failure_status_codes: Vec<u16>,

    /// Whether to count request timeout as a failure.
    /// Default: true
    #[serde(default = "default_true")]
    pub count_timeout: bool,

    /// Backoff half-open configuration for recovery (passive-only).
    /// Only effective when active health check is NOT enabled.
    #[serde(default)]
    pub backoff: Option<PassiveBackoff>,
}

/// Backoff strategy for passive-only recovery (half-open circuit breaker).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassiveBackoff {
    /// Initial wait time in seconds before the first half-open probe.
    /// Default: 5
    #[serde(default = "default_initial_backoff")]
    pub initial_sec: u64,

    /// Multiplier for exponential backoff after each failed half-open probe.
    /// Default: 2.0
    #[serde(default = "default_backoff_multiplier")]
    pub multiplier: f64,

    /// Maximum backoff interval in seconds.
    /// Default: 60
    #[serde(default = "default_max_backoff")]
    pub max_sec: u64,
}

// ============================================================
// Defaults
// ============================================================

fn default_method() -> String {
    "GET".to_string()
}
fn default_timeout_ms() -> u64 {
    5000
}
fn default_max_response_bytes() -> usize {
    1024
} // 1KB
fn default_status_on_error() -> u16 {
    503
}
fn default_max_retries() -> u32 {
    1
}
fn default_retry_delay() -> u64 {
    100
}
fn default_true() -> bool {
    true
}
fn default_rl_window() -> u64 {
    1
}
fn default_active_interval() -> u64 {
    10
}
fn default_active_timeout() -> u64 {
    2000
}
fn default_healthy_threshold() -> u32 {
    1
}
fn default_active_unhealthy_threshold() -> u32 {
    3
}
fn default_passive_unhealthy_threshold() -> u32 {
    3
}
fn default_failure_status_codes() -> Vec<u16> {
    vec![500, 502, 503, 504]
}
fn default_initial_backoff() -> u64 {
    5
}
fn default_backoff_multiplier() -> f64 {
    2.0
}
fn default_max_backoff() -> u64 {
    60
}

/// Global hard upper limit for webhook response body size (16KB).
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;

// ============================================================
// Default + Validation
// ============================================================

impl Default for WebhookServiceConfig {
    fn default() -> Self {
        Self {
            uri: String::new(),
            request_method: default_method(),
            timeout_ms: default_timeout_ms(),
            tls: None,
            request_headers: None,
            success_status_codes: None,
            max_response_bytes: default_max_response_bytes(),
            allow_degradation: false,
            status_on_error: default_status_on_error(),
            retry: None,
            rate_limit: None,
            health_check: None,
        }
    }
}

impl WebhookServiceConfig {
    /// Validate the configuration and return an error message if invalid.
    pub fn get_validation_error(&self) -> Option<&str> {
        if self.uri.is_empty() {
            return Some("uri is required");
        }
        if !self.uri.starts_with("http://") && !self.uri.starts_with("https://") {
            return Some("uri must start with http:// or https://");
        }
        let method_upper = self.request_method.to_uppercase();
        if !matches!(
            method_upper.as_str(),
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
        ) {
            return Some("requestMethod must be a valid HTTP method");
        }
        if self.timeout_ms == 0 {
            return Some("timeoutMs must be greater than 0");
        }
        if !(200..=599).contains(&self.status_on_error) {
            return Some("statusOnError must be between 200 and 599");
        }
        if self.max_response_bytes == 0 {
            return Some("maxResponseBytes must be greater than 0");
        }
        if self.max_response_bytes > WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES {
            return Some("maxResponseBytes exceeds global limit (16KB)");
        }
        None
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WebhookServiceConfig::default();
        assert_eq!(config.request_method, "GET");
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.max_response_bytes, 1024);
        assert!(!config.allow_degradation);
        assert_eq!(config.status_on_error, 503);
        assert!(config.retry.is_none());
        assert!(config.rate_limit.is_none());
        assert!(config.health_check.is_none());
    }

    #[test]
    fn test_validation_empty_uri() {
        let config = WebhookServiceConfig::default();
        assert_eq!(config.get_validation_error(), Some("uri is required"));
    }

    #[test]
    fn test_validation_invalid_uri() {
        let config = WebhookServiceConfig {
            uri: "ftp://invalid".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("uri must start with http:// or https://")
        );
    }

    #[test]
    fn test_validation_invalid_method() {
        let config = WebhookServiceConfig {
            uri: "http://example.com".to_string(),
            request_method: "INVALID".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("requestMethod must be a valid HTTP method")
        );
    }

    #[test]
    fn test_validation_zero_timeout() {
        let config = WebhookServiceConfig {
            uri: "http://example.com".to_string(),
            timeout_ms: 0,
            ..Default::default()
        };
        assert_eq!(config.get_validation_error(), Some("timeoutMs must be greater than 0"));
    }

    #[test]
    fn test_validation_max_response_bytes_exceeds_global() {
        let config = WebhookServiceConfig {
            uri: "http://example.com".to_string(),
            max_response_bytes: WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES + 1,
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("maxResponseBytes exceeds global limit (16KB)")
        );
    }

    #[test]
    fn test_validation_valid_config() {
        let config = WebhookServiceConfig {
            uri: "https://api.example.com/resolve".to_string(),
            ..Default::default()
        };
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn test_serde_minimal_yaml() {
        let yaml = r#"
uri: "http://localhost:8080/resolve"
"#;
        let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.uri, "http://localhost:8080/resolve");
        assert_eq!(config.request_method, "GET");
        assert_eq!(config.timeout_ms, 5000);
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn test_serde_full_yaml() {
        let yaml = r#"
uri: "https://key-service.internal/resolve"
requestMethod: POST
timeoutMs: 3000
requestHeaders:
  - Authorization
  - X-Request-ID
successStatusCodes: [200]
maxResponseBytes: 2048
allowDegradation: true
statusOnError: 503
retry:
  maxRetries: 1
  retryDelayMs: 100
  retryOnTimeout: true
  retryOnConnectError: true
  retryOnStatus: [502, 503, 504]
rateLimit:
  rate: 200
  windowSec: 1
healthCheck:
  passive:
    unhealthyThreshold: 3
    failureStatusCodes: [500, 502, 503, 504]
    countTimeout: true
  active:
    path: "/healthz"
    intervalSec: 10
    timeoutMs: 2000
    healthyThreshold: 1
"#;
        let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.uri, "https://key-service.internal/resolve");
        assert_eq!(config.request_method, "POST");
        assert_eq!(config.timeout_ms, 3000);
        assert_eq!(config.max_response_bytes, 2048);
        assert!(config.allow_degradation);
        // Validate before consuming fields with unwrap()
        assert!(config.get_validation_error().is_none());
        assert!(config.retry.is_some());
        let retry = config.retry.unwrap();
        assert_eq!(retry.max_retries, 1);
        assert!(retry.retry_on_timeout);
        assert_eq!(retry.retry_on_status, vec![502, 503, 504]);
        assert!(config.rate_limit.is_some());
        let rl = config.rate_limit.unwrap();
        assert_eq!(rl.rate, 200);
        assert_eq!(rl.window_sec, 1);
        assert!(config.health_check.is_some());
        let hc = config.health_check.unwrap();
        assert!(hc.passive.is_some());
        assert!(hc.active.is_some());
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = WebhookServiceConfig {
            uri: "https://example.com/api".to_string(),
            request_method: "POST".to_string(),
            ..Default::default()
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.uri, config.uri);
        assert_eq!(deserialized.request_method, config.request_method);
        assert_eq!(deserialized.timeout_ms, config.timeout_ms);
    }

    #[test]
    fn test_passive_backoff_defaults() {
        let yaml = r#"
unhealthyThreshold: 5
countTimeout: true
backoff:
  initialSec: 5
"#;
        let passive: PassiveHealthCheck = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(passive.unhealthy_threshold, 5);
        assert!(passive.count_timeout);
        let backoff = passive.backoff.unwrap();
        assert_eq!(backoff.initial_sec, 5);
        assert!((backoff.multiplier - 2.0).abs() < f64::EPSILON);
        assert_eq!(backoff.max_sec, 60);
    }
}
