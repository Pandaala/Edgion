//! Webhook service configuration for LinkSys
//!
//! Defines the connection parameters and health check settings for an external
//! HTTP webhook service. Referenced by KeyGet::Webhook via "namespace/name".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::gateway::backends::service_kind::EdgionService;
use crate::types::resources::common::{SecretSlot, TlsConfig};

// ============================================================
// WebhookAuth — authentication configuration
// ============================================================

/// Webhook authentication configuration.
///
/// Uses `type` field to select auth method. All methods require a Secret reference.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WebhookAuth {
    /// Basic authentication (username + password in Secret)
    Basic {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// Bearer token authentication (bearerToken in Secret)
    Bearer {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// HMAC signature authentication (hmacKey in Secret)
    Hmac {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// API Key authentication — injects the key as a header or query parameter.
    /// The secret must contain a `apiKey` data key.
    ApiKey {
        #[serde(flatten)]
        slot: SecretSlot,
        /// Where to inject the key: "header" or "query"
        r#in: String,
        /// Header name when `in = "header"` (e.g. "X-API-Key")
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "headerName")]
        header_name: Option<String>,
        /// Query parameter name when `in = "query"` (e.g. "api_key")
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "queryParam")]
        query_param: Option<String>,
    },
}

// ============================================================
// WebhookTarget — target backend for the webhook
// ============================================================

/// Webhook target backend.
///
/// Selects where the webhook request is sent. Use `Uri` for external HTTP/HTTPS
/// endpoints (existing behavior) or `Service` to target a Kubernetes Service
/// whose endpoints are injected by the Controller.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WebhookTarget {
    /// External HTTP/HTTPS endpoint (existing behavior).
    Uri { uri: String },

    /// Kubernetes Service backend.
    /// `kind` semantics match `EdgionService::from_kind()`:
    ///   `None` / `""` / `"Service"` / `"ServiceEndpointSlice"` → WRR over endpoints (injected by Controller)
    ///   `"ServiceClusterIp"`                                    → ClusterIP DNS, kube-proxy LB
    Service {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Pod IPs injected by the Controller. Format: "ip:port". Not set by users.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schemars(skip)]
        endpoints: Vec<String>,
    },
}

// ============================================================
// WebhookServiceConfig — main configuration
// ============================================================

/// Webhook HTTP service configuration.
///
/// Defines how to connect to an external HTTP service used for key resolution,
/// identity mapping, or other webhook-based lookups.
///
/// ## YAML Examples (LinkSys CRD)
///
/// External URI target:
/// ```yaml
/// apiVersion: edgion.io/v1
/// kind: LinkSys
/// metadata:
///   name: key-resolver
///   namespace: prod
/// spec:
///   type: webhook
///   config:
///     target:
///       type: uri
///       uri: "https://key-service.internal/resolve"
///     requestMethod: POST
///     timeoutMs: 3000
///     successStatusCodes: [200]
///     allowDegradation: true
///     statusOnError: 503
///     healthCheck:
///       active:
///         path: "/healthz"
///         intervalSec: 10
///         unhealthyThreshold: 3
/// ```
///
/// Kubernetes Service target:
/// ```yaml
///     target:
///       type: service
///       name: my-webhook-svc
///       namespace: prod
///       port: 8080
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookServiceConfig {
    // ============================================================
    // Authentication
    // ============================================================
    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,

    // ============================================================
    // Target
    // ============================================================
    /// Webhook target backend (required).
    /// Use `WebhookTarget::Uri` for an external HTTP/HTTPS URL or
    /// `WebhookTarget::Service` for a Kubernetes Service backend.
    pub target: WebhookTarget,

    // ============================================================
    // Connection
    // ============================================================
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
    pub tls: Option<TlsConfig>,

    // ============================================================
    // Response handling
    // ============================================================
    /// Status codes considered as success.
    /// Default: any 2xx status code
    #[serde(default)]
    pub success_status_codes: Option<Vec<u16>>,

    /// Static headers sent on every outbound call (merged with per-call headers; caller wins on conflict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_headers: Option<std::collections::HashMap<String, String>>,

    /// Maximum response body size to read in bytes.
    /// Prevents memory exhaustion from unexpectedly large webhook responses.
    ///
    /// Default: 1024 (1KB)
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

// ============================================================
// Default + Validation
// ============================================================

impl Default for WebhookServiceConfig {
    fn default() -> Self {
        Self {
            auth: None,
            target: WebhookTarget::Uri { uri: String::new() },
            request_method: default_method(),
            timeout_ms: default_timeout_ms(),
            tls: None,
            default_headers: None,
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
    pub fn get_validation_error(&self) -> Option<String> {
        match &self.target {
            WebhookTarget::Uri { uri } => {
                if uri.is_empty() {
                    return Some("uri is required".to_string());
                }
                if !uri.starts_with("http://") && !uri.starts_with("https://") {
                    return Some("uri must start with http:// or https://".to_string());
                }
            }
            WebhookTarget::Service { name, port, kind, .. } => {
                if name.is_empty() {
                    return Some("service name is required".to_string());
                }
                if port.is_some_and(|p| p <= 0) {
                    return Some("service port must be > 0".to_string());
                }
                match EdgionService::from_kind(kind.as_ref()) {
                    EdgionService::Unsupported(k) => {
                        return Some(format!("unsupported service kind: {k}"));
                    }
                    EdgionService::ServiceImport => {
                        return Some("ServiceImport is not yet supported for webhook".to_string());
                    }
                    EdgionService::ServiceExternalName => {
                        return Some("ServiceExternalName is not supported for webhook".to_string());
                    }
                    _ => {}
                }
            }
        }
        let method_upper = self.request_method.to_uppercase();
        if !matches!(
            method_upper.as_str(),
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
        ) {
            return Some("requestMethod must be a valid HTTP method".to_string());
        }
        if self.timeout_ms == 0 {
            return Some("timeoutMs must be greater than 0".to_string());
        }
        if !(200..=599).contains(&self.status_on_error) {
            return Some("statusOnError must be between 200 and 599".to_string());
        }
        if self.max_response_bytes == 0 {
            return Some("maxResponseBytes must be greater than 0".to_string());
        }
        None
    }
}

// Unit tests for this module live in `tests/webhook_test.rs`.
#[cfg(test)]
#[path = "tests/webhook_test.rs"]
mod tests;
