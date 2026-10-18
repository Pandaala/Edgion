//! AllEndpointStatus plugin — query all backend endpoints and return aggregated status.
//!
//! Designed for health checks and deployment verification: a single request fans out
//! to every endpoint behind the current route's backends and returns a JSON summary.
//!
//! Runs entirely in the RequestFilter stage (ErrTerminateRequest), never reaches upstream.

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use http::{method::InvalidMethod, Method};
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

use crate::backends::discovery::{get_service_discovery_coordinator, ServiceSectionKey, ServiceSelectorGeneration};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::policy::get_global_backend_tls_policy_store;
use crate::backends::validate_endpoint_backend_addr;
use crate::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::plugin_configs::all_endpoint_status::MAX_GLOBAL_CONCURRENCY;
use edgion_resources::resources::edgion_plugins::AllEndpointStatusConfig;
use edgion_resources::{HTTPBackendRef, HTTPBackendRefKind};

use super::transport::ProbeTransport;

/// Global configuration for AllEndpointStatus plugin.
/// Stored in YAML config, loaded at gateway startup, applies to all instances.
/// These values act as security ceilings that per-plugin config cannot exceed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AllEndpointStatusGlobalConfig {
    /// Global ceiling for max_endpoints across all plugin instances.
    /// Per-plugin config's maxEndpoints is clamped to min(plugin_value, this_value, 50).
    /// Default: 20. Hard maximum: 50.
    #[serde(default = "default_global_max_endpoints")]
    pub max_endpoints: usize,

    /// Minimum interval between consecutive plugin executions in milliseconds.
    /// If a new request arrives before this interval has elapsed since the last
    /// execution, it is immediately rejected with 429 Too Many Requests.
    /// Uses a process-level AtomicU64 timestamp for lock-free checking.
    /// Default: 1000 (1 second). Set to 0 to disable (not recommended).
    #[serde(default = "default_global_min_interval_ms")]
    pub min_interval_ms: u64,
}

fn default_global_max_endpoints() -> usize {
    20
}
fn default_global_min_interval_ms() -> u64 {
    1000
}

impl Default for AllEndpointStatusGlobalConfig {
    fn default() -> Self {
        Self {
            max_endpoints: default_global_max_endpoints(),
            min_interval_ms: default_global_min_interval_ms(),
        }
    }
}

/// Fallback used before the startup initializer publishes the process config.
static DEFAULT_CONFIG: LazyLock<AllEndpointStatusGlobalConfig> = LazyLock::new(AllEndpointStatusGlobalConfig::default);

/// Immutable process config, published once during gateway startup.
static ALL_ENDPOINT_STATUS_GLOBAL_CONFIG: OnceLock<AllEndpointStatusGlobalConfig> = OnceLock::new();

pub fn get_all_endpoint_status_global_config() -> &'static AllEndpointStatusGlobalConfig {
    ALL_ENDPOINT_STATUS_GLOBAL_CONFIG.get().unwrap_or(&DEFAULT_CONFIG)
}

/// Initialize the global AllEndpointStatus configuration.
/// Called once during gateway startup with the loaded YAML config.
pub fn init_all_endpoint_status_global_config(config: &AllEndpointStatusGlobalConfig) {
    // Startup owns publication. A duplicate call is ignored so the first
    // immutable configuration remains authoritative.
    let _ = ALL_ENDPOINT_STATUS_GLOBAL_CONFIG.set(config.clone());
    let published = get_all_endpoint_status_global_config();
    tracing::info!(
        max_endpoints = published.max_endpoints,
        min_interval_ms = published.min_interval_ms,
        "AllEndpointStatus global config initialized"
    );
}

// ============================================================
// Process-level concurrency and rate limiting
// ============================================================

/// Process-level concurrency gate: at most MAX_GLOBAL_CONCURRENCY (3) AllEndpointStatus
/// requests can execute simultaneously. This is a compile-time constant and cannot
/// be overridden by configuration, ensuring a deterministic upper bound on resource
/// consumption (3 × 50 endpoints × 16KB = 2.4MB max).
static GLOBAL_CONCURRENCY: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_GLOBAL_CONCURRENCY));

