//! Managed outbound HTTP client — owns the active-health JoinHandle and SSRF guard.
//!
//! [`ManagedOutboundClient`] is the single owner of a compiled outbound connection:
//! - Builds a `reqwest::Client` from `ExternalHttpConnection.tls`.
//! - Resolves the target backend into a [`ResolvedTarget`].
//! - Pre-compiles the request spec into a [`CompiledRequest`].
//! - Spawns an active-health probe task if configured, storing the
//!   `JoinHandle`; aborts it in `Drop`.
//! - Provides [`ManagedOutboundClient::dial_url`] which applies the SSRF guard
//!   before returning a base URL for the caller to use.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use edgion_resources::resources::common::external_http::{
    ActiveHealthCheck, ExternalHttpConnection, PassiveHealthCheck, TargetKind, WebhookRetry, WebhookTarget,
};
use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
use edgion_resources::resources::common::success::SuccessSpec;
use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::edgion_plugins::plugin_configs::CompiledTemplate;

use edgion_common::dns::ResolverSpec;

use crate::backends::service_kind::EdgionService;
use crate::backends::validate_endpoint_backend_addr;
use crate::observe::metrics::record_egress_blocked;
use crate::plugins::http::common::http_client::{build_webhook_client, get_http_client_arc};
use crate::util::net::IpClass;
use edgion_resources::resources::common::external_http::validate_connection_target;

use super::egress::{default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver};
use super::request::CompiledRequest;
use super::target::ResolvedTarget;

// Re-export SlidingWindowCounter from the webhook runtime for outbound rate limiting.
use crate::link_sys::providers::webhook::runtime::SlidingWindowCounter;

// ============================================================
// HealthState
// ============================================================

/// Shared health state for a managed outbound connection.
///
/// Owned by both [`ManagedOutboundClient`] and the spawned active-health task so
/// that either side can read and update the current health status without holding
/// a lock.
pub struct HealthState {
    /// Current health status: `true` = healthy (default on construction).
    pub healthy: AtomicBool,
    /// Consecutive passive-failure counter for the passive-health gate.
    pub passive_failures: AtomicU32,
    /// Epoch-seconds of the last half-open probe admission (passive-only recovery).
    pub last_halfopen: AtomicU64,
    /// Current backoff interval in seconds (passive-only recovery).
    pub backoff_sec: AtomicU64,
}

impl HealthState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
        })
    }
}

// ============================================================
// ManagedOutboundDialError
// ============================================================

/// Errors returned by [`ManagedOutboundClient::dial_url`].
#[derive(Debug)]
pub enum ManagedOutboundDialError {
    /// The resolved target currently has no ready backend endpoint.
    NoEndpoint,
    /// The resolved URL was blocked by the SSRF guard (loopback / link-local address,
    /// or — for a URL target with `blockPrivate: true` — a private/unique-local address).
    SsrfRejected(String),
}

impl std::fmt::Display for ManagedOutboundDialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEndpoint => write!(f, "no ready backend endpoint"),
            Self::SsrfRejected(msg) => write!(f, "SSRF guard rejected target: {msg}"),
        }
    }
}

// ============================================================
// ManagedOutboundClient
// ============================================================

/// Compiled, owned outbound HTTP client with active-health management and SSRF guard.
///
/// Created via [`ManagedOutboundClient::from_parts`] at resource-load time.
/// The active-health task (if configured) is aborted when this struct is dropped.
pub struct ManagedOutboundClient {
    /// HTTP client for outbound requests (may be custom TLS or the shared default).
    pub http_client: Arc<reqwest::Client>,
    /// Resolved runtime target (Static URL or Service-backed EndpointSlice pool).
    pub resolved_target: ResolvedTarget,
    /// Pre-compiled outbound request spec.
    pub compiled_request: CompiledRequest,
    /// Shared health state (used by the active-health task and the dial path).
    pub health: Arc<HealthState>,
    /// Optional sliding-window rate counter for outbound call rate limiting.
    pub rate: Option<SlidingWindowCounter>,
    /// Success criteria (status codes + optional JSON-Pointer body predicates);
    /// `None` = any 2xx status. Used by [`ManagedOutboundClient::classify`] to
    /// classify completed responses — not read directly by `outbound_call`.
    pub success: Option<SuccessSpec>,
    /// Maximum response body bytes to read.
    pub max_response_bytes: usize,
    /// Per-call request timeout.
    pub timeout: Duration,
    /// Optional compiled template rendering a per-request timeout override.
    /// When present and it renders to a parseable, non-zero value, it wins over
    /// `timeout` (clamped to `MAX_OUTBOUND_TIMEOUT_MS`); otherwise `timeout` is used.
    pub timeout_template: Option<CompiledTemplate>,
    /// Static per-request degradation flag from `ExternalHttpConnection.allow_degradation`.
    /// Used directly when `degradation_template` is absent or fails to render to a
    /// usable value; see [`ManagedOutboundClient::effective_allow_degradation`].
    pub allow_degradation: bool,
    /// Optional compiled template rendering a per-request degradation decision.
    /// When present and it renders to exactly "true"/"false", it wins over
    /// `allow_degradation`; otherwise `allow_degradation` is used.
    pub degradation_template: Option<CompiledTemplate>,
    /// Retry configuration. `None` = no retry.
    pub retry: Option<WebhookRetry>,
    /// Egress policy for URL targets. `None` for Service targets, whose addresses
    /// are guarded by `validate_endpoint_backend_addr`'s backend policy instead.
    pub egress_policy: Option<EgressPolicy>,
    /// Passive health-check configuration. `None` = no passive monitoring.
    pub passive_health_check: Option<PassiveHealthCheck>,
    /// Handle for the active-health background task. Aborted on Drop.
    pub health_task: Option<tokio::task::JoinHandle<()>>,
    /// Controller-resolved secret values for `${secretRef:...}` templates.
    /// Carried here so that `build_request` can resolve secret templates at
    /// call time without a separate per-call argument.
    pub resolved_secrets: Option<edgion_resources::resources::common::external_http::ResolvedSecrets>,
}

