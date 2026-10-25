use super::config_store::{get_hc_config_store, HealthCheckConfigStore};
use super::encrypted::EncryptedProbeTransport;
use super::probes::{build_http_probe_client, probe_grpc, probe_http, probe_http_with_client, probe_tcp, ProbeError};
use super::status_store::{get_health_status_store, HealthStatusStore};
use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
use crate::backends::discovery::get_service_discovery_coordinator;
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;
use crate::lb::LbBackend;
use edgion_resources::duration::parse_duration;
#[cfg(test)]
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use edgion_resources::resources::health_check::HealthCheckType;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use parking_lot::{Mutex, RwLock};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

/// Maximum number of individual backend probes that may be in flight
/// concurrently across all health check tasks. Each probe acquires a
/// permit only for the duration of its single probe call and releases it
/// immediately after, so the limit bounds total in-flight probes rather
/// than whole probe cycles — a service with many backends cannot hold a
/// permit for its entire cycle and starve unrelated services' probes.
const MAX_CONCURRENT_PROBES: usize = 50;

static PROBE_SEMAPHORE: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES)));

static HEALTH_CHECK_MANAGER: LazyLock<Arc<HealthCheckManager>> = LazyLock::new(|| Arc::new(HealthCheckManager::new()));

pub fn get_health_check_manager() -> Arc<HealthCheckManager> {
    HEALTH_CHECK_MANAGER.clone()
}

/// Manage background health check tasks per service.
pub struct HealthCheckManager {
    tasks: RwLock<HashMap<String, JoinHandle<()>>>,
    health_store: Arc<HealthStatusStore>,
    config_store: Arc<HealthCheckConfigStore>,
    reconcile_lock: Mutex<()>,
}

impl Default for HealthCheckManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheckManager {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            health_store: get_health_status_store(),
            config_store: get_hc_config_store(),
            reconcile_lock: Mutex::new(()),
        }
    }

    /// Reconcile a service after annotation/config changes.
    pub fn reconcile_service(&self, service_key: &str) {
        let _reconcile_guard = self.reconcile_lock.lock();
        match self.config_store.resolve_policy(service_key) {
            Some(policy) => {
                get_service_discovery_coordinator().set_health_policy(service_key, Some(Arc::clone(&policy.identity)));
                self.upsert_task(service_key.to_string(), policy);
                tracing::info!(
                    service = %service_key,
                    "Health check reconciled"
                );
            }
            None => {
                get_service_discovery_coordinator().set_health_policy(service_key, None);
                self.cancel_task(service_key);
                self.health_store.unregister_service(service_key);
            }
        }
    }

    fn upsert_task(&self, service_key: String, policy: Arc<super::config_store::ResolvedActiveHealthPolicy>) {
        let Ok(runtime_handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                service = %service_key,
                "No active Tokio runtime, skip starting health check task"
            );
            // No new task to spawn, but still drop any existing one for this key.
            self.cancel_task(&service_key);
            self.health_store.unregister_service(&service_key);
            return;
        };

        let task_service_key = service_key.clone();
        // Reconciliation is serialized; retire the old task before admitting its replacement.
        self.cancel_task(&service_key);
        // Status-marker ownership stays in the serialized reconciliation path.
        // A task aborted during synchronous work must not be able to recreate a
        // marker after removal.
        self.health_store.register_service(&service_key);
        let handle = runtime_handle.spawn(async move {
            health_check_loop(task_service_key, policy).await;
        });

        // Atomically swap the per-service handle: the single write-locked insert
        // is the linearization point, and the displaced handle (if any) is always
        // aborted. This prevents concurrent reconcile_service calls for the same
        // key (Service vs EndpointSlice handlers run on independent tasks) from
        // overwriting a freshly spawned handle without aborting it, which would
        // leak the orphaned health-check task.
        let previous = self.tasks.write().insert(service_key, handle);
        if let Some(old) = previous {
            old.abort();
        }
    }

    fn cancel_task(&self, service_key: &str) {
        if let Some(handle) = self.tasks.write().remove(service_key) {
            handle.abort();
        }
    }
}

/// The Host authority a probe should send for `service_key`: the Service's
/// `upstreamAuthority.healthCheckHost` when present, else the configured
/// static `config_host`. Pure function over the process-global
/// `BackendTrafficPolicyStore` so the precedence rule can be unit-tested
/// without spawning a `health_check_loop` task.
fn resolve_probe_host(service_key: &str, config_host: Option<&str>) -> Option<String> {
    let target =
        crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(service_key);
    get_backend_traffic_policy_store()
        .snapshot()
        .get_effective(&target)
        .and_then(|policy| {
            policy
                .upstream_authority
                .as_ref()
                .and_then(|compiled| compiled.health_check_host.clone())
        })
        .or_else(|| config_host.map(str::to_owned))
}