/// Process-level rate limiter: tracks the timestamp (ms since process start)
/// of the latest claim or successful plugin execution, encoded as elapsed
/// milliseconds plus one so zero remains the unclaimed sentinel. New requests
/// must wait at least `min_interval_ms` (from global YAML config) since this
/// timestamp. Uses AtomicU64 + Instant for lock-free, monotonic timing.
static LAST_EXECUTION_MS: AtomicU64 = AtomicU64::new(0);
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);
const MAX_ELAPSED_MS: u64 = u64::MAX - 1;

/// Get elapsed milliseconds since process start (monotonic).
fn elapsed_ms() -> u64 {
    duration_millis(Instant::now().saturating_duration_since(*PROCESS_START))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis())
        .unwrap_or(MAX_ELAPSED_MS)
        .min(MAX_ELAPSED_MS)
}

/// Encode a valid elapsed millisecond value while reserving zero for "never claimed".
fn encode_elapsed_ms(now_ms: u64) -> u64 {
    now_ms.min(MAX_ELAPSED_MS) + 1
}

/// Atomically claim the current execution tick.
///
/// Returns the exact remaining delay in milliseconds when the interval has not
/// elapsed. Clock rollback is treated as zero elapsed time and therefore fails
/// closed until the previous tick is reached again.
fn claim_execution_at(state: &AtomicU64, now_ms: u64, min_interval_ms: u64) -> Result<(), u64> {
    let now_ms = now_ms.min(MAX_ELAPSED_MS);
    let encoded_now = encode_elapsed_ms(now_ms);

    loop {
        let observed = state.load(Ordering::Relaxed);
        if let Some(last_ms) = observed.checked_sub(1) {
            let since_last = now_ms.saturating_sub(last_ms);
            if since_last < min_interval_ms {
                return Err(min_interval_ms - since_last);
            }
        }

        if state
            .compare_exchange(observed, encoded_now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(());
        }
    }
}

fn record_execution_completed_at(state: &AtomicU64, now_ms: u64) {
    state.fetch_max(encode_elapsed_ms(now_ms), Ordering::Relaxed);
}

// ============================================================
// Response data structures
// ============================================================

#[derive(Serialize)]
struct AggregatedResponse {
    summary: Summary,
    backends: Vec<BackendResult>,
}

#[derive(Serialize)]
struct Summary {
    total_backends: usize,
    total_endpoints: usize,
    success_count: usize,
    failure_count: usize,
    /// true if endpoint count exceeded max_endpoints and was truncated
    truncated: bool,
    timeout_ms: u64,
    wall_timeout_ms: u64,
    /// wall-clock time for the entire fan-out operation
    total_latency_ms: u64,
    /// total response body bytes fetched from all endpoints (before JSON encoding)
    total_response_bytes: usize,
    /// true if wall timeout was hit and some endpoints were cancelled
    wall_timeout_hit: bool,
}

#[derive(Serialize)]
struct BackendResult {
    name: String,
    namespace: String,
    port: u16,
    endpoint_count: usize,
    endpoints: Vec<EndpointResult>,
}

#[derive(Serialize)]
struct EndpointResult {
    /// IP:Port of the endpoint
    address: String,
    /// HTTP status code (null if request failed)
    status: Option<u16>,
    /// Request latency in milliseconds
    latency_ms: u64,
    /// Response body (truncated to max_body_size, null if failed)
    body: Option<String>,
    /// Whether the body was truncated due to size limit
    body_truncated: bool,
    /// Response headers (only if include_response_headers is true)
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<HashMap<String, String>>,
    /// Error message (null if successful)
    error: Option<String>,
}

struct CompletedEndpointResult {
    ordinal: usize,
    result: EndpointResult,
    response_body_bytes: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct EndpointAggregate {
    success_count: usize,
    failure_count: usize,
    total_response_bytes: usize,
}

// ============================================================
// Helper: stream-read body with size limit
// ============================================================

/// Read response body with a strict size limit using streaming.
/// Only reads up to `max_size` bytes, then stops immediately.
/// This prevents OOM when an endpoint returns a very large body —
/// we never buffer more than max_size in memory.
async fn read_body_limited(resp: reqwest::Response, max_size: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::with_capacity(max_size.min(8192));
    let mut truncated = false;
    let mut stream = resp.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let remaining = max_size - buf.len();
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                if chunk.len() <= remaining {
                    buf.extend_from_slice(&chunk);
                } else {
                    buf.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
            }
            Err(_) => break, // Network error mid-stream, return what we have
        }
    }

    (buf, truncated)
}