impl std::fmt::Debug for ManagedOutboundClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedOutboundClient")
            .field("resolved_target", &self.resolved_target)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("timeout", &self.timeout)
            .field("health_task_present", &self.health_task.is_some())
            .finish()
    }
}

impl Drop for ManagedOutboundClient {
    fn drop(&mut self) {
        if let Some(handle) = self.health_task.take() {
            handle.abort();
        }
    }
}

impl ManagedOutboundClient {
    // ============================================================
    // from_parts — primary constructor
    // ============================================================

    /// Build a [`ManagedOutboundClient`] from an [`ExternalHttpConnection`] and an
    /// [`OutboundRequestSpec`].
    ///
    /// * Validates the target via [`validate_connection_target`].
    /// * Builds a `reqwest::Client` from `conn.tls` (falls back to the shared
    ///   global client when TLS is absent).
    /// * Resolves the backend reference into a [`ResolvedTarget`].
    /// * Compiles the request spec via [`CompiledRequest::from_spec`].
    /// * Spawns an active-health probe task when `conn.health_check.active` is set.
    /// * Initialises a rate counter when `conn.rate_limit` is set.
    ///
    /// `timeout_retry_default` is the consumer's default for `retry_on_timeout`
    /// when the operator leaves it unset (`None`) on the resource: webhook
    /// callers pass `false` (non-idempotent POSTs), forward_auth callers pass
    /// `true` (idempotent auth checks).
    ///
    /// Returns `Err` with a human-readable message on any validation or compile failure.
    pub fn from_parts(
        conn: &ExternalHttpConnection,
        request: &OutboundRequestSpec,
        secrets: Option<&ResolvedSecrets>,
        ref_label: &str,
        timeout_retry_default: bool,
    ) -> Result<Self, String> {
        // ── 1. Validate target ─────────────────────────────────────────────
        if let Some(e) = validate_connection_target(conn) {
            return Err(format!("invalid target for '{}': {}", ref_label, e));
        }

        // ── 2. Build the DNS resolver / egress policy for URL targets ──────
        // A URL target is addressed by hostname, so the literal-level guard in
        // `dial_url` cannot see its addresses. `EgressResolver` classifies them at
        // connect time. Service targets keep reqwest's default resolver: their
        // addresses arrive as IP literals from the EndpointSlice store and are
        // already guarded by `dial_url`.
        let egress_policy: Option<EgressPolicy> = match conn.target.kind() {
            Ok(TargetKind::Url { block_private, .. }) => Some(EgressPolicy {
                block_private: block_private.unwrap_or_else(default_block_private),
            }),
            Ok(TargetKind::Service(_)) => None,
            // `validate_connection_target` above already returned on this error.
            Err(e) => return Err(format!("invalid target for '{ref_label}': {e}")),
        };
        let dns_resolver: Option<Arc<dyn reqwest::dns::Resolve>> = egress_policy.map(|policy| {
            Arc::new(EgressResolver::new(
                ResolverSpec::System,
                policy,
                egress_dns_cache_ttl(),
            )) as Arc<dyn reqwest::dns::Resolve>
        });

        // ── 3. Build HTTP client ───────────────────────────────────────────
        // `build_webhook_client` returns None only when neither TLS nor a resolver
        // is needed; a URL target always supplies a resolver, so it never falls back
        // to the shared global client.
        let http_client = match build_webhook_client(&conn.tls, dns_resolver) {
            None => get_http_client_arc(),
            Some(Ok(client)) => Arc::new(client),
            Some(Err(e)) => {
                return Err(format!("failed to build HTTP client for '{}': {}", ref_label, e));
            }
        };

        // ── 4. Resolve target ──────────────────────────────────────────────
        let resolved_target = build_resolved_target(&conn.target, conn.tls.is_some())?;

        // ── 5. Compile request spec ────────────────────────────────────────
        let compiled_request = CompiledRequest::from_spec(request)
            .map_err(|e| format!("failed to compile request spec for '{}': {}", ref_label, e))?;

        // ── 5b. Compile the timeout template (admission already validated syntax
        //        via `validate_connection_target`, but from_parts is also reachable
        //        directly in tests, so compile again defensively) ────────────────
        let timeout_template = conn
            .timeout_ms_template
            .as_deref()
            .map(CompiledTemplate::compile)
            .transpose()
            .map_err(|e| format!("failed to compile timeoutMsTemplate for '{}': {}", ref_label, e))?;

        // ── 5c. Compile the degradation template (admission already validated syntax
        //        via `validate_connection_target`, but from_parts is also reachable
        //        directly in tests, so compile again defensively) ────────────────────
        let degradation_template = conn
            .allow_degradation_template
            .as_deref()
            .map(CompiledTemplate::compile)
            .transpose()
            .map_err(|e| format!("failed to compile allowDegradationTemplate for '{}': {}", ref_label, e))?;

        // ── 6. Health state ────────────────────────────────────────────────
        let health = HealthState::new();

        // ── 7. Rate counter ────────────────────────────────────────────────
        let rate = conn
            .rate_limit
            .as_ref()
            .map(|rl| SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1))));

        // ── 8. Spawn active-health task ────────────────────────────────────
        let health_task = if let Some(active) = conn.health_check.as_ref().and_then(|hc| hc.active.as_ref()) {
            let active_clone = active.clone();
            let health_clone = Arc::clone(&health);
            let client_clone = Arc::clone(&http_client);
            let ref_clone = ref_label.to_string();
            // Build a standalone target for the health task. We re-use the same
            // resolved_target kind but construct a fresh one for the task so that
            // we can move it without cloning the AtomicUsize cursor (which is not
            // Clone). In practice the health task always uses pick_url() which
            // re-reads the global EndpointSlice store each tick.
            let task_target = build_resolved_target(&conn.target, conn.tls.is_some())
                .unwrap_or(ResolvedTarget::Static(String::new()));
            Some(tokio::spawn(async move {
                run_active_health_check(ref_clone, task_target, active_clone, health_clone, client_clone).await;
            }))
        } else {
            None
        };

        // Resolve the per-consumer retry_on_timeout default. `ref_label` cannot
        // carry the consumer identity, so the caller passes it via `timeout_retry_default`.
        let retry = conn.retry.clone().map(|mut r| {
            if r.retry_on_timeout.is_none() {
                r.retry_on_timeout = Some(timeout_retry_default);
            }
            r
        });
        let passive_health_check = conn.health_check.as_ref().and_then(|hc| hc.passive.as_ref()).cloned();

        Ok(ManagedOutboundClient {
            http_client,
            resolved_target,
            compiled_request,
            health,
            rate,
            success: conn.success.clone(),
            max_response_bytes: conn.max_response_bytes,
            timeout: Duration::from_millis(conn.timeout_ms),
            timeout_template,
            allow_degradation: conn.allow_degradation,
            degradation_template,
            retry,
            egress_policy,
            passive_health_check,
            health_task,
            resolved_secrets: secrets.cloned(),
        })
    }

    // ============================================================
    // dial_url — SSRF-guarded URL resolution
    // ============================================================

    /// Resolve the next base URL for an outbound call, applying the IP-literal SSRF guard.
    ///
    /// Returns:
    /// - `Ok(url)` — a `"scheme://host:port"` string safe to dial.
    /// - `Err(NoEndpoint)` — the target currently has no ready backend.
    /// - `Err(SsrfRejected)` — the resolved IP literal is loopback or link-local.
    ///
    /// A DNS hostname skips this guard by design (see [`extract_ip_literal`]); it is
    /// guarded at connect time by [`super::egress::EgressResolver`].
    pub fn dial_url(&self) -> Result<String, ManagedOutboundDialError> {
        let url = self
            .resolved_target
            .pick_url()
            .ok_or(ManagedOutboundDialError::NoEndpoint)?;

        let literal = extract_ip_literal(&url).map_err(|e| {
            ManagedOutboundDialError::SsrfRejected(format!("cannot parse address from URL {url:?}: {e}"))
        })?;

        if let Some(addr) = literal {
            let pingora_addr = pingora_core::protocols::l4::socket::SocketAddr::Inet(addr);
            validate_endpoint_backend_addr(&pingora_addr, false).map_err(|status| {
                ManagedOutboundDialError::SsrfRejected(format!("address {addr} is blocked by SSRF guard: {status:?}"))
            })?;

            // Unspecified (0.0.0.0 / ::) is blocked unconditionally for URL targets,
            // not gated on `blockPrivate`: `validate_endpoint_backend_addr` allows it
            // (cluster-internal Service targets legitimately never see 0.0.0.0), but on
            // Linux a connect to 0.0.0.0 is routed to 127.0.0.1 — a loopback-equivalent
            // bypass of the guard's "loopback always blocked" promise, and a hostile DNS
            // answer of 0.0.0.0 is a rebinding-class bypass. This block only runs when
            // `egress_policy` is set, i.e. for URL targets; Service targets are untouched.
            if self.egress_policy.is_some() && crate::util::net::classify_ip(addr.ip()) == IpClass::Unspecified {
                record_egress_blocked("unspecified");
                return Err(ManagedOutboundDialError::SsrfRejected(format!(
                    "address {addr} is an unspecified address"
                )));
            }

            // A URL target's `blockPrivate` must also apply to IP-literal hosts, which
            // never reach `EgressResolver`. `validate_endpoint_backend_addr` allows
            // RFC1918 by design, so the private check lives here.
            if let Some(policy) = self.egress_policy {
                if policy.block_private {
                    let class = crate::util::net::classify_ip(addr.ip());
                    if matches!(class, IpClass::Private | IpClass::UniqueLocal) {
                        record_egress_blocked("private");
                        return Err(ManagedOutboundDialError::SsrfRejected(format!(
                            "address {addr} is blocked by blockPrivate"
                        )));
                    }
                }
            }
        }

        Ok(url)
    }

    // ============================================================
    // Response classification
    // ============================================================

    /// Classify a completed response, reporting WHICH dimension (status vs.
    /// body) failed. Used by the outbound retry loop to gate
    /// `retry_on_body_failure` on the body dimension only.
    pub fn classify_detail(
        &self,
        resp: &super::call::OutboundResponse,
    ) -> edgion_resources::resources::common::success::SuccessOutcome {
        // Media types are case-insensitive (RFC 7231 §3.1.1.5); lowercase before
        // matching so e.g. "Application/JSON" is still recognized as JSON.
        let is_json = resp
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.to_ascii_lowercase())
            .is_some_and(|ct| ct.starts_with("application/json") || ct.contains("+json"));
        edgion_resources::resources::common::success::classify_detail(
            self.success.as_ref(),
            resp.status,
            &resp.body,
            is_json,
        )
    }

    /// Classify a completed response as success/failure per `self.success`.
    pub fn classify(&self, resp: &super::call::OutboundResponse) -> bool {
        self.classify_detail(resp) == edgion_resources::resources::common::success::SuccessOutcome::Success
    }

    // ============================================================
    // Degradation
    // ============================================================

    /// Effective degradation flag for this call: the template wins when it renders
    /// to exactly "true"/"false"; otherwise the static `allow_degradation`.
    pub fn effective_allow_degradation(&self, session: &dyn crate::plugins::runtime::PluginSession) -> bool {
        use edgion_resources::resources::common::external_http::resolve_bool_template;
        if let Some(tpl) = self.degradation_template.as_ref() {
            if let Ok(rendered) = super::request::evaluate_template(tpl, session, self.resolved_secrets.as_ref()) {
                if let Some(b) = resolve_bool_template(&rendered) {
                    return b;
                }
            }
            crate::observe::metrics::record_outbound_template_fallback("allow_degradation");
        }
        self.allow_degradation
    }

    // ============================================================
    // Test-only constructors
    // ============================================================

    /// Build a minimal client with a `Static` target for unit tests.
    /// No health task is spawned; only the `resolved_target` and a default
    /// `CompiledRequest` are set up.
    #[cfg(test)]
    pub fn test_with_static_target(url: &str) -> Self {
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

        ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Static(url.to_string()),
            compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec::default())
                .expect("default spec must compile"),
            health: HealthState::new(),
            rate: None,
            success: None,
            max_response_bytes: 1024,
            timeout: Duration::from_secs(5),
            timeout_template: None,
            allow_degradation: false,
            degradation_template: None,
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        }
    }
}

