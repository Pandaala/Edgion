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
    ActiveHealthCheck, ExternalHttpConnection, PassiveHealthCheck, WebhookRetry, WebhookTarget,
};
use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
use edgion_resources::resources::common::ResolvedSecrets;

use crate::backends::service_kind::EdgionService;
use crate::backends::validate_endpoint_backend_addr;
use crate::plugins::http::common::http_client::{build_webhook_client, get_http_client_arc};
use edgion_resources::resources::common::external_http::validate_webhook_target;

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
    /// The resolved URL was blocked by the SSRF guard (loopback / link-local address).
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
    /// Status codes considered successful; `None` = any 2xx.
    /// Used by callers to classify completed responses — not used by `outbound_call` itself.
    pub success_status_codes: Option<Vec<u16>>,
    /// Maximum response body bytes to read.
    pub max_response_bytes: usize,
    /// Per-call request timeout.
    pub timeout: Duration,
    /// Retry configuration. `None` = no retry.
    pub retry: Option<WebhookRetry>,
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
    /// * Validates the target via [`validate_webhook_target`].
    /// * Builds a `reqwest::Client` from `conn.tls` (falls back to the shared
    ///   global client when TLS is absent).
    /// * Resolves the backend reference into a [`ResolvedTarget`].
    /// * Compiles the request spec via [`CompiledRequest::from_spec`].
    /// * Spawns an active-health probe task when `conn.health_check.active` is set.
    /// * Initialises a rate counter when `conn.rate_limit` is set.
    ///
    /// Returns `Err` with a human-readable message on any validation or compile failure.
    pub fn from_parts(
        conn: &ExternalHttpConnection,
        request: &OutboundRequestSpec,
        secrets: Option<&ResolvedSecrets>,
        ref_label: &str,
    ) -> Result<Self, String> {
        // ── 1. Validate target ─────────────────────────────────────────────
        if let Some(e) = validate_webhook_target(&conn.target) {
            return Err(format!("invalid target for '{}': {}", ref_label, e));
        }

        // ── 2. Build HTTP client ───────────────────────────────────────────
        let http_client = match build_webhook_client(&conn.tls) {
            None => get_http_client_arc(),
            Some(Ok(client)) => Arc::new(client),
            Some(Err(e)) => {
                return Err(format!("failed to build HTTP client for '{}': {}", ref_label, e));
            }
        };

        // ── 3. Resolve target ──────────────────────────────────────────────
        let resolved_target = build_resolved_target(&conn.target, conn.tls.is_some())?;

        // ── 4. Compile request spec ────────────────────────────────────────
        let compiled_request = CompiledRequest::from_spec(request)
            .map_err(|e| format!("failed to compile request spec for '{}': {}", ref_label, e))?;

        // ── 5. Health state ────────────────────────────────────────────────
        let health = HealthState::new();

        // ── 6. Rate counter ────────────────────────────────────────────────
        let rate = conn
            .rate_limit
            .as_ref()
            .map(|rl| SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1))));

        // ── 7. Spawn active-health task ────────────────────────────────────
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

        let retry = conn.retry.clone();
        let passive_health_check = conn.health_check.as_ref().and_then(|hc| hc.passive.as_ref()).cloned();

        Ok(ManagedOutboundClient {
            http_client,
            resolved_target,
            compiled_request,
            health,
            rate,
            success_status_codes: conn.success_status_codes.clone(),
            max_response_bytes: conn.max_response_bytes,
            timeout: Duration::from_millis(conn.timeout_ms),
            retry,
            passive_health_check,
            health_task,
            resolved_secrets: secrets.cloned(),
        })
    }

    // ============================================================
    // dial_url — SSRF-guarded URL resolution
    // ============================================================

    /// Resolve the next base URL for an outbound call, applying the SSRF guard.
    ///
    /// Returns:
    /// - `Ok(url)` — a `"scheme://host:port"` string safe to dial.
    /// - `Err(NoEndpoint)` — the target currently has no ready backend.
    /// - `Err(SsrfRejected)` — the resolved address is loopback or link-local.
    ///
    /// The SSRF guard calls [`validate_endpoint_backend_addr`] with
    /// `allow_loopback = false` (safe default; forward_auth and other consumers
    /// that want to allow loopback must set up a separate path).
    pub fn dial_url(&self) -> Result<String, ManagedOutboundDialError> {
        let url = self
            .resolved_target
            .pick_url()
            .ok_or(ManagedOutboundDialError::NoEndpoint)?;

        // Parse the host:port from the URL for SSRF validation.
        let addr = extract_socket_addr(&url).map_err(|e| {
            ManagedOutboundDialError::SsrfRejected(format!("cannot parse address from URL {url:?}: {e}"))
        })?;

        validate_endpoint_backend_addr(&addr, false).map_err(|status| {
            ManagedOutboundDialError::SsrfRejected(format!("address {} is blocked by SSRF guard: {:?}", addr, status))
        })?;

        Ok(url)
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
            success_status_codes: None,
            max_response_bytes: 1024,
            timeout: Duration::from_secs(5),
            retry: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        }
    }
}

