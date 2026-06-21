//! Webhook service configuration for LinkSys
//!
//! Defines the connection parameters and health check settings for an external
//! HTTP webhook service. Referenced by KeyGet::Webhook via "namespace/name".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{EdgionService, SecretSlot, TlsConfig};
use crate::resources::http_route::BackendObjectReference;

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
// WebhookOutboundRequest — outbound request specification
// ============================================================

/// Outbound request specification for webhooks.
/// Declares which path, method, args, headers, and cookies are forwarded to the backend.
/// Omit path/method to reuse the frozen incoming values.
/// Omit args/headers/cookies blocks to forward nothing of that kind (empty allow-list).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookOutboundRequest {
    /// Outbound path specification. Omit to use frozen incoming path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathSpec>,

    /// Outbound HTTP method. Omit to use frozen incoming method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<MethodSpec>,

    /// Query arguments allow-list (origin + custom).
    #[serde(default)]
    pub args: ListFields,

    /// HTTP headers allow-list (origin + custom).
    #[serde(default)]
    pub headers: ListFields,

    /// Cookies allow-list (origin + custom).
    #[serde(default)]
    pub cookies: ListFields,
}

/// Path specification for outbound webhook requests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathSpec {
    /// Path template: literal string or ${type:name} interpolation.
    pub template: String,

    /// If true, consumer may pass pathOverride at call time.
    #[serde(default)]
    pub allow_override: bool,
}

/// HTTP method specification for outbound webhook requests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodSpec {
    /// Method template: literal (POST, GET, etc.) or ${type:name} interpolation.
    pub template: String,
}

/// Allow-listed fields (args/headers/cookies) split into origin and custom sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListFields {
    /// Fields forwarded from the incoming request (frozen at entry time).
    #[serde(default)]
    pub origin: Vec<OriginField>,

    /// Fields explicitly constructed by the operator.
    #[serde(default)]
    pub custom: Vec<CustomField>,
}

/// An origin field: forwarded from the incoming request with optional presence control.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OriginField {
    /// Field name in both incoming and outbound request.
    pub name: String,

    /// Behavior when the field is absent from the incoming request.
    #[serde(default)]
    pub presence: PresencePolicy,
}

/// Presence policy for origin fields.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PresencePolicy {
    /// Absent field is silently omitted (default).
    SkipIfMissing,

    /// Absent field causes webhook call to fail (respects allowDegradation).
    Required,

    /// Absent field triggers PluginLog alert but request continues.
    WarnIfMissing,
}

impl Default for PresencePolicy {
    fn default() -> Self {
        Self::SkipIfMissing
    }
}

/// A custom field injected into the outbound webhook request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomField {
    /// Field name in the outbound webhook request.
    pub name: String,

    /// Source: exactly one of value, template, or secret.
    #[serde(flatten)]
    pub source: CustomSource,
}

/// Source for a custom field: literal, template, or K8s Secret reference.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CustomSource {
    /// Verbatim literal string.
    Value(String),

    /// Template interpolated at call time using ctx-set template engine.
    Template(String),

    /// Value resolved from a Kubernetes Secret at config time.
    Secret(SecretSpec),
}

