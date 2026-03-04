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
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

use crate::core::backends::endpoint::{get_endpoint_roundrobin_store, EndpointExt};
use crate::core::backends::endpoint_slice::{get_roundrobin_store, EndpointSliceExt};
use crate::core::plugins::edgion_plugins::common::http_client::is_hop_by_hop;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::plugin_configs::all_endpoint_status::MAX_GLOBAL_CONCURRENCY;
use crate::types::resources::edgion_plugins::AllEndpointStatusConfig;

/// Global configuration for AllEndpointStatus plugin.
/// Stored in TOML config, loaded at gateway startup, applies to all instances.
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

/// Global config store, initialized from TOML at startup.
static ALL_ENDPOINT_STATUS_GLOBAL_CONFIG: LazyLock<RwLock<AllEndpointStatusGlobalConfig>> =
    LazyLock::new(|| RwLock::new(AllEndpointStatusGlobalConfig::default()));

pub fn get_all_endpoint_status_global_config() -> AllEndpointStatusGlobalConfig {
    ALL_ENDPOINT_STATUS_GLOBAL_CONFIG
        .read()
        .map(|c| c.clone())
        .unwrap_or_default()
}

/// Initialize the global AllEndpointStatus configuration.
/// Called once during gateway startup with the loaded TOML config.
pub fn init_all_endpoint_status_global_config(config: &AllEndpointStatusGlobalConfig) {
    if let Ok(mut c) = ALL_ENDPOINT_STATUS_GLOBAL_CONFIG.write() {
        *c = config.clone();
        tracing::info!(
            max_endpoints = config.max_endpoints,
            min_interval_ms = config.min_interval_ms,
            "AllEndpointStatus global config initialized"
        );
    }
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
/// of the last successful plugin execution. New requests must wait at least
/// `min_interval_ms` (from global TOML config) since this timestamp.
/// Uses AtomicU64 + Instant for lock-free, monotonic timing.
static LAST_EXECUTION_MS: AtomicU64 = AtomicU64::new(0);
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Get elapsed milliseconds since process start (monotonic).
fn elapsed_ms() -> u64 {
    PROCESS_START.elapsed().as_millis() as u64
}

// ============================================================
// Dedicated HTTP client (isolated connection pool)
// ============================================================

/// Dedicated HTTP client for AllEndpointStatus plugin.
/// Isolated connection pool — does NOT share with get_http_client().
/// This prevents fan-out requests from starving other plugins' connections.
static STATUS_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(3))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build AllEndpointStatus HTTP client")
});

fn get_status_http_client() -> &'static reqwest::Client {
    &STATUS_HTTP_CLIENT
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

#[derive(Serialize, Clone)]
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
struct ResolvedEndpoint {
    address: String,      // "ip:port"
    backend_name: String, // service name (used for Host header)
    use_tls: bool,
}

/// Resolve all endpoint addresses for a backend reference.
/// Tries EndpointSlice first (newer K8s API), falls back to Endpoints.
fn resolve_endpoints(
    backend_name: &str,
    backend_namespace: &str,
    backend_port: u16,
    use_tls: bool,
) -> Vec<ResolvedEndpoint> {
    let service_key = format!("{}/{}", backend_namespace, backend_name);

    // Try EndpointSlice first (preferred, newer K8s API)
    let eps_store = get_roundrobin_store();
    if let Some(slices) = eps_store.get_slices_for_service(&service_key) {
        let mut backends = BTreeSet::new();
        for slice in &slices {
            backends.extend(slice.build_backends(backend_port));
        }
        if !backends.is_empty() {
            return backends
                .into_iter()
                .map(|b| ResolvedEndpoint {
                    address: b.addr.to_string(),
                    backend_name: backend_name.to_string(),
                    use_tls,
                })
                .collect();
        }
    }

    // Fallback to Endpoints
    let ep_store = get_endpoint_roundrobin_store();
    if let Some(endpoints) = ep_store.get_endpoint_for_service(&service_key) {
        return endpoints
            .build_backends(backend_port)
            .into_iter()
            .map(|b| ResolvedEndpoint {
                address: b.addr.to_string(),
                backend_name: backend_name.to_string(),
                use_tls,
            })
            .collect();
    }

    vec![] // No endpoints found
}

