//! Shared HTTP types for external service connections.
//!
//! Contains:
//! - `ResolvedSecrets` — controller-resolved `${secretRef:...}` map carried
//!   inline to the gateway over conf_sync.
//! - `WebhookTarget` / `validate_webhook_target` — target backend selector.
//! - `ExternalHttpConnection` — the shared connection-level config block
//!   embedded (via `#[serde(flatten)]`) into `WebhookServiceConfig` and future
//!   external-HTTP consumers.
//! - `WebhookRetry`, `WebhookRateLimit`, `WebhookHealthCheck` and sub-types —
//!   connection-level retry / rate-limit / health-check config that is part of
//!   `ExternalHttpConnection`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::resources::common::EdgionService;
use crate::resources::common::TlsConfig;
use crate::resources::http_route::BackendObjectReference;

// ============================================================
// ResolvedSecrets
// ============================================================

/// Controller-resolved `${secretRef:...}` values, carried inline to the gateway
/// over conf_sync. The map key is the raw template var_name (`"ns/name:key"`);
/// the value is the decoded Secret value.
///
/// This type intentionally has **no** `JsonSchema` derive (so it can never enter
/// a CRD), serializes as `"[redacted]"` under the admin redact guard, and has a
/// hand-written `Debug` so a `{:?}` on any enclosing struct cannot leak values.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ResolvedSecrets(pub BTreeMap<String, String>);

impl std::fmt::Debug for ResolvedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ResolvedSecrets(<{} redacted entries>)", self.0.len())
    }
}

// ============================================================
// WebhookTarget
// ============================================================

/// Webhook target backend.
///
/// Targets a Kubernetes Service backend for webhook requests. The gateway resolves
/// pod endpoints at call time based on the `kind` semantics.
///
/// `kind` semantics match `EdgionService::from_kind()`:
///   `None` / `""` / `"Service"` / `"ServiceEndpointSlice"` → WRR over pod endpoints resolved by the gateway at call time
///   `"ServiceClusterIp"`                                    → ClusterIP DNS, kube-proxy LB
/// Other kinds (ExternalName, ServiceImport, unsupported) are rejected.
///
/// Port is required (webhook connections need an explicit destination port).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookTarget {
    /// Backend service reference (name, namespace, port, kind).
    /// Matches the Gateway API BackendObjectReference pattern.
    /// Fields are flattened for serialization (no nesting in YAML).
    #[serde(flatten)]
    pub backend_ref: BackendObjectReference,
}

impl Default for WebhookTarget {
    fn default() -> Self {
        Self {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: String::new(),
                namespace: None,
                port: None,
            },
        }
    }
}

/// Validate a WebhookTarget and return an error message if invalid.
///
/// Checks:
/// - `backend_ref.port` is Some (required)
/// - `backend_ref.kind` is in allowed set: {Service, ServiceClusterIp, ServiceEndpointSlice, None, ""}
/// - Rejects: ExternalName, ServiceImport, unsupported kinds
pub fn validate_webhook_target(target: &WebhookTarget) -> Option<String> {
    let backend_ref = &target.backend_ref;

    // Require port
    if backend_ref.port.is_none() {
        return Some("backendRef port is required".to_string());
    }

    // Validate port value if present
    if let Some(port) = backend_ref.port {
        if port <= 0 {
            return Some("backendRef port must be > 0".to_string());
        }
    }

    // Validate kind
    match EdgionService::from_kind(backend_ref.kind.as_ref()) {
        EdgionService::Unsupported(k) => {
            return Some(format!("unsupported service kind: {k}"));
        }
        EdgionService::ServiceImport => {
            return Some("ServiceImport is not yet supported for webhook".to_string());
        }
        EdgionService::ServiceExternalName => {
            return Some("ServiceExternalName is not supported for webhook".to_string());
        }
        // All other kinds are allowed: Service, ServiceClusterIp, ServiceEndpointSlice
        _ => {}
    }

    None
}

// ============================================================
// WebhookRetry
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
// WebhookRateLimit
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
// WebhookHealthCheck
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
// ExternalHttpConnection
// ============================================================

/// Shared connection-level configuration for external HTTP services.
///
/// Embedded (via `#[serde(flatten)]`) into `WebhookServiceConfig` and any future
/// external-HTTP consumer. Contains target, TLS, timeout, retry, rate-limit,
/// health-check, and response-handling config.
///
/// NOTE: `deny_unknown_fields` is intentionally absent — `#[serde(flatten)]`
/// on the parent struct silently ignores `deny_unknown_fields` anyway, so the
/// relaxation is made explicit here (spec §3.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalHttpConnection {
    // ---- Target ----
    /// Webhook target backend (required).
    /// Points to a Kubernetes Service backend whose port must be specified.
    pub target: WebhookTarget,

    // ---- TLS ----
    /// TLS configuration for connecting to the webhook service.
    ///
    /// Supports custom CA certificates and mutual TLS (mTLS).
    /// If not set, uses the system CA bundle and no client certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    // ---- Timeout ----
    /// Request timeout in milliseconds.
    /// Default: 5000 (5 seconds)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    // ---- Retry ----
    /// Retry configuration for failed webhook calls.
    /// Default: no retry (fail immediately on first error).
    #[serde(default)]
    pub retry: Option<WebhookRetry>,

    // ---- Rate limit (outbound call protection) ----
    /// Global rate limit for outbound webhook calls.
    ///
    /// Protects the external webhook service from being overwhelmed by the gateway.
    /// Uses a simple AtomicU64 sliding window counter.
    /// When the limit is reached, key_get returns None immediately (same as degradation).
    #[serde(default)]
    pub rate_limit: Option<WebhookRateLimit>,

    // ---- Health check ----
    /// Health check configuration for the webhook service.
    /// Supports active probing, passive monitoring, or both (recommended).
    #[serde(default)]
    pub health_check: Option<WebhookHealthCheck>,

    // ---- Response handling ----
    /// Maximum response body size to read in bytes.
    /// Prevents memory exhaustion from unexpectedly large webhook responses.
    ///
    /// Default: 1024 (1KB)
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,

    /// Status codes considered as success.
    /// Default: any 2xx status code
    #[serde(default)]
    pub success_status_codes: Option<Vec<u16>>,
}

impl Default for ExternalHttpConnection {
    fn default() -> Self {
        Self {
            target: WebhookTarget::default(),
            tls: None,
            timeout_ms: default_timeout_ms(),
            retry: None,
            rate_limit: None,
            health_check: None,
            max_response_bytes: default_max_response_bytes(),
            success_status_codes: None,
        }
    }
}

// ============================================================
// Default functions
// ============================================================

pub fn default_timeout_ms() -> u64 {
    5000
}
pub fn default_max_response_bytes() -> usize {
    1024
} // 1KB
fn default_max_retries() -> u32 {
    1
}
fn default_retry_delay() -> u64 {
    100
}
pub fn default_true() -> bool {
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