/// Reference to a field within a Kubernetes Secret.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSpec {
    /// Reference to the Kubernetes Secret object.
    pub secret_ref: SecretObjectReference,

    /// Key within the Secret's data map.
    pub key: String,

    /// Optional prefix prepended to the resolved value (e.g., "Bearer " for auth headers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// Reference to a Kubernetes Secret object.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretObjectReference {
    /// Secret namespace. Defaults to the LinkSys namespace if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Secret name.
    pub name: String,
}

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
/// Kubernetes Service target:
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
///       name: "my-webhook-svc"
///       namespace: "prod"
///       port: 8080
///     request:
///       path:
///         template: "/webhook"
///       method:
///         template: "POST"
///       headers:
///         origin:
///           - name: "X-Tenant-Id"
///       custom:
///         - name: "X-Source"
///           value: "edgion"
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookServiceConfig {
    // ============================================================
    // Authentication (DEPRECATED - shim field)
    // ============================================================
    /// Authentication configuration (DEPRECATED).
    ///
    /// **DEPRECATED:** This field accepts the old authentication format for backward compatibility
    /// during rolling upgrades. It is not used at runtime in this release.
    ///
    /// Migrate to `request.headers.custom` with `source: secret` for custom header injection.
    /// This field will be removed in v1.X+1.
    ///
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,

    // ============================================================
    // Target
    // ============================================================
    /// Webhook target backend (required).
    /// Points to a Kubernetes Service backend whose port must be specified.
    pub target: WebhookTarget,

    // ============================================================
    // Outbound Request
    // ============================================================
    /// Outbound webhook request specification.
    /// Defines the allowed request shape: path, method, args, headers, cookies.
    /// Default: empty (no fields forwarded).
    #[serde(default)]
    pub request: WebhookOutboundRequest,

    // ============================================================
    // Connection (DEPRECATED - shim field)
    // ============================================================
    /// HTTP method for the webhook request (DEPRECATED).
    ///
    /// **DEPRECATED:** This field is accepted for backward compatibility but not used at runtime.
    /// Migrate to the new `request.method` field in Wave B.
    ///
    /// Default: GET (when absent in new configs)
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_method: Option<serde_json::Value>,

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

    /// Static headers sent on every outbound call (DEPRECATED - shim field).
    ///
    /// **DEPRECATED:** This field accepts the old header format for backward compatibility.
    /// It is not used at runtime in this release.
    ///
    /// Migrate to the new `request.headers` field in Wave B.
    /// This field will be removed in v1.X+1.
    ///
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_headers: Option<serde_json::Value>,

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
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: String::new(),
                    namespace: None,
                    port: None, // Will fail validation (port required)
                },
            },
            request: WebhookOutboundRequest {
                path: None,
                method: None,
                args: ListFields::default(),
                headers: ListFields::default(),
                cookies: ListFields::default(),
            },
            request_method: None,
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
    ///
    /// Note: Deprecated shim fields (`auth`, `request_method`, `default_headers`)
    /// are not validated; they are accepted for backward compatibility but not used at runtime.
    pub fn get_validation_error(&self) -> Option<String> {
        // Validate the target
        if let Some(err) = validate_webhook_target(&self.target) {
            return Some(err);
        }

        // Note: request_method is now a deprecated shim field and is not validated.
        // It was previously validated as a valid HTTP method, but is no longer used at runtime.

        // Validate timeout
        if self.timeout_ms == 0 {
            return Some("timeoutMs must be greater than 0".to_string());
        }

        // Validate status on error
        if !(200..=599).contains(&self.status_on_error) {
            return Some("statusOnError must be between 200 and 599".to_string());
        }

        // Validate max response bytes
        if self.max_response_bytes == 0 {
            return Some("maxResponseBytes must be greater than 0".to_string());
        }

        // Validate the request field-name invariants (args/headers/cookies).
        if let Some(err) = validate_request_fields(&self.request) {
            return Some(err);
        }

        None
    }
}

/// Validate field-name invariants across the request's args/headers/cookies.
///
/// Three spec-MUST edge cases are enforced:
/// 1. A field name appearing in both `origin` and `custom` of the same group.
/// 2. Duplicate names within `origin`, or within `custom`, of the same group.
/// 3. A `headers.origin` entry named `Cookie` conflicting with a non-empty `cookies.*`.
///
/// Header names are compared case-insensitively (per HTTP spec); cookie names
/// (and arg names) are compared case-sensitively. The first error encountered
/// is returned.
fn validate_request_fields(request: &WebhookOutboundRequest) -> Option<String> {
    // args and cookies: case-sensitive names.
    if let Some(err) = validate_field_group(&request.args, false) {
        return Some(err);
    }
    // headers: case-insensitive names.
    if let Some(err) = validate_field_group(&request.headers, true) {
        return Some(err);
    }
    if let Some(err) = validate_field_group(&request.cookies, false) {
        return Some(err);
    }

    // Validation 3: headers.origin[name: Cookie] conflicts with any cookies.*.
    let has_cookie_header = request
        .headers
        .origin
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case("Cookie"));
    let has_cookies = !request.cookies.origin.is_empty() || !request.cookies.custom.is_empty();
    if has_cookie_header && has_cookies {
        return Some(
            "cannot specify both headers.origin[name:Cookie] and cookies.* — use one or the other".to_string(),
        );
    }

    None
}