// ============================================================
// Internal helpers
// ============================================================

/// Build a [`ResolvedTarget`] from a [`WebhookTarget`].
///
/// For a `Url` target the scheme comes from the URL itself, not from `tls_enabled`:
/// the URL encodes the transport, and the `tls` block only supplies custom CA /
/// client-certificate material. `tls_enabled` still drives the scheme for Service
/// targets, which carry no scheme of their own.
fn build_resolved_target(target: &WebhookTarget, tls_enabled: bool) -> Result<ResolvedTarget, String> {
    match target.kind()? {
        TargetKind::Url { url, .. } => Ok(ResolvedTarget::Static(url.to_string())),
        TargetKind::Service(svc) => {
            let scheme: &'static str = if tls_enabled { "https" } else { "http" };
            let ns = svc.namespace.as_deref().unwrap_or("default");
            // `kind()` has already validated a Service target's name/port/kind, so
            // this unwrap_or_default cannot fire in practice.
            let name = svc.name.as_deref().unwrap_or_default();

            match EdgionService::from_kind(svc.kind.as_ref()) {
                EdgionService::ServiceClusterIp => {
                    let port_suffix = svc.port.map(|p| format!(":{p}")).unwrap_or_default();
                    Ok(ResolvedTarget::Static(format!(
                        "{scheme}://{name}.{ns}.svc.cluster.local{port_suffix}"
                    )))
                }
                EdgionService::Service | EdgionService::ServiceEndpointSlice => Ok(ResolvedTarget::Service {
                    service_key: format!("{ns}/{name}"),
                    port: svc.port,
                    scheme,
                    cursor: AtomicUsize::new(0),
                }),
                EdgionService::Unsupported(k) => Err(format!("unsupported service kind: {k}")),
                EdgionService::ServiceImport => Err("ServiceImport is not supported".to_string()),
                EdgionService::ServiceExternalName => Err("ServiceExternalName is not supported".to_string()),
            }
        }
    }
}

