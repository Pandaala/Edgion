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

use crate::resources::common::ServiceBackendKind;
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
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
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

/// Validate an active-health path as a root-relative URL reference.
///
/// The path is joined to a runtime-selected backend origin. Requiring exactly
/// one leading slash and rejecting backslashes prevents the URL parser's
/// special-scheme normalization from treating the value as a replacement
/// authority. Percent-encoded bytes remain ordinary path data and are not
/// decoded for these structural checks.
pub fn validate_active_health_path(raw: &str) -> Result<(), String> {
    if !raw.starts_with('/') || raw.as_bytes().get(1) == Some(&b'/') {
        return Err("must be a root-relative URL reference starting with exactly one '/'".to_string());
    }
    if raw.contains('\\') {
        return Err("must not contain a backslash".to_string());
    }

    let base = url::Url::parse("https://health-check.invalid/")
        .map_err(|e| format!("internal validation base is invalid: {e}"))?;
    let parsed = base
        .join(raw)
        .map_err(|e| format!("must be a valid root-relative URL reference: {e}"))?;

    if parsed.scheme() != base.scheme()
        || parsed.host_str() != base.host_str()
        || parsed.port_or_known_default() != base.port_or_known_default()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("must not replace the target authority".to_string());
    }
    if parsed.fragment().is_some() {
        return Err("must not contain a fragment".to_string());
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
        Some(port) if !crate::gwapi_types::is_valid_port(port as i64) => {
            return Err(format!(
                "backendRef port must be in {}..={}",
                crate::gwapi_types::PORT_MIN,
                crate::gwapi_types::PORT_MAX
            ))
        }
        Some(_) => {}
    }
    match ServiceBackendKind::from_kind(svc.kind.as_ref()) {
        ServiceBackendKind::Unsupported(k) => Err(format!("unsupported service kind: {k}")),
        ServiceBackendKind::ServiceImport => Err("ServiceImport is not yet supported for webhook".to_string()),
        ServiceBackendKind::ServiceExternalName => Err("ServiceExternalName is not supported for webhook".to_string()),
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
    if let Some(path) = conn
        .health_check
        .as_ref()
        .and_then(|health| health.active.as_ref())
        .and_then(|active| active.path.as_deref())
    {
        if let Err(e) = validate_active_health_path(path) {
            return Some(format!("healthCheck.active.path: {e}"));
        }
    }
    if let Some(retry) = conn.retry.as_ref() {
        if let Some(e) = validate_retry(retry) {
            return Some(e);
        }
    }
    if let Some(success) = conn.success.as_ref() {
        if let Some(e) = crate::resources::common::success::validate_success(success) {
            return Some(e);
        }
    }
    if let Some(s) = conn.success.as_ref() {
        if !s.body.is_empty() && conn.max_response_bytes < MIN_BODY_PREDICATE_BYTES {
            return Some(format!(
                "maxResponseBytes must be at least {MIN_BODY_PREDICATE_BYTES} when success.body predicates are set"
            ));
        }
    }
    if let Some(tpl) = conn.timeout_ms_template.as_ref() {
        let compiled = match crate::resources::edgion_plugins::plugin_configs::CompiledTemplate::compile(tpl) {
            Ok(c) => c,
            Err(e) => return Some(format!("timeoutMsTemplate: {e}")),
        };
        if tpl.contains("${secretRef:") {
            return Some("timeoutMsTemplate must not reference ${secretRef:...}".to_string());
        }
        if let Some(e) = reject_client_controlled_vars(&compiled, "timeoutMsTemplate") {
            return Some(e);
        }
    }
    if conn.timeout_ms == 0 || conn.timeout_ms > MAX_OUTBOUND_TIMEOUT_MS {
        return Some(format!("timeoutMs must be between 1 and {MAX_OUTBOUND_TIMEOUT_MS}"));
    }
    if conn.max_response_bytes == 0 {
        return Some("maxResponseBytes must be greater than 0".to_string());
    }
    None
}