// ============================================================
// Plugin struct
// ============================================================

pub struct AllEndpointStatus {
    name: String,
    config: AllEndpointStatusConfig,
}

impl AllEndpointStatus {
    pub fn new(config: &AllEndpointStatusConfig) -> Self {
        Self {
            name: "AllEndpointStatus".to_string(),
            config: config.clone(),
        }
    }

    /// Write a JSON response and return ErrTerminateRequest.
    async fn send_json_response(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: &str,
    ) -> PluginRunningResult {
        self.send_json_response_with_headers(session, status, body, &[]).await
    }

    /// Write a JSON response with extra headers and return ErrTerminateRequest.
    async fn send_json_response_with_headers(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: &str,
        extra_headers: &[(&str, &str)],
    ) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };
        let _ = resp.insert_header("Content-Type", "application/json");
        let _ = resp.insert_header("Connection", "close");
        let _ = resp.insert_header("Cache-Control", "no-store");
        for (name, value) in extra_headers {
            let _ = resp.insert_header(name.to_string(), value.to_string());
        }

        let _ = session.write_response_header(Box::new(resp), false).await;
        let _ = session
            .write_response_body(Some(Bytes::from(body.to_string())), true)
            .await;
        session.shutdown().await;

        PluginRunningResult::ErrTerminateRequest
    }

    /// Fan-out requests to all endpoints with wall-clock timeout.
    /// Uses FuturesUnordered for partial results on wall timeout.
    async fn fan_out_requests(
        &self,
        endpoints: &[ResolvedEndpoint],
        original_method: &str,
        original_path: &str,
        original_query: Option<&str>,
        original_headers: &[(String, String)],
        _global_config: &AllEndpointStatusGlobalConfig,
    ) -> (Vec<EndpointResult>, bool /* wall_timeout_hit */) {
        let client = get_status_http_client();
        let semaphore = std::sync::Arc::new(Semaphore::new(self.config.concurrency_limit));
        let timeout = Duration::from_millis(self.config.effective_timeout_ms());
        let wall_timeout = Duration::from_millis(self.config.effective_wall_timeout_ms());
        let max_body = self.config.effective_max_body_size();
        let include_headers = self.config.include_response_headers;

        let method = self.config.method_override.as_deref().unwrap_or(original_method);
        let path = self.config.path_override.as_deref().unwrap_or(original_path);

        let futures: FuturesUnordered<_> = endpoints
            .iter()
            .map(|ep| {
                let sem = semaphore.clone();
                let addr = ep.address.clone();
                let backend_name = ep.backend_name.clone();
                let use_tls = ep.use_tls;
                let method = method.to_string();
                let path = path.to_string();
                let query = original_query.map(|q| q.to_string());
                let headers: Vec<(String, String)> = original_headers.to_vec();

                async move {
                    let _permit = sem.acquire().await.unwrap();
                    let start = Instant::now();

                    let scheme = if use_tls { "https" } else { "http" };
                    let mut url = format!("{}://{}{}", scheme, addr, path);
                    if let Some(ref q) = query {
                        url.push('?');
                        url.push_str(q);
                    }

                    let req_method: reqwest::Method = method.parse().unwrap_or(reqwest::Method::GET);
                    let mut req = client.request(req_method, &url).timeout(timeout);

                    // Forward original headers, filtering hop-by-hop
                    for (name, value) in &headers {
                        if !is_hop_by_hop(name) && name.to_lowercase() != "host" {
                            req = req.header(name.as_str(), value.as_str());
                        }
                    }
                    // Set Host to backend service name
                    req = req.header("Host", backend_name.as_str());

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
                            let body = String::from_utf8_lossy(&body_bytes).to_string();

                            EndpointResult {
                                address: addr,
                                status: Some(status),
                                latency_ms: start.elapsed().as_millis() as u64,
                                body: Some(body),
                                body_truncated: truncated,
                                headers: resp_headers,
                                error: None,
                            }
                        }
                        Err(e) => {
                            let error_msg = if e.is_timeout() {
                                "request timeout".to_string()
                            } else if e.is_connect() {
                                format!("connection failed: {}", e)
                            } else {
                                format!("request failed: {}", e)
                            };

                            EndpointResult {
                                address: addr,
                                status: None,
                                latency_ms: start.elapsed().as_millis() as u64,
                                body: None,
                                body_truncated: false,
                                headers: None,
                                error: Some(error_msg),
                            }
                        }
                    }
                }
            })
            .collect();

        // Wall-clock timeout with partial result preservation
        fan_out_with_wall_timeout(futures, wall_timeout).await
    }
}

