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
    validate_active_health_path, ActiveHealthCheck, ExternalHttpConnection, PassiveHealthCheck, TargetKind,
    WebhookRetry, WebhookTarget,
};
use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
use edgion_resources::resources::common::request_template::CompiledTemplate;
use edgion_resources::resources::common::success::SuccessSpec;
use edgion_resources::resources::common::ResolvedSecrets;

use edgion_common::dns::ResolverSpec;

use crate::backends::service_kind::ServiceBackendKind;
use crate::backends::validate_endpoint_backend_addr;
use crate::observe::metrics::record_egress_blocked;
use crate::plugins::http::common::http_client::{
    build_webhook_client, build_webhook_client_with_tls_selection, get_http_client_arc,
};
use crate::util::net::IpClass;
use edgion_common::link_sys_core::outbound_tls::{
    validate_active_tls_inheritance, validate_local_tls_selection, GlobalOutboundTlsSnapshot,
    ValidatedLocalTlsSelection,
};
use edgion_resources::resources::common::external_http::validate_connection_target;
use edgion_resources::resources::link_sys::SystemConfig;
use edgion_resources::resources::refs::link_sys_fields::BoundLinkSysConfig;

use super::call::replace_url_host;
use super::egress::{default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver};
use super::pinned_resolver::AnchorPinnedResolver;
use super::request::CompiledRequest;
use super::target::ResolvedTarget;
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;

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
    /// Process-monotonic passive-recovery gate baseline, set when the circuit
    /// opens and advanced when a half-open probe is admitted.
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

// The raw branch remains only for consumers awaiting their owning migration.
// Webhook always enters through its actual LinkSys binding.
enum ClientTls<'a, 'owner> {
    Raw,
    Owned(&'a ValidatedLocalTlsSelection<'owner>, &'a GlobalOutboundTlsSnapshot),
}

// Cold readiness only: actual owning adapters supply both fields. Presence is
// independent of Secret authorization and accepts a resolved empty value.
fn has_unresolved_template_secrets(request: &OutboundRequestSpec, resolved: Option<&ResolvedSecrets>) -> bool {
    request
        .collect_secret_ref_names()
        .iter()
        .any(|key| !resolved.is_some_and(|values| values.0.contains_key(key)))
}

/// Cold owning input shared by the cache gate and final client construction.
/// Neither configuration nor TLS proof can be replaced by a detached caller.
pub(crate) struct PreparedManagedHttp<'a> {
    field: &'a edgion_resources::resources::refs::plugin_fields::BoundManagedHttpConfig<'a>,
    proof: ValidatedLocalTlsSelection<'a>,
    defaults: &'a GlobalOutboundTlsSnapshot,
}

impl<'a> PreparedManagedHttp<'a> {
    pub(crate) fn new(
        field: &'a edgion_resources::resources::refs::plugin_fields::BoundManagedHttpConfig<'a>,
        defaults: &'a GlobalOutboundTlsSnapshot,
    ) -> Result<Self, String> {
        if has_unresolved_template_secrets(field.request(), field.resolved_secrets()) {
            return Err("managed HTTP template references are unresolved".into());
        }
        let selection = field.outbound_tls_selection().map_err(|error| error.to_string())?;
        let proof = validate_local_tls_selection(selection).map_err(|error| error.to_string())?;
        validate_active_tls_inheritance(&proof, defaults).map_err(|error| error.to_string())?;
        Ok(Self { field, proof, defaults })
    }

    pub(crate) fn plugin(&self) -> &edgion_resources::resources::edgion_plugins::EdgionPlugin {
        self.field.plugin()
    }

    pub(crate) fn build_client(&self) -> Result<ManagedOutboundClient, String> {
        use edgion_resources::resources::edgion_plugins::EdgionPlugin;
        let (label, retry_timeout, force_post) = match self.plugin() {
            EdgionPlugin::ForwardAuth(_) => ("ForwardAuth", true, false),
            EdgionPlugin::Guardrail(_) => ("Guardrail decision service", false, true),
            EdgionPlugin::AiGuard(_) => ("AiGuard decision service", false, true),
            _ => return Err("invalid managed HTTP plugin variant".into()),
        };
        let request = if force_post {
            let mut request = self.field.request().clone();
            request.method = Some(edgion_resources::resources::common::MethodSpec {
                template: "POST".into(),
            });
            std::borrow::Cow::Owned(request)
        } else {
            std::borrow::Cow::Borrowed(self.field.request())
        };
        ManagedOutboundClient::build(
            self.field.connection(),
            &request,
            self.field.resolved_secrets(),
            label,
            retry_timeout,
            Some(self.field.owner_namespace()),
            ClientTls::Owned(&self.proof, self.defaults),
        )
    }
}