/// Reject any template variable that is not on the trusted allow-list for
/// `timeoutMsTemplate`.
///
/// This field drives a per-request timeout deadline and is security-sensitive:
/// a client-controlled value would let an attacker pin the timeout to something
/// tiny (forcing a timeout on every call — a reliable DoS).
///
/// ALLOW ONLY `${ctx:...}` (set by a trusted upstream plugin, never the raw
/// client request), `${remoteIp}`, and matched-route metadata
/// (`${matchedHttpRoute:...}` / `${matchedGrpcRoute:...}`). Everything else —
/// `header`/`query`/`cookie`/`path`/`uri`/`requestPath`/`requestUri`/`method`/
/// `secretRef` — is rejected.
fn reject_client_controlled_vars(
    tpl: &crate::resources::edgion_plugins::plugin_configs::CompiledTemplate,
    field_name: &str,
) -> Option<String> {
    use crate::resources::edgion_plugins::plugin_configs::{TemplateSegment, TemplateVarType};
    for seg in tpl.segments() {
        if let TemplateSegment::Variable { var_type, .. } = seg {
            let allowed = matches!(
                var_type,
                TemplateVarType::Ctx
                    | TemplateVarType::RemoteIp
                    | TemplateVarType::MatchedHttpRoute
                    | TemplateVarType::MatchedGrpcRoute
            );
            if !allowed {
                return Some(format!(
                    "{field_name} must not read client-controlled input (header/query/cookie/path/method/uri); \
                     use ${{ctx:...}} (set by a trusted upstream plugin), ${{remoteIp}}, or a matched-route variable"
                ));
            }
        }
    }
    None
}

/// Minimum `max_response_bytes` required when `success.body` predicates are set.
/// The default 1 KB cap is too small to reliably hold the inspected field, and a
/// truncated body surfaces as `ResponseTooLarge`, not as a failed predicate.
pub const MIN_BODY_PREDICATE_BYTES: usize = 4096;

/// Upper bound for a template-rendered per-request timeout, in milliseconds.
/// A client-supplied `${header:...}` timeout must not be able to pin a worker.
pub const MAX_OUTBOUND_TIMEOUT_MS: u64 = 60_000;

/// Lower bound for a template-rendered per-request timeout, in milliseconds.
/// A render below this floor is treated exactly like a non-numeric/zero/empty
/// render: it falls back to the static `timeoutMs`. Without a floor, a
/// tiny-but-nonzero value (e.g. "1") would make the outbound call time out on
/// (near-)every attempt — a reliable denial of service.
pub const MIN_OUTBOUND_TIMEOUT_MS: u64 = 50;

// ============================================================
// WebhookRetry
// ============================================================

/// Absolute upper bound for any single outbound retry backoff, in milliseconds.
/// The retry sleep is inline on the data-plane request path, so an operator must
/// not be able to configure a multi-minute stall.
pub const MAX_RETRY_BACKOFF_CEILING_MS: u64 = 10_000;

/// Absolute upper bound for `maxRetries`. Combined with `MAX_RETRY_BACKOFF_CEILING_MS`,
/// this is what actually makes the "no multi-minute stall" invariant above
/// hold: an unbounded `maxRetries` (e.g. 200) would still allow tens of
/// minutes of inline `tokio::time::sleep` on a live data-plane request even
/// with a capped per-attempt backoff.
///
/// Note this only bounds the *retry backoff* portion of the stall. The static
/// `timeout_ms` field is independently bounded by `MAX_OUTBOUND_TIMEOUT_MS` in
/// `validate_connection_target` — without that bound, a large per-attempt
/// timeout combined with `maxRetries` would still allow a multi-minute inline
/// stall regardless of how tight the backoff ceiling is.
pub const MAX_RETRIES_CEILING: u32 = 10;

/// Backoff growth policy between retry attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackoffPolicy {
    /// delay = min(maxDelayMs, retryDelayMs << (attempt-1)).
    #[default]
    Exponential,
    /// delay = retryDelayMs (constant).
    Fixed,
}

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

    /// Base delay of the backoff curve, in milliseconds. Under
    /// `backoffPolicy: exponential` the Nth retry waits
    /// `retryDelayMs << (N-1)`, capped by `maxDelayMs`; under `fixed` it is the
    /// constant delay for every retry.
    /// Default: 100
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,

    /// Retry on timeout errors. `None` = the consumer's default (webhook: false,
    /// forward_auth: true). See design §4.3.
    #[serde(default)]
    pub retry_on_timeout: Option<bool>,

    /// Retry on connection errors (refused, reset, etc.).
    /// Default: true
    #[serde(default = "default_true")]
    pub retry_on_connect_error: bool,

    /// Retry on specific HTTP status codes from the webhook.
    /// Default: [] (do not retry on status codes by default)
    #[serde(default)]
    pub retry_on_status: Vec<u16>,

    /// Backoff growth policy between retries. Default: exponential.
    #[serde(default)]
    pub backoff_policy: BackoffPolicy,

    /// Per-attempt backoff ceiling in milliseconds. Range 1..=10000.
    /// Default: 1000.
    #[serde(default = "default_max_delay")]
    pub max_delay_ms: u64,

    /// Add 50-100% jitter to each backoff delay. Default: true.
    #[serde(default = "default_true")]
    pub jitter: bool,

    /// On a 429/503 whose status is also in `retry_on_status`, honor the
    /// `Retry-After` header (clamped to `max_delay_ms`). Default: false.
    #[serde(default)]
    pub honor_retry_after: bool,

    /// Retry when a `success.body` predicate classifies the response as failure.
    /// Default: false (a business error like {"code":-101} is not transient).
    #[serde(default)]
    pub retry_on_body_failure: bool,
}