// ============================================================
// Helper: resolve endpoints from backend ref
// ============================================================

/// Information about a resolved endpoint for fan-out
#[derive(Clone)]
struct ResolvedEndpoint {
    address: String,      // "ip:port"
    backend_name: String, // service name (used for Host header)
    transport: Arc<ProbeTransport>,
    allow_loopback: bool,
}

struct EndpointCollection {
    backends: Vec<BackendResult>,
    selected_endpoints: Vec<ResolvedEndpoint>,
    discovered_endpoint_count: usize,
    unsafe_endpoint_count: usize,
    denied_ref_count: usize,
    invalid_ref_count: usize,
}

impl EndpointCollection {
    fn truncated(&self) -> bool {
        self.discovered_endpoint_count > self.selected_endpoints.len()
    }
}

fn resolved_endpoint_from_backend(
    backend: pingora_load_balancing::Backend,
    backend_name: &str,
    transport: Arc<ProbeTransport>,
    allow_loopback_upstream: bool,
) -> Result<ResolvedEndpoint, ()> {
    validate_endpoint_backend_addr(&backend.addr, allow_loopback_upstream).map_err(|_| ())?;
    Ok(ResolvedEndpoint {
        address: backend.addr.to_string(),
        backend_name: backend_name.to_string(),
        transport,
        allow_loopback: allow_loopback_upstream,
    })
}

/// Resolve endpoint sockets and policy section from the same published Service proof.
fn resolve_endpoints(
    backend_name: &str,
    backend_namespace: &str,
    backend_port: u16,
    allow_loopback_upstream: bool,
    discovery: &Arc<ServiceSelectorGeneration>,
    policies: &BackendTLSPolicySnapshot,
    gateway_key: Option<&str>,
) -> Result<(Vec<ResolvedEndpoint>, usize), ()> {
    let service_key = format!("{}/{}", backend_namespace, backend_name);
    let service = discovery.available(&service_key).ok_or(())?;
    let section = service
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(backend_port).ok_or(())?))
        .map_err(|_| ())?;
    let lookup = policies.lookup(
        &PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: backend_namespace.to_owned(),
            name: backend_name.to_owned(),
        },
        section.identity.name.as_deref(),
    );
    let transport = Arc::new(ProbeTransport::new(
        lookup,
        Arc::clone(discovery),
        policies.generation(),
        gateway_key,
    ));
    let mut unsafe_count = 0;
    let endpoints = section
        .ready_slots
        .iter()
        .filter_map(|slot| {
            match resolved_endpoint_from_backend(
                slot.backend.inner.clone(),
                backend_name,
                Arc::clone(&transport),
                allow_loopback_upstream,
            ) {
                Ok(endpoint) => Some(endpoint),
                Err(()) => {
                    unsafe_count += 1;
                    None
                }
            }
        })
        .collect();
    Ok((endpoints, unsafe_count))
}

fn collect_route_endpoints_with<F>(
    backend_refs: &[HTTPBackendRef],
    route_namespace: &str,
    effective_max: usize,
    allow_loopback_upstream: bool,
    mut resolver: F,
) -> EndpointCollection
where
    F: FnMut(&str, &str, u16, bool) -> Result<(Vec<ResolvedEndpoint>, usize), ()>,
{
    let mut backends = Vec::new();
    let mut selected_endpoints = Vec::with_capacity(effective_max);
    let mut discovered_endpoint_count = 0usize;
    let mut unsafe_endpoint_count = 0usize;
    let mut denied_ref_count = 0usize;
    let mut invalid_ref_count = 0usize;

    for backend_ref in backend_refs {
        if backend_ref.backend_kind() != HTTPBackendRefKind::Service {
            continue;
        }
        if backend_ref.ref_denied.is_some() {
            denied_ref_count = denied_ref_count.saturating_add(1);
            continue;
        }

        let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
        let Some(port) = backend_ref
            .port
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0)
        else {
            invalid_ref_count += 1;
            continue;
        };
        let Ok((resolved, unsafe_count)) = resolver(&backend_ref.name, namespace, port, allow_loopback_upstream) else {
            invalid_ref_count += 1;
            continue;
        };

        let endpoint_count = resolved.len();
        discovered_endpoint_count = discovered_endpoint_count.saturating_add(endpoint_count);
        unsafe_endpoint_count = unsafe_endpoint_count.saturating_add(unsafe_count);

        let remaining = effective_max.saturating_sub(selected_endpoints.len());
        selected_endpoints.extend(resolved.into_iter().take(remaining));
        backends.push(BackendResult {
            name: backend_ref.name.clone(),
            namespace: namespace.to_string(),
            port,
            endpoint_count,
            endpoints: Vec::new(),
        });
    }

    EndpointCollection {
        backends,
        selected_endpoints,
        discovered_endpoint_count,
        unsafe_endpoint_count,
        denied_ref_count,
        invalid_ref_count,
    }
}