/// Extract an IP-literal socket address from a URL string like `"http://10.0.0.1:8080"`.
///
/// Returns:
/// - `Ok(Some(addr))` — the host is an IP literal; the caller applies the SSRF guard.
/// - `Ok(None)` — the host is a DNS name. IP classification cannot apply, so the
///   caller skips the literal-level guard. Hostname targets are guarded instead by
///   [`super::egress::EgressResolver`], which classifies the addresses actually
///   handed to reqwest.
/// - `Err(msg)` — the URL is malformed.
///
/// Deliberately uses `std::net::SocketAddr` rather than pingora's: pingora's
/// `FromStr` falls back to parsing a Unix-domain-socket pathname when IP parsing
/// fails, which turns every hostname into a `SocketAddr::Unix` that
/// `classify_backend_addr` silently allows.
fn extract_ip_literal(url: &str) -> Result<Option<std::net::SocketAddr>, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL {url:?}: {e}"))?;

    let host = parsed.host_str().ok_or_else(|| format!("URL has no host: {url:?}"))?;

    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("URL has no port and no default for scheme: {url:?}"))?;

    // `Url::host_str` keeps IPv6 literals in their bracketed form (e.g. "[2001:db8::1]"),
    // so strip the brackets before parsing as an IP address. IPv4 literals and DNS
    // names have no brackets, so this is a no-op for them.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);

    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => Ok(Some(std::net::SocketAddr::new(ip, port))),
        Err(_) => Ok(None),
    }
}