/// Internal Route auth retains its exact effective field through construction.
pub(crate) struct PreparedRouteAuth<'a> {
    field: &'a edgion_resources::resources::refs::route_auth_fields::EffectiveRouteAuthField<'a>,
    proof: ValidatedLocalTlsSelection<'a>,
    defaults: &'a GlobalOutboundTlsSnapshot,
}
impl<'a> PreparedRouteAuth<'a> {
    pub(crate) fn new(
        field: &'a edgion_resources::resources::refs::route_auth_fields::EffectiveRouteAuthField<'a>,
        defaults: &'a GlobalOutboundTlsSnapshot,
    ) -> Result<Self, String> {
        if let Some(error) = field.resolution_error() {
            return Err(error.to_string());
        }
        let config = field.config();
        if has_unresolved_template_secrets(&config.request, config.resolved_secrets.as_ref()) {
            return Err("HTTPRoute authentication template references are unresolved".into());
        }
        let selection = field.outbound_tls_selection().map_err(|error| error.to_string())?;
        let proof = validate_local_tls_selection(selection).map_err(|error| error.to_string())?;
        validate_active_tls_inheritance(&proof, defaults).map_err(|error| error.to_string())?;
        Ok(Self { field, proof, defaults })
    }
    pub(crate) fn config(&self) -> &edgion_resources::resources::edgion_plugins::ForwardAuthConfig {
        self.field.config()
    }
    pub(crate) fn build_client(&self) -> Result<ManagedOutboundClient, String> {
        let config = self.config();
        ManagedOutboundClient::build(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            "ForwardAuth",
            true,
            Some(self.field.namespace()),
            ClientTls::Owned(&self.proof, self.defaults),
        )
    }
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
        self.stop_health_check();
    }
}