// ============================================================
// Plugin struct
// ============================================================

pub struct AllEndpointStatus {
    name: String,
    config: AllEndpointStatusConfig,
    method_override: Option<Method>,
}

impl AllEndpointStatus {
    pub fn new(config: &AllEndpointStatusConfig) -> Result<Self, InvalidMethod> {
        let method_override = config.method_override.as_deref().map(str::parse).transpose()?;
        Ok(Self {
            name: "AllEndpointStatus".to_string(),
            config: config.clone(),
            method_override,
        })
    }

    fn selected_method(&self, original_method: &str) -> Result<Method, InvalidMethod> {
        match &self.method_override {
            Some(method) => Ok(method.clone()),
            None => original_method.parse(),
        }
    }

    /// Write a JSON response and return ErrTerminateRequest.
    async fn send_json_response(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: Bytes,
    ) -> PluginRunningResult {
        self.send_json_response_with_headers(session, status, body, &[]).await
    }

    /// Write a JSON response with extra headers and return ErrTerminateRequest.
    async fn send_json_response_with_headers(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: Bytes,
        extra_headers: &[(&str, &str)],
    ) -> PluginRunningResult {
        let Ok(mut resp) = ResponseHeader::build(status, None) else {
            return PluginRunningResult::ErrTerminateRequest;
        };
        let _ = resp.insert_header("Content-Type", "application/json");
        let _ = resp.insert_header("Content-Length", body.len().to_string());
        let _ = resp.insert_header("Connection", "close");
        let _ = resp.insert_header("Cache-Control", "no-store");
        for (name, value) in extra_headers {
            let _ = resp.insert_header(name.to_string(), value.to_string());
        }

        let _ = session.write_response_header(Box::new(resp), false).await;
        let _ = session.write_response_body(Some(body), true).await;
        session.shutdown().await;

        PluginRunningResult::ErrTerminateRequest
    }

    /// Fan-out requests to all endpoints with wall-clock timeout.
    /// Uses FuturesUnordered for partial results on wall timeout.
    async fn fan_out_requests(
        &self,
        endpoints: &[ResolvedEndpoint],
        method: &Method,
        original_path: &str,
        original_query: Option<&str>,
        original_headers: &[(String, String)],
    ) -> (Vec<CompletedEndpointResult>, bool /* wall_timeout_hit */) {
        let Some(permit_count) = fan_out_permit_count(self.config.concurrency_limit, endpoints.len()) else {
            return (Vec::new(), false);
        };

        let semaphore = std::sync::Arc::new(Semaphore::new(permit_count));
        let timeout = Duration::from_millis(self.config.effective_timeout_ms());
        let wall_timeout = Duration::from_millis(self.config.effective_wall_timeout_ms());
        let max_body = self.config.effective_max_body_size();
        let include_headers = self.config.include_response_headers;

        let path = self.config.path_override.as_deref().unwrap_or(original_path);

        let dynamic_listed: Vec<String> = original_headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("connection"))
            .map(|(_, v)| connection_listed_hop_by_hop(v))
            .unwrap_or_default();