// ============================================================
// Active health check task
// ============================================================

/// Standalone active health check loop for [`ManagedOutboundClient`].
///
/// Periodically probes `target.pick_url()/active.path` and updates
/// `health.healthy`. Long-running; exits only when the task is aborted (via
/// `Drop` on the owner).
async fn run_active_health_check(
    ref_label: String,
    target: ResolvedTarget,
    active: ActiveHealthCheck,
    health: Arc<HealthState>,
    client: Arc<reqwest::Client>,
) {
    let interval = Duration::from_secs(active.interval_sec.max(1));
    let timeout = Duration::from_millis(active.timeout_ms.max(100));

    let mut consecutive_failures: u32 = 0;
    let mut consecutive_successes: u32 = 0;

    loop {
        tokio::time::sleep(interval).await;

        let Some(base) = target.pick_url() else {
            // No ready backend yet — skip this tick without resetting counters.
            let _ = &ref_label; // suppress unused-variable lint
            continue;
        };

        let check_url = match &active.path {
            Some(path) => format!("{}{}", base.trim_end_matches('/'), path),
            None => base,
        };

        let result = client.get(&check_url).timeout(timeout).send().await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                consecutive_failures = 0;
                consecutive_successes += 1;
                if consecutive_successes >= active.healthy_threshold {
                    health.healthy.store(true, Ordering::Relaxed);
                }
            }
            _ => {
                consecutive_successes = 0;
                consecutive_failures += 1;
                if consecutive_failures >= active.unhealthy_threshold {
                    health.healthy.store(false, Ordering::Relaxed);
                }
            }
        }
    }
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::external_http::WebhookServiceRef;

    fn url_target(url: &str) -> WebhookTarget {
        WebhookTarget {
            url: Some(url.to_string()),
            ..Default::default()
        }
    }

    /// A URL target resolves to a Static base URL whose scheme comes from the URL,
    /// not from `tls.is_some()`.
    #[test]
    fn build_resolved_target_url_uses_url_scheme() {
        // https URL with no tls block must still be https.
        let t = build_resolved_target(&url_target("https://api.example.com"), false).unwrap();
        match t {
            ResolvedTarget::Static(url) => assert_eq!(url, "https://api.example.com"),
            other => panic!("expected Static, got {other:?}"),
        }

        // http URL must stay http even when a tls block is present (custom CA only).
        let t = build_resolved_target(&url_target("http://api.example.com:8080"), true).unwrap();
        match t {
            ResolvedTarget::Static(url) => assert_eq!(url, "http://api.example.com:8080"),
            other => panic!("expected Static, got {other:?}"),
        }
    }

    /// An invalid URL target surfaces the schema-layer error verbatim.
    #[test]
    fn build_resolved_target_rejects_non_origin_url() {
        let err = build_resolved_target(&url_target("https://api.example.com/v1"), false).unwrap_err();
        assert!(err.contains("origin-only"), "got: {err}");
    }

    /// Service targets are unchanged.
    #[test]
    fn build_resolved_target_service_cluster_ip_unchanged() {
        let target = WebhookTarget {
            service: WebhookServiceRef {
                kind: Some("ServiceClusterIp".to_string()),
                name: Some("auth-svc".to_string()),
                namespace: Some("prod".to_string()),
                port: Some(8080),
                ..Default::default()
            },
            ..Default::default()
        };
        match build_resolved_target(&target, false).unwrap() {
            ResolvedTarget::Static(url) => assert_eq!(url, "http://auth-svc.prod.svc.cluster.local:8080"),
            other => panic!("expected Static, got {other:?}"),
        }
    }

    /// TDD test (written before implementation): Drop aborts the spawned health task.
    ///
    /// RED: fails because ManagedOutboundClient / health_task do not exist yet.
    /// GREEN: passes after from_parts + Drop impl are wired in.
    #[tokio::test]
    async fn drop_aborts_health_task() {
        let client = test_client_with_active_health().await;
        let handle = client.health_task.as_ref().unwrap().abort_handle();
        drop(client);
        tokio::task::yield_now().await;
        assert!(handle.is_finished());
    }

    /// TDD test: dial_url rejects loopback (SSRF guard).
    ///
    /// A ManagedOutboundClient whose resolved_target points to 127.0.0.1:80
    /// must return Err(SsrfRejected) from dial_url.
    #[test]
    fn dial_url_rejects_loopback_target() {
        let client = ManagedOutboundClient::test_with_static_target("http://127.0.0.1:80");
        let result = client.dial_url();
        assert!(
            matches!(result, Err(ManagedOutboundDialError::SsrfRejected(_))),
            "expected SsrfRejected, got {result:?}"
        );
    }

    /// A non-loopback IP address is allowed through dial_url.
    #[test]
    fn dial_url_allows_routable_ip() {
        let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        let result = client.dial_url();
        assert!(
            matches!(result, Ok(ref u) if u == "http://10.0.0.1:8080"),
            "expected Ok with url, got {result:?}"
        );
    }

    /// A Static target with a DNS hostname is not blocked by the literal-level guard.
    /// The skip is intentional: `classify_backend_addr` only acts on IP addresses, and
    /// hostname targets are protected by `EgressResolver` at connect time instead.
    #[test]
    fn dial_url_skips_literal_guard_for_dns_hostname() {
        let client = ManagedOutboundClient::test_with_static_target("http://my-svc.default.svc.cluster.local:8080");
        let result = client.dial_url();
        assert!(
            matches!(result, Ok(ref u) if u == "http://my-svc.default.svc.cluster.local:8080"),
            "DNS hostnames must pass the literal guard unchanged; got {result:?}"
        );
    }

    /// A hostname is not an IP literal: the guard is skipped explicitly, and no
    /// Unix socket address is ever manufactured.
    #[test]
    fn extract_ip_literal_returns_none_for_hostname() {
        let got = extract_ip_literal("http://my-svc.default.svc.cluster.local:8080").unwrap();
        assert!(got.is_none(), "a DNS hostname must yield None, got {got:?}");
    }

    #[test]
    fn extract_ip_literal_parses_ipv4_and_ipv6() {
        let v4 = extract_ip_literal("http://10.0.0.1:8080").unwrap().unwrap();
        assert_eq!(v4.to_string(), "10.0.0.1:8080");

        let v6 = extract_ip_literal("https://[2001:db8::1]:8443").unwrap().unwrap();
        assert_eq!(v6.port(), 8443);
    }

    /// A scheme's default port is used when the URL omits one.
    #[test]
    fn extract_ip_literal_uses_default_port() {
        let addr = extract_ip_literal("https://10.0.0.1").unwrap().unwrap();
        assert_eq!(addr.port(), 443);
    }

    /// dial_url returns NoEndpoint when the Service target has no ready backends.
    #[test]
    fn dial_url_returns_no_endpoint_for_empty_service() {
        use std::sync::atomic::AtomicUsize;
        let client = ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Service {
                service_key: "nonexistent-ns/nonexistent-svc".to_string(),
                port: Some(8080),
                scheme: "http",
                cursor: AtomicUsize::new(0),
            },
            compiled_request: CompiledRequest::from_spec(
                &edgion_resources::resources::common::outbound_request::OutboundRequestSpec::default(),
            )
            .unwrap(),
            health: HealthState::new(),
            rate: None,
            success: None,
            max_response_bytes: 1024,
            timeout: Duration::from_secs(5),
            timeout_template: None,
            allow_degradation: false,
            degradation_template: None,
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        };
        let result = client.dial_url();
        assert!(
            matches!(result, Err(ManagedOutboundDialError::NoEndpoint)),
            "expected NoEndpoint, got {result:?}"
        );
    }

    /// from_parts rejects an invalid target (missing port).
    #[test]
    fn from_parts_rejects_missing_port() {
        use edgion_resources::resources::common::external_http::{
            ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
        };
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("svc".to_string()),
                    namespace: Some("default".to_string()),
                    port: None, // missing port — must be rejected
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let result = ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test", false);
        assert!(result.is_err(), "from_parts must reject missing port");
        let msg = result.unwrap_err();
        assert!(msg.contains("port"), "error should mention port, got: {msg}");
    }

    /// blockPrivate must reject an IP-literal URL target, which never reaches
    /// EgressResolver because hyper-util skips DNS for IP literals.
    #[test]
    fn dial_url_blocks_private_ip_literal_when_policy_set() {
        let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.egress_policy = Some(EgressPolicy { block_private: true });
        match client.dial_url() {
            Err(ManagedOutboundDialError::SsrfRejected(msg)) => {
                assert!(msg.contains("blockPrivate"), "got: {msg}");
            }
            other => panic!("expected SsrfRejected, got {other:?}"),
        }
    }

    /// The same literal passes when blockPrivate is off — matching the resolver path,
    /// which allows private ranges by default.
    #[test]
    fn dial_url_allows_private_ip_literal_by_default() {
        let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.egress_policy = Some(EgressPolicy { block_private: false });
        assert!(client.dial_url().is_ok());
    }

    /// Link-local is rejected regardless of blockPrivate, on both paths.
    #[test]
    fn dial_url_blocks_metadata_ip_literal_regardless_of_policy() {
        let mut client = ManagedOutboundClient::test_with_static_target("http://169.254.169.254:80");
        client.egress_policy = Some(EgressPolicy { block_private: false });
        assert!(matches!(
            client.dial_url(),
            Err(ManagedOutboundDialError::SsrfRejected(_))
        ));
    }

    /// 0.0.0.0 must be rejected regardless of blockPrivate: `validate_endpoint_backend_addr`
    /// does not block Unspecified (Service targets legitimately allow it), so the URL-target
    /// path needs its own unconditional check independent of the `block_private` gate.
    #[test]
    fn dial_url_blocks_unspecified_ip_literal_regardless_of_policy() {
        let mut client = ManagedOutboundClient::test_with_static_target("http://0.0.0.0:8080");
        client.egress_policy = Some(EgressPolicy { block_private: false });
        assert!(matches!(
            client.dial_url(),
            Err(ManagedOutboundDialError::SsrfRejected(_))
        ));
    }

    /// A URL target with no TLS block must still get a dedicated client — otherwise
    /// it would fall back to the shared global client, which carries no EgressResolver.
    #[test]
    fn url_target_without_tls_gets_a_dedicated_client() {
        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                url: Some("https://api.example.com".to_string()),
                ..Default::default()
            },
            tls: None,
            ..Default::default()
        };
        let client =
            ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test/url-hook", false)
                .expect("url target must build");

        // The shared global client is a distinct Arc allocation; a dedicated client
        // must not alias it.
        assert!(
            !Arc::ptr_eq(&client.http_client, &get_http_client_arc()),
            "a URL target must not reuse the shared global client"
        );
    }

    /// A Service target with no TLS block still reuses the shared global client.
    #[test]
    fn service_target_without_tls_reuses_global_client() {
        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    name: Some("auth-svc".to_string()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            tls: None,
            ..Default::default()
        };
        let client =
            ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test/svc-hook", false)
                .expect("service target must build");
        assert!(Arc::ptr_eq(&client.http_client, &get_http_client_arc()));
    }

    /// Helper: a `WebhookRetry` block with every field set explicitly (no `Default`
    /// impl exists), used by the `from_parts` retry-resolution tests below.
    fn retry_block(retry_on_timeout: Option<bool>) -> WebhookRetry {
        use edgion_resources::resources::common::external_http::BackoffPolicy;
        WebhookRetry {
            max_retries: 3,
            retry_delay_ms: 100,
            retry_on_timeout,
            retry_on_connect_error: true,
            retry_on_status: vec![],
            backoff_policy: BackoffPolicy::Exponential,
            max_delay_ms: 10_000,
            jitter: false,
            honor_retry_after: false,
            retry_on_body_failure: false,
        }
    }

    /// Helper: build a valid URL-target `ExternalHttpConnection` carrying the given
    /// retry block, for driving `from_parts`'s retry-resolution logic.
    fn conn_with_retry(retry: WebhookRetry) -> ExternalHttpConnection {
        ExternalHttpConnection {
            target: WebhookTarget {
                url: Some("https://api.example.com".to_string()),
                ..Default::default()
            },
            retry: Some(retry),
            ..Default::default()
        }
    }

    /// `from_parts(..., true)` fills an unset `retry_on_timeout` with `Some(true)` —
    /// this is the forward_auth call-site default (idempotent auth checks).
    #[test]
    fn from_parts_resolves_unset_retry_on_timeout_to_true_default() {
        let conn = conn_with_retry(retry_block(None));
        let client =
            ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test/forward-auth", true)
                .expect("valid URL-target config must build");

        let retry = client.retry.as_ref().expect("retry block must survive from_parts");
        assert_eq!(retry.retry_on_timeout, Some(true));
    }

    /// `from_parts(..., false)` fills an unset `retry_on_timeout` with `Some(false)` —
    /// this is the webhook call-site default (non-idempotent POSTs).
    #[test]
    fn from_parts_resolves_unset_retry_on_timeout_to_false_default() {
        let conn = conn_with_retry(retry_block(None));
        let client =
            ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test/webhook", false)
                .expect("valid URL-target config must build");

        let retry = client.retry.as_ref().expect("retry block must survive from_parts");
        assert_eq!(retry.retry_on_timeout, Some(false));
    }

    /// An operator's explicit `retry_on_timeout` must survive `from_parts` unchanged —
    /// the per-consumer default only fills the gap when the operator leaves it unset.
    #[test]
    fn from_parts_does_not_clobber_explicit_retry_on_timeout() {
        let conn = conn_with_retry(retry_block(Some(true)));
        // Consumer default is `false`; the explicit `Some(true)` on the resource
        // must win regardless.
        let client =
            ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test/webhook", false)
                .expect("valid URL-target config must build");

        let retry = client.retry.as_ref().expect("retry block must survive from_parts");
        assert_eq!(retry.retry_on_timeout, Some(true));
    }

    /// Helper: build a ManagedOutboundClient with an active health check config so
    /// that from_parts spawns a long-running health task.
    async fn test_client_with_active_health() -> ManagedOutboundClient {
        use edgion_resources::resources::common::external_http::{
            ActiveHealthCheck, ExternalHttpConnection, WebhookHealthCheck, WebhookServiceRef, WebhookTarget,
        };
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: Some("Service".to_string()),
                    name: Some("test-svc".to_string()),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            health_check: Some(WebhookHealthCheck {
                active: Some(ActiveHealthCheck {
                    path: Some("/healthz".to_string()),
                    interval_sec: 3600, // very long — task will not exit naturally
                    timeout_ms: 2000,
                    healthy_threshold: 1,
                    unhealthy_threshold: 3,
                }),
                passive: None,
            }),
            ..Default::default()
        };
        let request = OutboundRequestSpec::default();

        ManagedOutboundClient::from_parts(&conn, &request, None, "test/client", false)
            .expect("from_parts must succeed for valid config")
    }

    // ============================================================
    // effective_allow_degradation
    // ============================================================

    fn compile_tpl(s: &str) -> CompiledTemplate {
        CompiledTemplate::compile(s).unwrap()
    }

    /// TDD: a `degradation_template` that renders "true" wins over a static
    /// `allow_degradation: false`.
    #[test]
    fn effective_allow_degradation_template_wins_over_static() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.allow_degradation = false;
        client.degradation_template = Some(compile_tpl("${ctx:is_staging}"));

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "is_staging")
            .return_const(Some("true".to_string()));

        assert!(
            client.effective_allow_degradation(&session),
            "template rendering \"true\" must win over the static false"
        );
    }

    /// TDD: a `degradation_template` that renders a non-strict token ("1") falls
    /// back to the static `allow_degradation` — this proves the strict grammar
    /// (only exact "true"/"false" are accepted).
    #[test]
    fn effective_allow_degradation_falls_back_on_non_strict_render() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.allow_degradation = false;
        client.degradation_template = Some(compile_tpl("${ctx:is_staging}"));

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "is_staging")
            .return_const(Some("1".to_string()));

        assert!(
            !client.effective_allow_degradation(&session),
            "a non-strict render (\"1\") must fall back to the static allow_degradation"
        );
    }

    /// No template set: the static `allow_degradation` is used directly.
    #[test]
    fn effective_allow_degradation_uses_static_when_no_template() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.allow_degradation = true;
        client.degradation_template = None;

        let session = MockPluginSession::new();
        assert!(client.effective_allow_degradation(&session));
    }
}