/// Validate a retry block. Returns the first error message, or None.
pub fn validate_retry(retry: &WebhookRetry) -> Option<String> {
    if retry.retry_delay_ms == 0 {
        return Some("retryDelayMs must be greater than 0".to_string());
    }
    if !(1..=MAX_RETRY_BACKOFF_CEILING_MS).contains(&retry.max_delay_ms) {
        return Some(format!(
            "maxDelayMs must be between 1 and {MAX_RETRY_BACKOFF_CEILING_MS}"
        ));
    }
    if retry.max_retries > MAX_RETRIES_CEILING {
        return Some(format!("maxRetries must be between 0 and {MAX_RETRIES_CEILING}"));
    }
    None
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

    /// Template rendering a per-request timeout in milliseconds, e.g.
    /// "${ctx:desired_timeout_ms}". Only `${ctx:...}`, `${remoteIp}`, and
    /// matched-route variables are accepted at admission (see
    /// `reject_client_controlled_vars`) — a client-controlled source
    /// (header/query/cookie/path/method/uri) would let an attacker pin the
    /// deadline to a tiny value. A non-numeric, empty, or below-floor
    /// (< `MIN_OUTBOUND_TIMEOUT_MS`) render falls back to `timeout_ms`. The
    /// rendered value is clamped to `MAX_OUTBOUND_TIMEOUT_MS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms_template: Option<String>,

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

    /// Success criteria (status codes + optional JSON-Pointer body predicates).
    /// Default: any 2xx status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<crate::resources::common::success::SuccessSpec>,

    // ---- Failure policy (currently honored by forward_auth only) ----
    /// HTTP status code to return when the outbound call fails. Range
    /// 200-599.
    /// Default: 503.
    ///
    /// Only the forward_auth plugin reads this field; it is inert for
    /// webhook.
    #[serde(default = "default_status_on_error")]
    pub status_on_error: u16,
}