        let futures: FuturesUnordered<_> = endpoints
            .iter()
            .enumerate()
            .map(|(ordinal, ep)| {
                let sem = &semaphore;
                let addr = ep.address.clone();
                let backend_name = ep.backend_name.as_str();
                let transport = &ep.transport;
                let allow_loopback = ep.allow_loopback;
                let method = method.clone();
                let query = original_query;
                let headers = original_headers;
                let dynamic_listed = &dynamic_listed;

                async move {
                    let _permit = sem
                        .acquire()
                        .await
                        .expect("Semaphore never closed (created per fan_out_requests call)");
                    let start = Instant::now();

                    let prepared = addr
                        .parse()
                        .map_err(|_| "invalid probe address")
                        .and_then(|address| transport.client_and_url(address, path, query, allow_loopback));
                    let (client, url) = match prepared {
                        Ok(prepared) => prepared,
                        Err(error) => return failed_endpoint(ordinal, addr, start, error),
                    };

                    let mut req = client.request(method, url).timeout(timeout);

                    // Forward original headers. Skip hop-by-hop (RFC 9110 §7.6.1),
                    // Host (rewritten below to the backend service), and headers
                    // that describe the inbound body — this probe sends none.
                    for (name, value) in headers {
                        if is_hop_by_hop_or_listed(name, dynamic_listed)
                            || name.eq_ignore_ascii_case("host")
                            || is_inbound_body_header(name)
                        {
                            continue;
                        }
                        req = req.header(name.as_str(), value.as_str());
                    }
                    req = req.header("Host", backend_name);

                    match req.send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let resp_headers = if include_headers {
                                Some(
                                    resp.headers()
                                        .iter()
                                        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
                                        .collect::<HashMap<String, String>>(),
                                )
                            } else {
                                None
                            };

                            let (body_bytes, truncated) = read_body_limited(resp, max_body).await;
                            let response_body_bytes = body_bytes.len();
                            let body = String::from_utf8_lossy(&body_bytes).to_string();

                            CompletedEndpointResult {
                                ordinal,
                                response_body_bytes,
                                result: EndpointResult {
                                    address: addr,
                                    status: Some(status),
                                    latency_ms: duration_millis(Instant::now().saturating_duration_since(start)),
                                    body: Some(body),
                                    body_truncated: truncated,
                                    headers: resp_headers,
                                    error: None,
                                },
                            }
                        }
                        Err(e) => {
                            let error_msg = if e.is_timeout() {
                                "request timeout"
                            } else if e.is_connect() {
                                "connection failed"
                            } else {
                                "request failed"
                            };
                            failed_endpoint(ordinal, addr, start, error_msg)
                        }
                    }
                }
            })
            .collect();

        // Wall-clock timeout with partial result preservation
        fan_out_with_wall_timeout(futures, wall_timeout).await
    }
}

fn failed_endpoint(ordinal: usize, address: String, start: Instant, error: &'static str) -> CompletedEndpointResult {
    CompletedEndpointResult {
        ordinal,
        response_body_bytes: 0,
        result: EndpointResult {
            address,
            status: None,
            latency_ms: duration_millis(start.elapsed()),
            body: None,
            body_truncated: false,
            headers: None,
            error: Some(error.to_owned()),
        },
    }
}

/// Headers that describe the inbound message body.
///
/// `Content-Length` and `Expect` are end-to-end, so hop-by-hop filtering leaves
/// them alone. The status probe sends no body, so copying them makes the backend
/// wait for bytes that never arrive, or answer `417`.
fn is_inbound_body_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("expect")
}

fn fan_out_permit_count(configured: usize, selected_endpoint_count: usize) -> Option<usize> {
    if selected_endpoint_count == 0 {
        return None;
    }

    Some(configured.min(selected_endpoint_count))
}

/// Fan-out with wall timeout that preserves partial results.
async fn fan_out_with_wall_timeout(
    mut unordered: FuturesUnordered<impl std::future::Future<Output = CompletedEndpointResult>>,
    wall_timeout: Duration,
) -> (Vec<CompletedEndpointResult>, bool) {
    let mut results = Vec::with_capacity(unordered.len());
    let deadline = tokio::time::sleep(wall_timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            result = unordered.next() => {
                match result {
                    Some(result) => results.push(result),
                    None => break,
                }
            }
            _ = &mut deadline => {
                let remaining = unordered.len();
                return (results, remaining > 0);
            }
        }
    }

    (results, false)
}

