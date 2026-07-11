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

/// Service-backed webhook target fields.
///
/// Webhook-local on purpose: this is NOT the shared
/// [`crate::resources::http_route::BackendObjectReference`], whose `name` is
/// required and whose relaxation would silently degrade HTTPRoute's `backendRef`
/// validation. Every field here is optional so that a `url`-only target parses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookServiceRef {
    /// Group of the referent. Empty = core.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Kind of the referent. `None` / `""` defaults to `Service`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Name of the referent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Namespace of the backend. `None` = the LinkSys resource's namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Destination port. Required for a service target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

/// Webhook target backend: either an external URL or a Kubernetes Service.
///
/// The two forms are mutually exclusive and enforced by [`WebhookTarget::kind`],
/// not by the type system: the serialized shape must stay flat so that existing
/// CRs and conf_sync payloads round-trip unchanged. An untagged enum would have
/// silently matched `{url, name}` as a Service target.
///
/// ## YAML
///
/// ```yaml
/// # External URL (origin only)
/// target:
///   url: https://api.example.com
///   blockPrivate: true      # optional
///
/// # Kubernetes Service
/// target:
///   name: auth-svc
///   namespace: prod
///   port: 8080
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTarget {
    /// External URL target. Origin only: `scheme://host[:port]`, no path, query,
    /// fragment, or credentials. Mutually exclusive with the service fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// URL targets only. When `true`, resolved RFC1918 / unique-local addresses are
    /// rejected in addition to the always-blocked loopback and link-local ranges.
    /// `None` = inherit the gateway-local `outbound_egress.block_private` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_private: Option<bool>,

    /// Kubernetes Service target fields (flattened: no nesting in YAML).
    #[serde(flatten)]
    pub service: WebhookServiceRef,
}

/// Validated view of a [`WebhookTarget`]. Derived by [`WebhookTarget::kind`],
/// never stored.
#[derive(Debug)]
pub enum TargetKind<'a> {
    /// External URL target.
    Url {
        /// Validated origin-only URL.
        url: &'a str,
        /// `None` = inherit the gateway-local default.
        block_private: Option<bool>,
    },
    /// Kubernetes Service target.
    Service(&'a WebhookServiceRef),
}

impl WebhookTarget {
    /// Resolve this target to a validated [`TargetKind`], or return the first
    /// error found. This is the single source of truth: `validate_webhook_target`
    /// delegates here, so the schema layer and the gateway cannot disagree.
    pub fn kind(&self) -> Result<TargetKind<'_>, String> {
        let svc = &self.service;
        let has_service = !svc.group.is_empty()
            || svc.kind.is_some()
            || svc.name.is_some()
            || svc.namespace.is_some()
            || svc.port.is_some();

        match (&self.url, has_service) {
            (Some(_), true) => Err(
                "target: url and service fields (name/namespace/port/kind/group) are mutually exclusive; \
                 set exactly one form"
                    .to_string(),
            ),
            (None, false) => Err("target: one of url or name is required".to_string()),
            (Some(url), false) => {
                validate_origin_url(url)?;
                Ok(TargetKind::Url {
                    url,
                    block_private: self.block_private,
                })
            }
            (None, true) => {
                if self.block_private.is_some() {
                    return Err("target: blockPrivate applies to url targets only".to_string());
                }
                validate_service_ref(svc)?;
                Ok(TargetKind::Service(svc))
            }
        }
    }
}

/// Validate an origin-only URL: `scheme://host[:port]` with `http`/`https`,
/// no path, query, fragment, or credentials.
///
/// Path and method come from the `request` block, so admitting a path here would
/// leave `request.path`, `path_override`, and `healthCheck.active.path`
/// composition undefined.
fn validate_origin_url(raw: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("target: url is not a valid URL: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("target: url scheme must be http or https, got '{other}'")),
    }
    if parsed.host_str().is_none() {
        return Err("target: url must have a host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("target: url must not contain credentials".to_string());
    }
    // `Url::parse("https://host")` normalizes the path to "/".
    let path = parsed.path();
    if !path.is_empty() && path != "/" {
        return Err(format!(
            "target: url must be origin-only (scheme://host[:port]); path '{path}' is not allowed \
             — put the path in request.path.template"
        ));
    }
    if parsed.query().is_some() {
        return Err("target: url must be origin-only; a query string is not allowed".to_string());
    }
    if parsed.fragment().is_some() {
        return Err("target: url must be origin-only; a fragment is not allowed".to_string());
    }
    Ok(())
}

/// Validate a Service-backed target.
///
/// Checks:
/// - `name` is present and non-empty
/// - `port` is present and > 0
/// - `kind` is in the allowed set: {Service, ServiceClusterIp, ServiceEndpointSlice, None, ""}
/// - Rejects: ExternalName, ServiceImport, unsupported kinds
fn validate_service_ref(svc: &WebhookServiceRef) -> Result<(), String> {
    if svc.name.as_deref().unwrap_or("").is_empty() {
        return Err("target: one of url or name is required".to_string());
    }
    match svc.port {
        None => return Err("backendRef port is required".to_string()),
        Some(port) if port <= 0 => return Err("backendRef port must be > 0".to_string()),
        Some(_) => {}
    }
    match EdgionService::from_kind(svc.kind.as_ref()) {
        EdgionService::Unsupported(k) => Err(format!("unsupported service kind: {k}")),
        EdgionService::ServiceImport => Err("ServiceImport is not yet supported for webhook".to_string()),
        EdgionService::ServiceExternalName => Err("ServiceExternalName is not supported for webhook".to_string()),
        // Service, ServiceClusterIp, ServiceEndpointSlice
        _ => Ok(()),
    }
}

/// Validate a WebhookTarget and return an error message if invalid.
pub fn validate_webhook_target(target: &WebhookTarget) -> Option<String> {
    target.kind().err()
}

/// Validate a target together with the connection's TLS block.
///
/// Beyond [`validate_webhook_target`], this rejects `url: http://...` combined
/// with `tls.enabled: true`. A webhook target is a security boundary; silently
/// preferring the URL's plaintext scheme over an explicit `tls.enabled` would be
/// a downgrade the operator never asked for.
pub fn validate_connection_target(conn: &ExternalHttpConnection) -> Option<String> {
    let kind = match conn.target.kind() {
        Ok(k) => k,
        Err(e) => return Some(e),
    };
    if let TargetKind::Url { url, .. } = kind {
        let tls_enabled = conn.tls.as_ref().map(|t| t.enabled).unwrap_or(false);
        // Parse the URL to get the normalized scheme (lowercase), not the raw string prefix.
        // validate_origin_url has already run in target.kind(), so parse cannot fail.
        if let Ok(parsed) = url::Url::parse(url) {
            if parsed.scheme() == "http" && tls_enabled {
                return Some(
                    "target: url scheme is http but tls.enabled is true; use https:// or set tls.enabled: false"
                        .to_string(),
                );
            }
        }
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