impl Default for ExternalHttpConnection {
    fn default() -> Self {
        Self {
            target: WebhookTarget::default(),
            tls: None,
            timeout_ms: default_timeout_ms(),
            timeout_ms_template: None,
            retry: None,
            rate_limit: None,
            health_check: None,
            max_response_bytes: default_max_response_bytes(),
            success: None,
            status_on_error: default_status_on_error(),
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
fn default_max_delay() -> u64 {
    1000
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
pub fn default_status_on_error() -> u16 {
    503
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod failure_policy_tests {
    use super::*;

    #[test]
    fn failure_policy_defaults() {
        let c = ExternalHttpConnection::default();
        assert_eq!(c.status_on_error, 503);
    }

    #[test]
    fn failure_policy_parses_top_level() {
        let c: ExternalHttpConnection =
            serde_yaml::from_str("target:\n  url: https://api.example.com\nstatusOnError: 502\n").unwrap();
        assert_eq!(c.status_on_error, 502);
    }
}

#[cfg(test)]
mod active_health_path_tests {
    use super::*;

    fn connection_with_path(path: &str) -> ExternalHttpConnection {
        ExternalHttpConnection {
            target: WebhookTarget {
                url: Some("https://api.example.com".to_string()),
                ..Default::default()
            },
            health_check: Some(WebhookHealthCheck {
                active: Some(ActiveHealthCheck {
                    path: Some(path.to_string()),
                    interval_sec: 10,
                    timeout_ms: 2_000,
                    healthy_threshold: 1,
                    unhealthy_threshold: 3,
                }),
                passive: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn active_health_path_accepts_root_relative_path_and_query() {
        for path in ["/healthz", "/healthz?ready=1", "/%2F%5C%23?next=%2F"] {
            assert!(
                validate_active_health_path(path).is_ok(),
                "path {path:?} must be accepted"
            );
            assert!(
                validate_connection_target(&connection_with_path(path)).is_none(),
                "path {path:?} must pass connection validation"
            );
        }
    }

    #[test]
    fn active_health_path_rejects_authority_and_fragment_forms() {
        for path in [
            "healthz",
            "?ready=1",
            "https://evil.example/healthz",
            "//evil.example/healthz",
            "///evil.example/healthz",
            r"/\evil.example/healthz",
            r"/healthz\tail",
            "/healthz#details",
        ] {
            let err = validate_connection_target(&connection_with_path(path))
                .unwrap_or_else(|| panic!("path {path:?} must be rejected"));
            assert!(err.contains("healthCheck.active.path"), "got: {err}");
        }
    }
}

#[cfg(test)]
mod success_body_max_response_bytes_tests {
    use super::*;

    #[test]
    fn body_predicate_requires_explicit_max_response_bytes() {
        // A success.body predicate on a connection left at the 1KB default must be rejected.
        let c: ExternalHttpConnection = serde_yaml::from_str(
            "target:\n  url: https://api.example.com\nmaxResponseBytes: 512\nsuccess:\n  body:\n    - pointer: /code\n      equals: 0\n",
        )
        .unwrap();
        let err = validate_connection_target(&c).expect("must require explicit maxResponseBytes");
        assert!(err.contains("maxResponseBytes"), "got: {err}");
    }

    #[test]
    fn body_predicate_passes_with_adequate_max_response_bytes() {
        let c: ExternalHttpConnection = serde_yaml::from_str(
            "target:\n  url: https://api.example.com\nmaxResponseBytes: 8192\nsuccess:\n  body:\n    - pointer: /code\n      equals: 0\n",
        )
        .unwrap();
        assert!(validate_connection_target(&c).is_none());
    }

    #[test]
    fn no_body_predicate_allows_default_max_response_bytes() {
        // The default 1KB cap is fine when there's no body predicate to inspect.
        let c: ExternalHttpConnection = serde_yaml::from_str("target:\n  url: https://api.example.com\n").unwrap();
        assert!(validate_connection_target(&c).is_none());
    }
}

#[cfg(test)]
mod timeout_ms_template_tests {
    use super::*;

    #[test]
    fn default_has_no_timeout_template() {
        let c = ExternalHttpConnection::default();
        assert!(c.timeout_ms_template.is_none());
    }

    #[test]
    fn valid_template_passes_validation() {
        let c: ExternalHttpConnection = serde_yaml::from_str(
            "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${ctx:desired_timeout_ms}\"\n",
        )
        .unwrap();
        assert_eq!(c.timeout_ms_template.as_deref(), Some("${ctx:desired_timeout_ms}"));
        assert!(validate_connection_target(&c).is_none());
    }

    #[test]
    fn malformed_template_is_rejected_at_admission() {
        // Unclosed "${" must be caught by CompiledTemplate::compile at validation
        // time, not silently at runtime.
        let c: ExternalHttpConnection =
            serde_yaml::from_str("target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${ctx:X-Test\"\n")
                .unwrap();
        let err = validate_connection_target(&c).expect("malformed template must be rejected");
        assert!(err.contains("timeoutMsTemplate"), "got: {err}");
    }

    #[test]
    fn secret_ref_in_template_is_rejected_at_admission() {
        // ${secretRef:...} is never resolved for timeoutMsTemplate (not scanned by
        // collect_secret_ref_names nor the forward_auth credential resolver), so a
        // config that references one would silently fall back to the static
        // timeoutMs at runtime with no visible error. Reject it at admission instead.
        let c: ExternalHttpConnection = serde_yaml::from_str(
            "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${secretRef:ns/name:key}\"\n",
        )
        .unwrap();
        let err = validate_connection_target(&c).expect("secretRef in timeoutMsTemplate must be rejected");
        assert!(err.contains("timeoutMsTemplate"), "got: {err}");
        assert!(err.contains("secretRef"), "got: {err}");
    }

    /// CRITICAL 1 regression: a client-controlled var in `timeoutMsTemplate` is an
    /// auth-bypass / DoS vector on forward_auth (a client-chosen 1ms deadline makes
    /// the auth call always time out). `${header:...}`, `${query:...}`,
    /// `${cookie:...}`, and `${method}` must all be rejected at admission.
    #[test]
    fn timeout_template_rejects_client_controlled_vars() {
        for expr in [
            "${header:X-Auth-Timeout}",
            "${query:timeout}",
            "${cookie:timeout}",
            "${method}",
            "${path}",
            "${uri}",
            "${requestPath}",
            "${requestUri}",
        ] {
            let c: ExternalHttpConnection = serde_yaml::from_str(&format!(
                "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"{expr}\"\n"
            ))
            .unwrap();
            let err = validate_connection_target(&c)
                .unwrap_or_else(|| panic!("{expr} must be rejected in timeoutMsTemplate"));
            assert!(err.contains("timeoutMsTemplate"), "got: {err}");
            assert!(err.contains("client-controlled"), "got: {err}");
        }
    }

    /// Trusted variable types must still be accepted for `timeoutMsTemplate`.
    #[test]
    fn timeout_template_accepts_trusted_vars() {
        for expr in [
            "${ctx:is_staging}",
            "${remoteIp}",
            "${matchedHttpRoute:name}",
            "${matchedGrpcRoute:name}",
        ] {
            let c: ExternalHttpConnection = serde_yaml::from_str(&format!(
                "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"{expr}\"\n"
            ))
            .unwrap();
            assert!(
                validate_connection_target(&c).is_none(),
                "{expr} should be accepted in timeoutMsTemplate"
            );
        }
    }
}

#[cfg(test)]
mod timeout_ms_bound_tests {
    use super::*;

    /// The static `timeoutMs` field must share the same upper bound as the
    /// template-rendered timeout (`MAX_OUTBOUND_TIMEOUT_MS`). Without this, a
    /// `timeoutMs: 600000` + `maxRetries: 10` config would be admitted and
    /// stall the data-plane request inline for ~110 minutes.
    #[test]
    fn rejects_timeout_ms_above_ceiling() {
        let c: ExternalHttpConnection =
            serde_yaml::from_str("target:\n  url: https://api.example.com\ntimeoutMs: 120000\n").unwrap();
        let err = validate_connection_target(&c).expect("120000 exceeds MAX_OUTBOUND_TIMEOUT_MS");
        assert!(err.contains("timeoutMs"), "got: {err}");
    }

    #[test]
    fn accepts_reasonable_timeout_ms() {
        let c: ExternalHttpConnection =
            serde_yaml::from_str("target:\n  url: https://api.example.com\ntimeoutMs: 5000\n").unwrap();
        assert!(validate_connection_target(&c).is_none());
    }

    #[test]
    fn accepts_timeout_ms_at_ceiling() {
        let c: ExternalHttpConnection = serde_yaml::from_str(&format!(
            "target:\n  url: https://api.example.com\ntimeoutMs: {MAX_OUTBOUND_TIMEOUT_MS}\n"
        ))
        .unwrap();
        assert!(validate_connection_target(&c).is_none());
    }
}

#[cfg(test)]
mod retry_schema_tests {
    use super::*;

    #[test]
    fn retry_new_fields_default() {
        let r: WebhookRetry = serde_yaml::from_str("maxRetries: 3\n").unwrap();
        assert!(matches!(r.backoff_policy, BackoffPolicy::Exponential));
        assert_eq!(r.max_delay_ms, 1000);
        assert!(r.jitter);
        assert!(!r.honor_retry_after);
        assert_eq!(r.retry_on_timeout, None); // per-consumer default resolved later
    }

    #[test]
    fn retry_rejects_excessive_max_delay() {
        let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nmaxDelayMs: 20000\n").unwrap();
        let err = validate_retry(&r).expect("20000 exceeds ceiling");
        assert!(err.contains("maxDelayMs"), "got: {err}");
    }

    #[test]
    fn retry_rejects_zero_base_delay() {
        let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nretryDelayMs: 0\n").unwrap();
        assert!(validate_retry(&r).is_some());
    }

    #[test]
    fn retry_policy_parses_lowercase() {
        let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nbackoffPolicy: fixed\n").unwrap();
        assert!(matches!(r.backoff_policy, BackoffPolicy::Fixed));
    }

    /// IMPORTANT 4 regression: `maxRetries: 200` + `maxDelayMs: 10000` would allow
    /// ~33 minutes of inline `tokio::time::sleep` on a live data-plane request —
    /// the very multi-minute stall `MAX_RETRY_BACKOFF_CEILING_MS`'s doc says must
    /// be impossible. `maxRetries` itself must also be bounded.
    #[test]
    fn retry_rejects_excessive_max_retries() {
        let r: WebhookRetry = serde_yaml::from_str("maxRetries: 200\n").unwrap();
        let err = validate_retry(&r).expect("200 exceeds the maxRetries ceiling");
        assert!(err.contains("maxRetries"), "got: {err}");
    }

    #[test]
    fn retry_accepts_max_retries_at_ceiling() {
        let r: WebhookRetry = serde_yaml::from_str(&format!("maxRetries: {MAX_RETRIES_CEILING}\n")).unwrap();
        assert!(validate_retry(&r).is_none());
    }
}