impl ManagedOutboundClient {
    /// Stop background probes without interrupting callers holding this client.
    /// Repeated calls are harmless; task completion does not gate replacement.
    pub(crate) fn stop_health_check(&self) {
        if let Some(handle) = &self.health_task {
            handle.abort();
        }
    }

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
        Self::build(
            conn,
            request,
            secrets,
            ref_label,
            timeout_retry_default,
            None,
            ClientTls::Raw,
        )
    }

    /// Prepare only the Webhook variant borrowed from its actual LinkSys.
    pub(crate) fn from_webhook(
        binding: &BoundLinkSysConfig<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
        ref_label: &str,
    ) -> Result<Self, String> {
        let source = binding.source().map_err(|error| error.to_string())?;
        let SystemConfig::Webhook(config) = binding.config() else {
            return Err("managed Webhook construction requires the actual Webhook provider".into());
        };
        let selection = binding.outbound_tls_selection().map_err(|error| error.to_string())?;
        let proof = validate_local_tls_selection(selection).map_err(|error| error.to_string())?;
        validate_active_tls_inheritance(&proof, defaults).map_err(|error| error.to_string())?;
        Self::build(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            ref_label,
            false,
            source.scope().namespace(),
            ClientTls::Owned(&proof, defaults),
        )
    }

    fn build(
        conn: &ExternalHttpConnection,
        request: &OutboundRequestSpec,
        secrets: Option<&ResolvedSecrets>,
        ref_label: &str,
        timeout_retry_default: bool,
        owner_namespace: Option<&str>,
        tls: ClientTls<'_, '_>,
    ) -> Result<Self, String> {
        // ── 1. Validate target ─────────────────────────────────────────────
        if let Some(e) = validate_connection_target(conn) {
            return Err(format!("invalid target for '{}': {}", ref_label, e));
        }

        // ── 2. Egress policy for URL targets ───────────────────────────────
        let target_kind = match conn.target.kind() {
            Ok(k) => k,
            // `validate_connection_target` above already returned on this error.
            Err(e) => return Err(format!("invalid target for '{ref_label}': {e}")),
        };

        let egress_policy: Option<EgressPolicy> = match &target_kind {
            TargetKind::Url { block_private, .. } => Some(EgressPolicy {
                block_private: block_private.unwrap_or_else(default_block_private),
            }),
            TargetKind::Service(_) => None,
        };

        // ── 3. Resolve target ──────────────────────────────────────────────
        // Resolved before the DNS resolver / client are built so that resolver
        // selection (step 4) can key off `ResolvedTarget` itself rather than a
        // second, independently-derived classification of the target kind.
        let tls_active = match &tls {
            ClientTls::Raw => conn.tls.is_some(),
            ClientTls::Owned(proof, _) => proof.active_field().is_some(),
        };
        let resolved_target = build_resolved_target_in_namespace(&conn.target, tls_active, owner_namespace)?;

        // ── 4. Build the DNS resolver: EgressResolver for URL targets, or the
        //      pinned resolver for a Service-pool anchor ────────────────────
        // A URL target is addressed by hostname, so the literal-level guard in
        // `dial_url` cannot see its addresses. `EgressResolver` classifies them at
        // connect time.
        //
        // A Service-pool target (`ResolvedTarget::Service`, i.e. the
        // `Service`/`ServiceEndpointSlice` sub-kinds) ALWAYS gets
        // `AnchorPinnedResolver`, regardless of whether an `upstreamAuthority`
        // policy currently exists for it. This is deliberate: `outbound_call`
        // decides, per request, whether to rewrite the URL host to a
        // request-composed authority via a fresh snapshot `get_effective`
        // lookup. If resolver installation here were gated on that SAME lookup
        // at build time, a client built before a policy existed would carry no
        // pinned resolver — and a policy attached later would make
        // `outbound_call` rewrite the host to a request-influenced hostname
        // with nothing but the system DNS resolver behind it (a split-brain
        // SSRF: two decisions about the same store, made at two different
        // times, can disagree). Installing the resolver unconditionally for
        // every Service-pool target closes that window structurally: "has a
        // pinned resolver" and "`authority_service_key()` is `Some`" are now
        // provably the same set, since both are derived from `resolved_target`
        // being `ResolvedTarget::Service`.
        //
        // It costs nothing when no policy is active: the URL host stays an IP
        // literal from `pick_url()`, and reqwest/hyper-util skips the injected
        // resolver entirely for IP-literal hosts (see the `dial_url` /
        // `extract_ip_literal` comments below) — the resolver only does
        // anything once `call.rs` rewrites the host to a real hostname.
        //
        // `ServiceClusterIp` is NOT part of this set: it resolves to
        // `ResolvedTarget::Static("http://name.ns.svc.cluster.local:port")`, and
        // `authority_service_key()` returns `None` for `Static` — the host swap
        // in `call.rs` never engages for it. A pinned resolver here would
        // wrongly resolve the cluster.local name to pod IPs, bypassing the
        // ClusterIP that mode explicitly wants, so it keeps the default resolver.
        let dns_resolver: Option<Arc<dyn reqwest::dns::Resolve>> = match (&target_kind, &resolved_target) {
            (TargetKind::Url { .. }, _) => egress_policy.map(|policy| {
                Arc::new(EgressResolver::new(
                    ResolverSpec::System,
                    policy,
                    egress_dns_cache_ttl(),
                )) as Arc<dyn reqwest::dns::Resolve>
            }),
            (TargetKind::Service(_), ResolvedTarget::Service { service_key, port, .. }) => {
                Some(Arc::new(AnchorPinnedResolver::new(service_key.clone(), *port)) as Arc<dyn reqwest::dns::Resolve>)
            }
            (TargetKind::Service(_), ResolvedTarget::Static(_)) => None,
        };

        // ── 5. Build HTTP client ───────────────────────────────────────────
        // `build_webhook_client` returns None only when neither TLS nor a resolver
        // is needed; a URL target and a Service-pool target both always supply a
        // resolver (steps 2-4 above), so only a `ServiceClusterIp` (`Static`)
        // target with no TLS block falls back to the shared global client.
        let prepared_client = match tls {
            ClientTls::Raw => build_webhook_client(&conn.tls, dns_resolver),
            ClientTls::Owned(proof, defaults) => build_webhook_client_with_tls_selection(proof, defaults, dns_resolver),
        };
        let http_client = match prepared_client {
            None => get_http_client_arc(),
            Some(Ok(client)) => Arc::new(client),
            Some(Err(e)) => {
                return Err(format!("failed to build HTTP client for '{}': {}", ref_label, e));
            }
        };

        // ── 6. Compile request spec ────────────────────────────────────────
        let compiled_request = CompiledRequest::from_spec(request)
            .map_err(|e| format!("failed to compile request spec for '{}': {}", ref_label, e))?;

        // ── 6b. Compile the timeout template (admission already validated syntax
        //        via `validate_connection_target`, but from_parts is also reachable
        //        directly in tests, so compile again defensively) ────────────────
        let timeout_template = conn
            .timeout_ms_template
            .as_deref()
            .map(CompiledTemplate::compile)
            .transpose()
            .map_err(|e| format!("failed to compile timeoutMsTemplate for '{}': {}", ref_label, e))?;

        // ── 7. Health state ────────────────────────────────────────────────
        let health = HealthState::new();

        // ── 8. Rate counter ────────────────────────────────────────────────
        let rate = conn
            .rate_limit
            .as_ref()
            .map(|rl| SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1))));

        let passive_health_check = conn.health_check.as_ref().and_then(|hc| hc.passive.as_ref()).cloned();

        // ── 9. Spawn active-health task ────────────────────────────────────
        let health_task = if let Some(active) = conn.health_check.as_ref().and_then(|hc| hc.active.as_ref()) {
            let active_clone = active.clone();
            let health_clone = Arc::clone(&health);
            let client_clone = Arc::clone(&http_client);
            let has_passive_health = passive_health_check.is_some();
            // Build a standalone target for the health task. We re-use the same
            // resolved_target kind but construct a fresh one for the task so that
            // we can move it without cloning the AtomicUsize cursor (which is not
            // Clone). In practice the health task always uses pick_url() which
            // re-reads the global EndpointSlice store each tick.
            let task_target = build_resolved_target_in_namespace(&conn.target, tls_active, owner_namespace)
                .unwrap_or(ResolvedTarget::Static(String::new()));
            Some(tokio::spawn(async move {
                run_active_health_check(
                    task_target,
                    active_clone,
                    health_clone,
                    client_clone,
                    egress_policy,
                    has_passive_health,
                )
                .await;
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

        validate_literal_dial_url(&url, self.egress_policy)?;

        Ok(url)
    }

    /// The `service_key` when this client's target is a Service anchor, else
    /// `None`. Used by `outbound_call` to look up an `upstreamAuthority` policy
    /// for this client without re-matching `ResolvedTarget` at the call site.
    pub fn authority_service_key(&self) -> Option<&str> {
        match &self.resolved_target {
            ResolvedTarget::Service { service_key, .. } => Some(service_key.as_str()),
            ResolvedTarget::Static(_) => None,
        }
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
fn build_resolved_target_in_namespace(
    target: &WebhookTarget,
    tls_enabled: bool,
    owner_namespace: Option<&str>,
) -> Result<ResolvedTarget, String> {
    match target.kind()? {
        TargetKind::Url { url, .. } => Ok(ResolvedTarget::Static(url.to_string())),
        TargetKind::Service(svc) => {
            let scheme: &'static str = if tls_enabled { "https" } else { "http" };
            let ns = svc.namespace.as_deref().or(owner_namespace).unwrap_or("default");
            // `kind()` has already validated a Service target's name/port/kind, so
            // this unwrap_or_default cannot fire in practice.
            let name = svc.name.as_deref().unwrap_or_default();

            match ServiceBackendKind::from_kind(svc.kind.as_ref()) {
                ServiceBackendKind::ServiceClusterIp => {
                    let port_suffix = svc.port.map(|p| format!(":{p}")).unwrap_or_default();
                    Ok(ResolvedTarget::Static(format!(
                        "{scheme}://{name}.{ns}.svc.cluster.local{port_suffix}"
                    )))
                }
                ServiceBackendKind::Service | ServiceBackendKind::ServiceEndpointSlice => Ok(ResolvedTarget::Service {
                    service_key: format!("{ns}/{name}"),
                    port: svc.port,
                    scheme,
                    cursor: AtomicUsize::new(0),
                }),
                ServiceBackendKind::Unsupported(k) => Err(format!("unsupported service kind: {k}")),
                ServiceBackendKind::ServiceImport => Err("ServiceImport is not supported".to_string()),
                ServiceBackendKind::ServiceExternalName => Err("ServiceExternalName is not supported".to_string()),
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
pub(crate) fn extract_ip_literal(url: &str) -> Result<Option<std::net::SocketAddr>, String> {
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

/// Apply the final IP-literal safety policy for one outbound URL.
///
/// DNS names deliberately return success here: URL targets resolve through
/// `EgressResolver`, while Service authority names resolve through
/// `AnchorPinnedResolver`. Both classify the actual connector addresses.
fn validate_literal_dial_url(url: &str, egress_policy: Option<EgressPolicy>) -> Result<(), ManagedOutboundDialError> {
    let literal = extract_ip_literal(url)
        .map_err(|e| ManagedOutboundDialError::SsrfRejected(format!("cannot parse address from URL {url:?}: {e}")))?;

    let Some(addr) = literal else {
        return Ok(());
    };

    let pingora_addr = pingora_core::protocols::l4::socket::SocketAddr::Inet(addr);
    validate_endpoint_backend_addr(&pingora_addr, false).map_err(|status| {
        ManagedOutboundDialError::SsrfRejected(format!("address {addr} is blocked by SSRF guard: {status:?}"))
    })?;

    // Unspecified addresses are rejected for free-form URL targets. Service
    // targets keep the canonical endpoint policy, which allows cluster-internal
    // address classes other than loopback and link-local.
    if egress_policy.is_some() && crate::util::net::classify_ip(addr.ip()) == IpClass::Unspecified {
        record_egress_blocked("unspecified");
        return Err(ManagedOutboundDialError::SsrfRejected(format!(
            "address {addr} is an unspecified address"
        )));
    }

    // IP-literal URL hosts never reach EgressResolver, so enforce the URL
    // target's optional private-address policy at this final-dial seam.
    if egress_policy.is_some_and(|policy| policy.block_private)
        && matches!(
            crate::util::net::classify_ip(addr.ip()),
            IpClass::Private | IpClass::UniqueLocal
        )
    {
        record_egress_blocked("private");
        return Err(ManagedOutboundDialError::SsrfRejected(format!(
            "address {addr} is blocked by blockPrivate"
        )));
    }

    Ok(())
}

/// Compose and validate the URL for one active-health probe.
///
/// The health path is parsed as a URL reference before its parsed path and
/// query are applied. Raw concatenation is intentionally avoided: special-URL
/// backslash and scheme-relative forms can otherwise replace the authority.
fn prepare_active_health_url(
    base: &str,
    path: Option<&str>,
    egress_policy: Option<EgressPolicy>,
) -> Result<reqwest::Url, String> {
    let mut composed = reqwest::Url::parse(base).map_err(|e| format!("invalid active-health base URL: {e}"))?;
    let original_scheme = composed.scheme().to_string();
    let original_host = composed.host_str().map(str::to_string);
    let original_port = composed.port_or_known_default();

    if !composed.username().is_empty() || composed.password().is_some() {
        return Err("active-health base URL must not contain credentials".to_string());
    }

    if let Some(raw_path) = path {
        validate_active_health_path(raw_path).map_err(|e| format!("invalid active-health path: {e}"))?;
        let reference_base = reqwest::Url::parse("https://health-check.invalid/")
            .map_err(|e| format!("invalid active-health validation base: {e}"))?;
        let reference = reference_base
            .join(raw_path)
            .map_err(|e| format!("invalid active-health path reference: {e}"))?;
        composed.set_path(reference.path());
        composed.set_query(reference.query());

        if composed.path() != reference.path() || composed.query() != reference.query() {
            return Err("active-health path or query changed during URL composition".to_string());
        }
    }
    composed.set_fragment(None);

    if composed.scheme() != original_scheme
        || composed.host_str() != original_host.as_deref()
        || composed.port_or_known_default() != original_port
        || !composed.username().is_empty()
        || composed.password().is_some()
    {
        return Err("active-health path changed the target authority".to_string());
    }

    validate_literal_dial_url(composed.as_str(), egress_policy).map_err(|e| e.to_string())?;
    Ok(composed)
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
    target: ResolvedTarget,
    active: ActiveHealthCheck,
    health: Arc<HealthState>,
    client: Arc<reqwest::Client>,
    egress_policy: Option<EgressPolicy>,
    has_passive_health: bool,
) {
    let interval = Duration::from_secs(active.interval_sec.max(1));
    let timeout = Duration::from_millis(active.timeout_ms.max(100));

    // Fixed for this task's lifetime, mirroring `ManagedOutboundClient::authority_service_key`:
    // only a `ResolvedTarget::Service` anchor can carry an `upstreamAuthority` policy.
    let service_key: Option<String> = match &target {
        ResolvedTarget::Service { service_key, .. } => Some(service_key.clone()),
        ResolvedTarget::Static(_) => None,
    };

    let mut consecutive_failures: u32 = 0;
    let mut consecutive_successes: u32 = 0;

    loop {
        tokio::time::sleep(interval).await;

        let success = match target.pick_url() {
            Some(base) => {
                // A Service anchor carrying an `upstreamAuthority` policy
                // dispatches by Host/SNI. The active probe has no request to
                // render a label from, so it uses the static healthCheckHost.
                // The lookup is refreshed every tick so policy hot reload takes
                // effect without respawning this task.
                let base = resolve_probe_base(base, service_key.as_deref());
                match prepare_active_health_url(&base, active.path.as_deref(), egress_policy) {
                    Ok(check_url) => client
                        .get(check_url)
                        .timeout(timeout)
                        .send()
                        .await
                        .is_ok_and(|response| response.status().is_success()),
                    Err(_) => false,
                }
            }
            None => false,
        };

        record_active_health_result(
            &health,
            &active,
            has_passive_health,
            &mut consecutive_failures,
            &mut consecutive_successes,
            success,
        );
    }
}

/// Update the active-health counters and the shared health flag for one tick.
///
/// Passive monitoring exclusively owns circuit opening when it is configured.
/// Active success may recover either mode, but it must not reset passive
/// counters or the monotonic half-open/backoff state.
fn record_active_health_result(
    health: &HealthState,
    active: &ActiveHealthCheck,
    has_passive_health: bool,
    consecutive_failures: &mut u32,
    consecutive_successes: &mut u32,
    success: bool,
) {
    if success {
        *consecutive_failures = 0;
        *consecutive_successes = (*consecutive_successes).saturating_add(1);
        if *consecutive_successes >= active.healthy_threshold {
            health.healthy.store(true, Ordering::Relaxed);
        }
    } else {
        *consecutive_successes = 0;
        *consecutive_failures = (*consecutive_failures).saturating_add(1);
        if !has_passive_health && *consecutive_failures >= active.unhealthy_threshold {
            health.healthy.store(false, Ordering::Relaxed);
        }
    }
}

/// Resolve the effective probe base URL for one active-health tick.
///
/// When `service_key` names a Service anchor carrying an `upstreamAuthority`
/// policy with `health_check_host` set, the probe's Host header AND URL host
/// (and, over TLS, its SNI) are rewritten to that static authority — an active
/// probe has no request to render a per-request label from. Otherwise (no
/// `service_key`, no policy, or a policy with no `health_check_host`) `base`
/// is returned unchanged, exactly as before this override existed.
fn resolve_probe_base(base: String, service_key: Option<&str>) -> String {
    match service_key
        .and_then(|key| {
            let target =
                crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(key);
            get_backend_traffic_policy_store()
                .snapshot()
                .get_effective(&target)
                .and_then(|policy| policy.upstream_authority.clone())
        })
        .and_then(|compiled| compiled.health_check_host.clone())
    {
        Some(host) => replace_url_host(&base, &host),
        None => base,
    }
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/common/outbound/client_test.rs"]
mod tests;