async fn health_check_loop(service_key: String, policy: Arc<super::config_store::ResolvedActiveHealthPolicy>) {
    let config = policy.config();
    let policy_identity = Arc::clone(&policy.identity);
    let interval = parse_probe_duration(&config.interval, Duration::from_secs(10)).max(Duration::from_secs(1));
    let timeout = parse_probe_duration(&config.timeout, Duration::from_secs(3)).max(Duration::from_millis(100));
    let http_path = config.path.clone().unwrap_or_else(|| "/".to_string());
    let http_expected_statuses = if config.expected_statuses.is_empty() {
        vec![200]
    } else {
        config.expected_statuses.clone()
    };
    // A Service carrying an `upstreamAuthority` policy dispatches by Host/SNI,
    // and an active probe has no request to render a per-request label from.
    // Admission guarantees `health_check_host` is present and in-pattern
    // whenever an active health check is configured for such a Service, so it
    // always wins over the static `config.host` when a policy is attached.
    // Resolved once here (not per-tick): the whole `ProbeParams` bundle below
    // is immutable for this task's lifetime, and any policy change respawns a
    // fresh task via `BackendTrafficPolicyStore::reapply_service` ->
    // `HealthCheckManager::reconcile_service`.
    let http_host = resolve_probe_host(&service_key, config.host.as_deref());
    let grpc_service_name = config.grpc_service_name.clone().unwrap_or_default();
    let encrypted = matches!(config.r#type, HealthCheckType::Https | HealthCheckType::Grpcs)
        .then(|| EncryptedProbeTransport::build(policy.owner()));

    let mut http_client = if matches!(config.r#type, HealthCheckType::Http) {
        match build_http_probe_client(timeout) {
            Ok(client) => Some(client),
            Err(err) => {
                let error_class = ProbeError::RequestFailed(err).class();
                tracing::warn!(
                    service = %service_key,
                    error = error_class,
                    "Failed to build reusable HTTP health-check client, falling back to per-probe client"
                );
                None
            }
        }
    } else {
        None
    };

    // Add deterministic startup jitter so many services don't probe at the same instant.
    let startup_jitter = derive_startup_jitter(&service_key, interval / 2);
    if !startup_jitter.is_zero() {
        tokio::time::sleep(startup_jitter).await;
    }

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let semaphore = PROBE_SEMAPHORE.clone();

    // Immutable probe parameters for this task's lifetime, shared across all
    // backend probe futures of every cycle (config changes spawn a fresh task,
    // so these never mutate here). Wrapped in an Arc so each future clones a
    // pointer rather than the underlying strings/vectors.
    let probe_params = Arc::new(ProbeParams {
        kind: config.r#type.clone(),
        path: http_path,
        port: config.port,
        host: http_host,
        expected_statuses: http_expected_statuses,
        grpc_service_name,
        timeout,
        encrypted,
    });

    loop {
        ticker.tick().await;

        let backends = get_service_backends(&service_key);

        // Best-effort: rebuild the shared HTTP client once per cycle if it could
        // not be built earlier. Hoisting the retry here (instead of doing it
        // mid-probe) keeps the per-backend futures free of shared mutable state.
        if matches!(config.r#type, HealthCheckType::Http) && http_client.is_none() {
            if let Ok(client) = build_http_probe_client(timeout) {
                http_client = Some(client);
            }
        }

        // Probe all backends of this service concurrently rather than one after
        // another, so the worst-case sweep time is max(probe_i) instead of
        // sum(probe_i). Each future independently acquires a PROBE_SEMAPHORE
        // permit for the duration of its single probe and releases it right
        // after, so the global MAX_CONCURRENT_PROBES cap and the per-probe
        // acquire/release semantics (cross-service fairness, backends-F016) are
        // preserved. Mirrors Envoy's per-host parallel active health checking.
        let mut probes = FuturesUnordered::new();
        for backend in backends {
            let runtime_identity = backend.runtime_identity().cloned();
            let membership_identity = backend.runtime_membership_identity();
            let Some(addr) = backend.addr.as_inet().copied() else {
                tracing::debug!(
                    service = %service_key,
                    backend = %backend.addr,
                    "Skipping health check for non-IP backend address"
                );
                // No permit acquired and no probe issued for non-IP backends.
                if let (Some(identity), Some(membership)) = (runtime_identity.as_deref(), membership_identity) {
                    get_service_discovery_coordinator().record_health_completion(
                        &policy_identity,
                        membership,
                        identity,
                        false,
                        config.healthy_threshold,
                        config.unhealthy_threshold,
                    );
                }
                continue;
            };

            let semaphore = semaphore.clone();
            let params = probe_params.clone();
            let client = http_client.clone();
            let config_store = get_hc_config_store();
            let captured_service_key = &service_key;
            let captured_policy_identity = &policy_identity;
            probes.push(async move {
                let probe = async {
                    let permit = semaphore.acquire().await.expect("probe semaphore closed");
                    let result = probe_backend(&params, client.as_ref(), addr).await;
                    drop(permit);
                    result
                };
                let result = if params.encrypted.is_some() {
                    config_store
                        .with_current_generation(captured_service_key, captured_policy_identity, probe)
                        .await
                } else {
                    probe.await
                };
                (addr, runtime_identity, membership_identity, result)
            });
        }

        // Apply results sequentially as each probe completes. The Coordinator
        // rechecks membership and endpoint identity under the endpoint-local
        // completion lock, so an old probe cannot mutate a replacement that
        // reused the same socket.
        while let Some((addr, runtime_identity, membership_identity, result)) = probes.next().await {
            if let (Some(identity), Some(membership)) = (runtime_identity.as_deref(), membership_identity) {
                get_service_discovery_coordinator().record_health_completion(
                    &policy_identity,
                    membership,
                    identity,
                    result.is_ok(),
                    config.healthy_threshold,
                    config.unhealthy_threshold,
                );
            }

            if let Err(err) = result {
                tracing::debug!(
                    service = %service_key,
                    backend = %addr,
                    error = err.class(),
                    "Health check probe failed"
                );
            }
        }
    }
}

/// Immutable parameters for a single backend probe, shared (via `Arc`) across all
/// concurrent probe futures of one service health-check cycle.
struct ProbeParams {
    kind: HealthCheckType,
    path: String,
    port: Option<u16>,
    host: Option<String>,
    expected_statuses: Vec<u16>,
    grpc_service_name: String,
    timeout: Duration,
    encrypted: Option<Result<EncryptedProbeTransport, ProbeError>>,
}

/// Run one probe against `addr` for the given probe parameters. When the probe
/// type is HTTP and a shared client is available it is reused; otherwise a
/// per-probe client is built (preserving the original fallback behavior).
async fn probe_backend(
    params: &ProbeParams,
    http_client: Option<&reqwest::Client>,
    addr: std::net::SocketAddr,
) -> Result<(), ProbeError> {
    match params.kind {
        HealthCheckType::Http => match http_client {
            Some(client) => {
                probe_http_with_client(
                    client,
                    addr,
                    &params.path,
                    params.port,
                    params.host.as_deref(),
                    &params.expected_statuses,
                )
                .await
            }
            None => {
                probe_http(
                    addr,
                    &params.path,
                    params.port,
                    params.host.as_deref(),
                    &params.expected_statuses,
                    params.timeout,
                )
                .await
            }
        },
        HealthCheckType::Tcp => probe_tcp(addr, params.port, params.timeout).await,
        HealthCheckType::Grpc => probe_grpc(addr, params.port, &params.grpc_service_name, params.timeout).await,
        HealthCheckType::Https | HealthCheckType::Grpcs => {
            let transport = params
                .encrypted
                .as_ref()
                .and_then(|transport| transport.as_ref().ok())
                .ok_or(ProbeError::InvalidTlsConfig)?;
            let address = std::net::SocketAddr::new(addr.ip(), params.port.unwrap_or(addr.port()));
            tokio::time::timeout(params.timeout, async {
                match params.kind {
                    HealthCheckType::Https => {
                        transport
                            .https(
                                address,
                                &params.path,
                                params.host.as_deref(),
                                &params.expected_statuses,
                                params.timeout,
                            )
                            .await
                    }
                    HealthCheckType::Grpcs => {
                        transport
                            .grpcs(address, &params.grpc_service_name, params.timeout)
                            .await
                    }
                    _ => unreachable!("encrypted probe branch"),
                }
            })
            .await
            .map_err(|_| ProbeError::Timeout)?
        }
    }
}

fn parse_probe_duration(input: &str, fallback: Duration) -> Duration {
    parse_duration(input).unwrap_or(fallback)
}

fn derive_startup_jitter(service_key: &str, max_jitter: Duration) -> Duration {
    let max_ms = max_jitter.as_millis();
    if max_ms == 0 {
        return Duration::ZERO;
    }

    let mut hasher = DefaultHasher::new();
    service_key.hash(&mut hasher);
    let offset_ms = hasher.finish() % (max_ms as u64 + 1);
    Duration::from_millis(offset_ms)
}

/// Returns the set of backends for a service, sourced exclusively from the
/// EndpointSlice discovery store. The store already deduplicates and orders
/// its result, so no further dedup is needed here.
fn get_service_backends(service_key: &str) -> Vec<Arc<LbBackend>> {
    get_roundrobin_store()
        .get_backends_for_service(service_key)
        .iter()
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/manager_test.rs"]
mod tests;