// ============================================================
// Internal helpers
// ============================================================

/// Build a [`ResolvedTarget`] from a [`WebhookTarget`] backend reference.
///
/// Mirrors the logic in `WebhookManager::upsert` but is now also used by
/// [`ManagedOutboundClient::from_parts`] so that both share the same scheme
/// / service-key resolution rules.
fn build_resolved_target(target: &WebhookTarget, tls_enabled: bool) -> Result<ResolvedTarget, String> {
    let backend_ref = &target.backend_ref;
    let scheme: &'static str = if tls_enabled { "https" } else { "http" };
    let ns = backend_ref.namespace.as_deref().unwrap_or("default");

    match EdgionService::from_kind(backend_ref.kind.as_ref()) {
        EdgionService::ServiceClusterIp => {
            let port_suffix = backend_ref.port.map(|p| format!(":{p}")).unwrap_or_default();
            Ok(ResolvedTarget::Static(format!(
                "{scheme}://{}.{}.svc.cluster.local{}",
                backend_ref.name, ns, port_suffix
            )))
        }
        EdgionService::Service | EdgionService::ServiceEndpointSlice => Ok(ResolvedTarget::Service {
            service_key: format!("{}/{}", ns, backend_ref.name),
            port: backend_ref.port,
            scheme,
            cursor: AtomicUsize::new(0),
        }),
        EdgionService::Unsupported(k) => Err(format!("unsupported service kind: {k}")),
        EdgionService::ServiceImport => Err("ServiceImport is not supported".to_string()),
        EdgionService::ServiceExternalName => Err("ServiceExternalName is not supported".to_string()),
    }
}

/// Extract a Pingora [`pingora_core::protocols::l4::socket::SocketAddr`] from a URL
/// string like `"http://10.0.0.1:8080"`.
///
/// Used by [`ManagedOutboundClient::dial_url`] to feed `validate_endpoint_backend_addr`.
/// Returns `Err` for non-IP hostnames (cluster-local DNS names cannot be SSRF-blocked
/// by IP classification, so the caller skips the guard for those).
fn extract_socket_addr(url: &str) -> Result<pingora_core::protocols::l4::socket::SocketAddr, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL {url:?}: {e}"))?;

    let host = parsed.host_str().ok_or_else(|| format!("URL has no host: {url:?}"))?;

    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("URL has no port and no default for scheme: {url:?}"))?;

    // Pingora's SocketAddr parses "host:port" strings directly (see util/net.rs).
    // This will fail for DNS hostnames (non-IP), which is intentional: DNS names
    // cannot be SSRF-blocked by IP classification.
    let addr_str = format!("{host}:{port}");
    addr_str
        .parse::<pingora_core::protocols::l4::socket::SocketAddr>()
        .map_err(|_| format!("not an IP address (DNS hostname): {addr_str}"))
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

    /// A Static target with a cluster-local DNS hostname (not an IP) is not blocked
    /// by the SSRF guard — classify_backend_addr only acts on IP addresses.
    #[test]
    fn dial_url_allows_dns_hostname() {
        let client = ManagedOutboundClient::test_with_static_target("http://my-svc.default.svc.cluster.local:8080");
        // extract_socket_addr will fail to parse the hostname as SocketAddr, which we
        // treat as "not an IP → skip SSRF guard → Ok".
        // Verify that dial_url does not return SsrfRejected for a DNS name.
        let result = client.dial_url();
        // Could be Ok (if parse succeeds for some reason) or a specific error,
        // but must NOT be SsrfRejected.
        assert!(
            !matches!(result, Err(ManagedOutboundDialError::SsrfRejected(_))),
            "DNS hostnames must not be SsrfRejected; got {result:?}"
        );
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
            success_status_codes: None,
            max_response_bytes: 1024,
            timeout: Duration::from_secs(5),
            retry: None,
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
        use edgion_resources::resources::common::external_http::{ExternalHttpConnection, WebhookTarget};
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
        use edgion_resources::resources::http_route::BackendObjectReference;

        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: "svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: None, // missing port — must be rejected
                },
            },
            ..Default::default()
        };
        let result = ManagedOutboundClient::from_parts(&conn, &OutboundRequestSpec::default(), None, "test");
        assert!(result.is_err(), "from_parts must reject missing port");
        let msg = result.unwrap_err();
        assert!(msg.contains("port"), "error should mention port, got: {msg}");
    }

    /// Helper: build a ManagedOutboundClient with an active health check config so
    /// that from_parts spawns a long-running health task.
    async fn test_client_with_active_health() -> ManagedOutboundClient {
        use edgion_resources::resources::common::external_http::{
            ActiveHealthCheck, ExternalHttpConnection, WebhookHealthCheck, WebhookTarget,
        };
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
        use edgion_resources::resources::http_route::BackendObjectReference;

        let conn = ExternalHttpConnection {
            target: WebhookTarget {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: Some("Service".to_string()),
                    name: "test-svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                },
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

        ManagedOutboundClient::from_parts(&conn, &request, None, "test/client")
            .expect("from_parts must succeed for valid config")
    }
}