/// Fan-out with wall timeout that preserves partial results.
async fn fan_out_with_wall_timeout(
    mut unordered: FuturesUnordered<impl std::future::Future<Output = EndpointResult>>,
    wall_timeout: Duration,
) -> (Vec<EndpointResult>, bool) {
    let mut results = Vec::with_capacity(unordered.len());
    let deadline = tokio::time::sleep(wall_timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            Some(result) = unordered.next() => {
                results.push(result);
            }
            _ = &mut deadline => {
                let remaining = unordered.len();
                if remaining > 0 {
                    tracing::warn!(
                        completed = results.len(),
                        cancelled = remaining,
                        wall_timeout_ms = wall_timeout.as_millis() as u64,
                        "AllEndpointStatus wall timeout: partial results returned"
                    );
                }
                return (results, remaining > 0);
            }
            else => break,
        }
    }

    (results, false)
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
        let min_interval = global_config.min_interval_ms;
        if min_interval > 0 {
            let now = elapsed_ms();
            let last = LAST_EXECUTION_MS.load(Ordering::Relaxed);
            let since_last = now.saturating_sub(last);

            if since_last < min_interval {
                let retry_after_s = (min_interval - since_last).div_ceil(1000);
                log.push("FAIL rate-limited; ");
                let body = format!(r#"{{"error":"rate limited, retry after {}s"}}"#, retry_after_s);
                return self
                    .send_json_response_with_headers(
                        session,
                        429,
                        &body,
                        &[("Retry-After", &retry_after_s.to_string())],
                    )
                    .await;
            }

            // CAS to claim this execution slot
            let _ = LAST_EXECUTION_MS.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed);
        }

        // === Global concurrency gate ===
        let _global_permit = match tokio::time::timeout(Duration::from_secs(5), GLOBAL_CONCURRENCY.acquire()).await {
            Ok(Ok(permit)) => permit,
            _ => {
                log.push("FAIL concurrency-limit; ");
                return self
                    .send_json_response(
                        session,
                        503,
                        r#"{"error":"too many concurrent all-endpoint-status requests"}"#,
                    )
                    .await;
            }
        };

        let start = Instant::now();

        // 1. Get route_unit and backend_refs
        let ctx = session.ctx();
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit.clone(),
            None => {
                log.push("FAIL no-route; ");
                return self
                    .send_json_response(session, 503, r#"{"error":"no route matched"}"#)
                    .await;
            }
        };

        let backend_refs = match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => refs.clone(),
            _ => {
                log.push("OK 0ep; ");
                let empty = r#"{"summary":{"total_backends":0,"total_endpoints":0,"success_count":0,"failure_count":0,"truncated":false,"timeout_ms":0,"wall_timeout_ms":0,"total_latency_ms":0,"total_response_bytes":0,"wall_timeout_hit":false},"backends":[]}"#;
                return self.send_json_response(session, 200, empty).await;
            }
        };

        // Collect original request info (before mutable borrow)
        let original_method = session.method();
        let original_path = session.get_path().to_string();
        let original_query = session.get_query();
        let original_headers = session.request_headers();
        let route_namespace = route_unit.matched_info.rns.clone();

        // 2. Resolve all endpoints from all backends
        let effective_max = self.config.effective_max_endpoints(global_config.max_endpoints);
        let mut all_backend_results: Vec<BackendResult> = Vec::new();
        let mut all_endpoints: Vec<ResolvedEndpoint> = Vec::new();
        let mut total_truncated = false;

        for backend_ref in &backend_refs {
            let namespace = backend_ref.namespace.as_deref().unwrap_or(&route_namespace);
            let port = backend_ref.port.map(|p| p as u16).unwrap_or(80);
            let use_tls = backend_ref.backend_tls_policy.is_some();

            let resolved = resolve_endpoints(&backend_ref.name, namespace, port, use_tls);

            all_backend_results.push(BackendResult {
                name: backend_ref.name.clone(),
                namespace: namespace.to_string(),
                port,
                endpoint_count: resolved.len(),
                endpoints: Vec::new(), // Will be filled after fan-out
            });

            all_endpoints.extend(resolved);
        }

        // Apply max_endpoints limit
        if all_endpoints.len() > effective_max {
            all_endpoints.truncate(effective_max);
            total_truncated = true;
        }

        let total_endpoints = all_endpoints.len();

        if total_endpoints == 0 {
            log.push("OK 0ep; ");
            let response = AggregatedResponse {
                summary: Summary {
                    total_backends: all_backend_results.len(),
                    total_endpoints: 0,
                    success_count: 0,
                    failure_count: 0,
                    truncated: false,
                    timeout_ms: self.config.effective_timeout_ms(),
                    wall_timeout_ms: self.config.effective_wall_timeout_ms(),
                    total_latency_ms: start.elapsed().as_millis() as u64,
                    total_response_bytes: 0,
                    wall_timeout_hit: false,
                },
                backends: all_backend_results,
            };
            let json_body = serde_json::to_string(&response).unwrap_or_default();
            return self.send_json_response(session, 200, &json_body).await;
        }

        // 3. Fan-out requests
        let (endpoint_results, wall_timeout_hit) = self
            .fan_out_requests(
                &all_endpoints,
                &original_method,
                &original_path,
                original_query.as_deref(),
                &original_headers,
                &global_config,
            )
            .await;

        // 4. Map results back to backends
        // Build a map: address -> EndpointResult for completed endpoints
        let mut result_map: HashMap<String, EndpointResult> = HashMap::new();
        for r in &endpoint_results {
            result_map.insert(r.address.clone(), r.clone());
        }

        let mut success_count = 0usize;
        let mut failure_count = 0usize;
        let mut total_response_bytes = 0usize;
        let mut ep_offset = 0usize;

        for backend in &mut all_backend_results {
            let count = backend
                .endpoint_count
                .min(all_endpoints.len().saturating_sub(ep_offset));
            for i in 0..count {
                let idx = ep_offset + i;
                if idx >= all_endpoints.len() {
                    break;
                }
                let addr = &all_endpoints[idx].address;
                if let Some(result) = result_map.remove(addr) {
                    if result.error.is_none() {
                        success_count += 1;
                        if let Some(ref body) = result.body {
                            total_response_bytes += body.len();
                        }
                    } else {
                        failure_count += 1;
                    }
                    backend.endpoints.push(result);
                } else {
                    // Endpoint was cancelled by wall timeout
                    failure_count += 1;
                    backend.endpoints.push(EndpointResult {
                        address: addr.clone(),
                        status: None,
                        latency_ms: 0,
                        body: None,
                        body_truncated: false,
                        headers: None,
                        error: Some("wall timeout exceeded".to_string()),
                    });
                }
            }
            ep_offset += count;
        }

        // Update last execution timestamp
        LAST_EXECUTION_MS.store(elapsed_ms(), Ordering::Relaxed);

        let total_latency = start.elapsed().as_millis() as u64;

        // 5. Build aggregated response
        let response = AggregatedResponse {
            summary: Summary {
                total_backends: all_backend_results.len(),
                total_endpoints,
                success_count,
                failure_count,
                truncated: total_truncated,
                timeout_ms: self.config.effective_timeout_ms(),
                wall_timeout_ms: self.config.effective_wall_timeout_ms(),
                total_latency_ms: total_latency,
                total_response_bytes,
                wall_timeout_hit,
            },
            backends: all_backend_results,
        };

        let json_body = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("AllEndpointStatus: failed to serialize response: {}", e);
                log.push("FAIL serialize; ");
                return self
                    .send_json_response(session, 500, r#"{"error":"internal serialization error"}"#)
                    .await;
            }
        };

        let fail_str = if failure_count > 0 {
            format!(" {}fail", failure_count)
        } else {
            String::new()
        };
        log.push(&format!("OK {}ep{} {}ms; ", total_endpoints, fail_str, total_latency));

        self.send_json_response(session, 200, &json_body).await
        // _global_permit is dropped here, releasing the concurrency slot
    }
}