fn assemble_endpoint_results(
    selected_endpoints: &[ResolvedEndpoint],
    completed_results: Vec<CompletedEndpointResult>,
    backends: &mut [BackendResult],
) -> EndpointAggregate {
    let mut completed_by_ordinal: Vec<Option<CompletedEndpointResult>> =
        std::iter::repeat_with(|| None).take(selected_endpoints.len()).collect();

    for completed in completed_results {
        let ordinal = completed.ordinal;
        debug_assert!(
            ordinal < completed_by_ordinal.len(),
            "completed endpoint ordinal is outside the selected prefix"
        );
        if let Some(slot) = completed_by_ordinal.get_mut(ordinal) {
            debug_assert!(slot.is_none(), "one fan-out future owns each endpoint ordinal");
            if slot.is_none() {
                *slot = Some(completed);
            }
        }
    }

    let mut aggregate = EndpointAggregate::default();
    let mut endpoint_offset = 0usize;

    for backend in backends {
        let count = backend
            .endpoint_count
            .min(selected_endpoints.len().saturating_sub(endpoint_offset));
        for index in endpoint_offset..endpoint_offset + count {
            if let Some(completed) = completed_by_ordinal[index].take() {
                if completed
                    .result
                    .status
                    .is_some_and(|status| (200..400).contains(&status))
                {
                    aggregate.success_count = aggregate.success_count.saturating_add(1);
                } else {
                    aggregate.failure_count = aggregate.failure_count.saturating_add(1);
                }
                aggregate.total_response_bytes = aggregate
                    .total_response_bytes
                    .saturating_add(completed.response_body_bytes);
                backend.endpoints.push(completed.result);
            } else {
                aggregate.failure_count = aggregate.failure_count.saturating_add(1);
                backend.endpoints.push(EndpointResult {
                    address: selected_endpoints[index].address.clone(),
                    status: None,
                    latency_ms: 0,
                    body: None,
                    body_truncated: false,
                    headers: None,
                    error: Some("wall timeout exceeded".to_string()),
                });
            }
        }
        endpoint_offset += count;
    }

    aggregate
}

// ============================================================
// RequestFilter implementation
// ============================================================

#[async_trait]
impl RequestFilter for AllEndpointStatus {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let global_config = get_all_endpoint_status_global_config();