/// Validate a single field group (args, headers, or cookies).
///
/// `case_insensitive` controls whether names are normalized to lowercase before
/// comparison (true for headers). Checks, in order:
/// - duplicate names within `origin`,
/// - duplicate names within `custom`,
/// - a name shared between `origin` and `custom`.
fn validate_field_group(group: &ListFields, case_insensitive: bool) -> Option<String> {
    let norm = |name: &str| -> String {
        if case_insensitive {
            name.to_lowercase()
        } else {
            name.to_string()
        }
    };

    // Validation 2: duplicate names within origin.
    let mut origin_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for field in &group.origin {
        if !origin_names.insert(norm(&field.name)) {
            return Some(format!("duplicate field name '{}' in origin", field.name));
        }
    }

    // Validation 2: duplicate names within custom.
    let mut custom_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for field in &group.custom {
        if !custom_names.insert(norm(&field.name)) {
            return Some(format!("duplicate field name '{}' in custom", field.name));
        }
    }

    // Validation 1: a name appearing in both origin and custom.
    for field in &group.custom {
        if origin_names.contains(&norm(&field.name)) {
            return Some(format!("field '{}' appears in both origin and custom", field.name));
        }
    }

    None
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod webhook_outbound_request_tests {
    use super::*;

    #[test]
    fn test_webhook_outbound_request_all_origin() {
        let yaml_str = r#"
args:
  origin:
    - name: tenant_id
    - name: trace_id
      presence: required
headers:
  origin:
    - name: X-Tenant-Id
cookies:
  origin:
    - name: session_id
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(request.args.origin.len(), 2);
        assert_eq!(request.headers.origin.len(), 1);
        assert_eq!(request.cookies.origin.len(), 1);
        assert!(request.path.is_none());
    }

    #[test]
    fn test_webhook_outbound_request_with_path_and_method() {
        let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args: {}
headers: {}
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert!(request.path.is_some());
        assert_eq!(request.path.as_ref().unwrap().template, "/webhook/${ctx:action}");
        assert_eq!(request.path.as_ref().unwrap().allow_override, true);
        assert!(request.method.is_some());
        assert_eq!(request.method.as_ref().unwrap().template, "POST");
    }

    #[test]
    fn test_custom_field_with_secret() {
        let yaml_str = r#"
- name: Authorization
  secret:
    secretRef:
      name: webhook-token
    key: token
    prefix: "Bearer "
"#;
        let custom_fields: Vec<CustomField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(custom_fields.len(), 1);
        assert_eq!(custom_fields[0].name, "Authorization");
        match &custom_fields[0].source {
            CustomSource::Secret(spec) => {
                assert_eq!(spec.secret_ref.name, "webhook-token");
                assert_eq!(spec.key, "token");
                assert_eq!(spec.prefix, Some("Bearer ".to_string()));
            }
            _ => panic!("Expected Secret source"),
        }
    }

    #[test]
    fn test_custom_field_with_literal_value() {
        let yaml_str = r#"
- name: X-Source
  value: edgion
"#;
        let custom_fields: Vec<CustomField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(custom_fields.len(), 1);
        assert_eq!(custom_fields[0].name, "X-Source");
        match &custom_fields[0].source {
            CustomSource::Value(val) => {
                assert_eq!(val, "edgion");
            }
            _ => panic!("Expected Value source"),
        }
    }

    #[test]
    fn test_custom_field_with_template() {
        let yaml_str = r#"
- name: X-Action
  template: "${ctx:action}"
"#;
        let custom_fields: Vec<CustomField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(custom_fields.len(), 1);
        assert_eq!(custom_fields[0].name, "X-Action");
        match &custom_fields[0].source {
            CustomSource::Template(tpl) => {
                assert_eq!(tpl, "${ctx:action}");
            }
            _ => panic!("Expected Template source"),
        }
    }

    #[test]
    fn test_presence_policy_default() {
        let origin_field = OriginField {
            name: "X-Foo".to_string(),
            presence: Default::default(),
        };
        assert!(matches!(origin_field.presence, PresencePolicy::SkipIfMissing));
    }

    #[test]
    fn test_presence_policy_required() {
        let yaml_str = r#"
- name: X-Required
  presence: required
"#;
        let fields: Vec<OriginField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].presence, PresencePolicy::Required));
    }

    #[test]
    fn test_presence_policy_warn() {
        let yaml_str = r#"
- name: X-Warn
  presence: warnIfMissing
"#;
        let fields: Vec<OriginField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].presence, PresencePolicy::WarnIfMissing));
    }

    #[test]
    fn test_secret_object_reference_with_namespace() {
        let yaml_str = r#"
namespace: my-namespace
name: webhook-secret
"#;
        let secret_ref: SecretObjectReference = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(secret_ref.namespace, Some("my-namespace".to_string()));
        assert_eq!(secret_ref.name, "webhook-secret");
    }

    #[test]
    fn test_secret_object_reference_without_namespace() {
        let yaml_str = r#"
name: webhook-secret
"#;
        let secret_ref: SecretObjectReference = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert!(secret_ref.namespace.is_none());
        assert_eq!(secret_ref.name, "webhook-secret");
    }

    #[test]
    fn test_webhook_outbound_request_all_custom() {
        let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args:
  custom:
    - name: source
      value: edgion
    - name: action
      template: "${ctx:action}"
headers:
  custom:
    - name: Authorization
      secret:
        secretRef:
          name: webhook-token
        key: token
        prefix: "Bearer "
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert!(request.path.is_some());
        assert!(request.method.is_some());
        assert_eq!(request.path.as_ref().unwrap().allow_override, true);
        assert_eq!(request.args.custom.len(), 2);
        assert_eq!(request.headers.custom.len(), 1);
    }

    #[test]
    fn test_list_fields_default() {
        let fields: ListFields = Default::default();
        assert_eq!(fields.origin.len(), 0);
        assert_eq!(fields.custom.len(), 0);
    }

    #[test]
    fn test_deny_unknown_fields_on_path_spec() {
        let yaml_str = r#"
template: "/webhook"
allowOverride: true
unknownField: value
"#;
        let result: Result<PathSpec, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_deny_unknown_fields_on_method_spec() {
        let yaml_str = r#"
template: "POST"
unknownField: value
"#;
        let result: Result<MethodSpec, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_deny_unknown_fields_on_webhook_outbound_request() {
        let yaml_str = r#"
path:
  template: "/webhook"
unknownField: value
"#;
        let result: Result<WebhookOutboundRequest, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    /// Build an otherwise-valid config so request-field validation is the only
    /// thing that can fail.
    fn valid_base_config() -> WebhookServiceConfig {
        let mut config = WebhookServiceConfig::default();
        config.target.backend_ref.name = "webhook-svc".to_string();
        config.target.backend_ref.port = Some(8080);
        config
    }

    fn origin(name: &str) -> OriginField {
        OriginField {
            name: name.to_string(),
            presence: PresencePolicy::SkipIfMissing,
        }
    }

    fn custom(name: &str) -> CustomField {
        CustomField {
            name: name.to_string(),
            source: CustomSource::Value("x".to_string()),
        }
    }

    #[test]
    fn test_same_field_origin_and_custom_rejected() {
        // args: same name in origin and custom.
        let mut config = valid_base_config();
        config.request.args.origin = vec![origin("tenant_id")];
        config.request.args.custom = vec![custom("tenant_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("appears in both origin and custom"), "got: {err}");

        // headers: case-insensitive collision (X-Trace-Id vs x-trace-id).
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("X-Trace-Id")];
        config.request.headers.custom = vec![custom("x-trace-id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("appears in both origin and custom"), "got: {err}");

        // cookies: case-sensitive — distinct case must NOT collide.
        let mut config = valid_base_config();
        config.request.cookies.origin = vec![origin("Session")];
        config.request.cookies.custom = vec![custom("session")];
        assert!(
            config.get_validation_error().is_none(),
            "cookie names are case-sensitive; different case should be allowed"
        );
    }

    #[test]
    fn test_duplicate_names_in_origin_rejected() {
        // args duplicate in origin.
        let mut config = valid_base_config();
        config.request.args.origin = vec![origin("trace_id"), origin("trace_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("origin"),
            "got: {err}"
        );

        // headers duplicate in origin, case-insensitive.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("X-Id"), origin("x-id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("origin"),
            "got: {err}"
        );
    }

    #[test]
    fn test_duplicate_names_in_custom_rejected() {
        let mut config = valid_base_config();
        config.request.headers.custom = vec![custom("Authorization"), custom("Authorization")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("custom"),
            "got: {err}"
        );
    }

    #[test]
    fn test_cookie_both_origin_and_custom_rejected() {
        // headers.origin[name: Cookie] with cookies.origin present.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("Cookie")];
        config.request.cookies.origin = vec![origin("session_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

        // case-insensitive header name "cookie" with cookies.custom present.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("cookie")];
        config.request.cookies.custom = vec![custom("token")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

        // Cookie header with no cookies.* configured is fine.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("Cookie")];
        assert!(config.get_validation_error().is_none(), "Cookie header alone is valid");
    }
}

// Unit tests for this module live in `tests/webhook_test.rs`.
#[cfg(test)]
#[path = "tests/webhook_test.rs"]
mod tests;