        // === Global rate limit check ===
        // Loop until we either claim the execution slot via CAS or get rate-limited.
        // A bare load+check followed by a discarded CAS would let concurrent requests
        // that all read the same `last` value bypass the min-interval gate together.
        let min_interval = global_config.min_interval_ms;
        if min_interval > 0 {
            if let Err(remaining_ms) = claim_execution_at(&LAST_EXECUTION_MS, elapsed_ms(), min_interval) {
                let retry_after_s = remaining_ms.div_ceil(1000);
                log.push_err("rate limited");
                let body = format!(r#"{{"error":"rate limited, retry after {}s"}}"#, retry_after_s);
                return self
                    .send_json_response_with_headers(
                        session,
                        429,
                        Bytes::from(body),
                        &[("Retry-After", &retry_after_s.to_string())],
                    )
                    .await;
            }
        }

        // === Global concurrency gate ===
        let Ok(Ok(_global_permit)) = tokio::time::timeout(Duration::from_secs(5), GLOBAL_CONCURRENCY.acquire()).await
        else {
            log.push_err("concurrency limit");
            return self
                .send_json_response(
                    session,
                    503,
                    Bytes::from_static(br#"{"error":"too many concurrent all-endpoint-status requests"}"#),
                )
                .await;
        };

        let start = Instant::now();

        // 1. Get route_unit and backend_refs
        let ctx = session.ctx();
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit.clone(),
            None => {
                log.push_err("no route");
                return self
                    .send_json_response(session, 503, Bytes::from_static(br#"{"error":"no route matched"}"#))
                    .await;
            }
        };

        let backend_refs = match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => refs.clone(),
            _ => {
                log.push("status served 0");
                let empty = br#"{"summary":{"total_backends":0,"total_endpoints":0,"success_count":0,"failure_count":0,"truncated":false,"timeout_ms":0,"wall_timeout_ms":0,"total_latency_ms":0,"total_response_bytes":0,"wall_timeout_hit":false},"backends":[]}"#;
                return self.send_json_response(session, 200, Bytes::from_static(empty)).await;
            }
        };

        // Collect original request info (before mutable borrow)
        let original_path = session.get_path().to_string();
        let original_query = session.get_query();
        let original_headers = session.request_headers();
        let route_namespace = route_unit.matched_info.rsc.ns.clone();

        // 2. Resolve all endpoints from all backends
        let effective_max = self.config.effective_max_endpoints(global_config.max_endpoints);
        let discovery = get_service_discovery_coordinator().load_generation();
        let policies = get_global_backend_tls_policy_store().snapshot();
        let gateway_key = ctx.gateway_info.as_ref().map(|gateway| gateway.gateway_key());
        let collection = collect_route_endpoints_with(
            &backend_refs,
            &route_namespace,
            effective_max,
            ctx.allow_loopback_upstream,
            |name, namespace, port, allow_loopback| {
                resolve_endpoints(
                    name,
                    namespace,
                    port,
                    allow_loopback,
                    &discovery,
                    &policies,
                    gateway_key,
                )
            },
        );

        if collection.invalid_ref_count > 0 {
            log.push_err("probe configuration unavailable");
            return PluginRunningResult::ErrResponse {
                status: 503,
                body: Some(Bytes::from_static(b"probe configuration unavailable")),
            };
        }

        if collection.denied_ref_count > 0 {
            log.push_err("backend ref denied");
        }
        if collection.unsafe_endpoint_count > 0 {
            log.push_err("unsafe upstream");
        }

        let total_truncated = collection.truncated();
        let mut all_backend_results = collection.backends;
        let all_endpoints = collection.selected_endpoints;
        let total_endpoints = all_endpoints.len();

        if total_endpoints == 0 {
            log.push("status served 0");
            let response = AggregatedResponse {
                summary: Summary {
                    total_backends: all_backend_results.len(),
                    total_endpoints: 0,
                    success_count: 0,
                    failure_count: 0,
                    truncated: total_truncated,
                    timeout_ms: self.config.effective_timeout_ms(),
                    wall_timeout_ms: self.config.effective_wall_timeout_ms(),
                    total_latency_ms: duration_millis(Instant::now().saturating_duration_since(start)),
                    total_response_bytes: 0,
                    wall_timeout_hit: false,
                },
                backends: all_backend_results,
            };
            let json_body = serde_json::to_string(&response).unwrap_or_default();
            return self.send_json_response(session, 200, Bytes::from(json_body)).await;
        }

        // Resolve the inherited request method once, before any endpoint future.
        let Ok(method) = self.selected_method(session.get_method()) else {
            log.push_err("probe configuration unavailable");
            return PluginRunningResult::ErrResponse {
                status: 503,
                body: Some(Bytes::from_static(b"probe configuration unavailable")),
            };
        };

        // 3. Fan-out requests
        let (endpoint_results, wall_timeout_hit) = self
            .fan_out_requests(
                &all_endpoints,
                &method,
                &original_path,
                original_query.as_deref(),
                &original_headers,
            )
            .await;

        // 4. Map results back to backends
        let aggregate = assemble_endpoint_results(&all_endpoints, endpoint_results, &mut all_backend_results);
        if aggregate.failure_count > 0 {
            log.push_err("endpoint probe failed");
        }

        record_execution_completed_at(&LAST_EXECUTION_MS, elapsed_ms());

        let total_latency = duration_millis(Instant::now().saturating_duration_since(start));

        // 5. Build aggregated response
        let response = AggregatedResponse {
            summary: Summary {
                total_backends: all_backend_results.len(),
                total_endpoints,
                success_count: aggregate.success_count,
                failure_count: aggregate.failure_count,
                truncated: total_truncated,
                timeout_ms: self.config.effective_timeout_ms(),
                wall_timeout_ms: self.config.effective_wall_timeout_ms(),
                total_latency_ms: total_latency,
                total_response_bytes: aggregate.total_response_bytes,
                wall_timeout_hit,
            },
            backends: all_backend_results,
        };

        let Ok(json_body) = serde_json::to_string(&response) else {
            log.push_err("serialize fail");
            return self
                .send_json_response(
                    session,
                    500,
                    Bytes::from_static(br#"{"error":"internal serialization error"}"#),
                )
                .await;
        };

        log.push_fmt("status served {}", la![total_endpoints]);

        self.send_json_response(session, 200, Bytes::from(json_body)).await
        // _global_permit is dropped here, releasing the concurrency slot
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/all_endpoint_status/plugin_test.rs"]
mod tests;
