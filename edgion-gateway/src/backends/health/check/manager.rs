use super::config_store::{get_hc_config_store, HealthCheckConfigStore};
use super::probes::{build_http_probe_client, probe_grpc, probe_http, probe_http_with_client, probe_tcp, ProbeError};
use super::status_store::{get_health_status_store, HealthStatusStore};
use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
use crate::backends::discovery::get_service_discovery_coordinator;
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;
use crate::lb::EdgionBackend;
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};
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
            return;
        };

        let health_store = self.health_store.clone();
        let task_service_key = service_key.clone();
        let config = policy.config.clone();
        let policy_identity = Arc::clone(&policy.identity);
        let handle = runtime_handle.spawn(async move {
            health_check_loop(task_service_key, config, policy_identity, health_store).await;
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
fn resolve_probe_host(service_key: &str, config_host: Option<String>) -> Option<String> {
    let target =
        crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(service_key);
    get_backend_traffic_policy_store()
        .snapshot()
        .get_effective(&target)
        .and_then(|policy| policy.upstream_authority.clone())
        .and_then(|compiled| compiled.health_check_host.clone())
        .or(config_host)
}

async fn health_check_loop(
    service_key: String,
    config: ActiveHealthCheckConfig,
    policy_identity: Arc<crate::backends::discovery::HealthPolicyIdentity>,
    health_store: Arc<HealthStatusStore>,
) {
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
    let http_host = resolve_probe_host(&service_key, config.host.clone());
    let grpc_service_name = config.grpc_service_name.clone().unwrap_or_default();

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
    });

    loop {
        ticker.tick().await;

        let backends = get_service_backends(&service_key);
        // Keep the service marker for panic-threshold gating, but do not put
        // address hashes on the production write path. Routing health lives in
        // the Coordinator's identity-keyed runtime state; hash-keyed methods
        // remain only for isolated compatibility tests.
        health_store.register_service(&service_key, Vec::new());

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
            probes.push(async move {
                let permit = semaphore.acquire().await.expect("probe semaphore closed");
                let result = probe_backend(&params, client.as_ref(), addr).await;
                drop(permit);
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
fn get_service_backends(service_key: &str) -> Vec<Arc<EdgionBackend>> {
    get_roundrobin_store()
        .get_backends_for_service(service_key)
        .iter()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_startup_jitter_within_bound() {
        let max = Duration::from_millis(500);
        let jitter = derive_startup_jitter("default/svc-a", max);
        assert!(jitter <= max);
    }

    #[test]
    fn test_derive_startup_jitter_deterministic() {
        let max = Duration::from_secs(3);
        let a = derive_startup_jitter("default/svc-a", max);
        let b = derive_startup_jitter("default/svc-a", max);
        assert_eq!(a, b);
    }

    #[test]
    fn test_derive_startup_jitter_zero_bound() {
        let jitter = derive_startup_jitter("default/svc-a", Duration::ZERO);
        assert_eq!(jitter, Duration::ZERO);
    }

    mod resolve_probe_host_tests {
        use super::*;
        use crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK;
        use edgion_resources::resources::edgion_backend_traffic_policy::{
            EdgionBackendTrafficPolicySpec, PolicyTargetRef,
        };
        use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
        use edgion_resources::resources::EdgionBackendTrafficPolicy;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        use std::collections::{HashMap, HashSet};

        /// Build a minimal valid `EdgionBackendTrafficPolicy` targeting `svc`
        /// (within namespace `ns`) with the given `upstreamAuthority`.
        fn policy_with_authority(
            ns: &str,
            name: &str,
            svc: &str,
            authority: Option<UpstreamAuthorityConfig>,
        ) -> Arc<EdgionBackendTrafficPolicy> {
            Arc::new(EdgionBackendTrafficPolicy {
                metadata: ObjectMeta {
                    name: Some(name.to_string()),
                    namespace: Some(ns.to_string()),
                    ..Default::default()
                },
                spec: EdgionBackendTrafficPolicySpec {
                    target_refs: vec![PolicyTargetRef {
                        group: String::new(),
                        kind: "Service".to_string(),
                        name: svc.to_string(),
                    }],
                    resolved_target_class: Some(
                        edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                    ),
                    retry_constraint: None,
                    load_balancer: None,
                    health_check: None,
                    outlier_detection: None,
                    upstream_authority: authority,
                    circuit_breaker: None,
                    connection: None,
                },
                status: None,
            })
        }

        /// Insert `policy` into the process-global `BackendTrafficPolicyStore`
        /// under `policy_key` and reapply, then hand back a guard that removes
        /// it again on drop so other tests sharing the singleton are unaffected.
        struct InstalledPolicy {
            policy_key: String,
            service_key: String,
        }

        impl InstalledPolicy {
            fn install(policy_key: &str, service_key: &str, policy: Arc<EdgionBackendTrafficPolicy>) -> Self {
                let mut add = HashMap::new();
                add.insert(policy_key.to_string(), policy);
                get_backend_traffic_policy_store().partial_update_policies(add, HashMap::new(), HashSet::new());
                Self {
                    policy_key: policy_key.to_string(),
                    service_key: service_key.to_string(),
                }
            }
        }

        impl Drop for InstalledPolicy {
            fn drop(&mut self) {
                let mut remove = HashSet::new();
                remove.insert(self.policy_key.clone());
                get_backend_traffic_policy_store().partial_update_policies(HashMap::new(), HashMap::new(), remove);
                // Sanity: the authority must be gone after cleanup so later tests
                // (or this one, if the assertion above already failed) never see
                // a stale entry for this key.
                debug_assert!(get_backend_traffic_policy_store()
                    .get_authority(&self.service_key)
                    .is_none());
            }
        }

        // `healthCheckHost` present on the resolved authority wins over a
        // different `config_host`. This is the precedence the report describes,
        // and the one the moved authority-write fix (store.rs) exists to serve
        // fresh to the spawned probe task.
        #[test]
        fn health_check_host_overrides_config_host() {
            let _guard = GLOBAL_STORE_TEST_LOCK.lock();
            let ns = "hcresolve-a";
            let svc = "svc";
            let service_key = format!("{ns}/{svc}");
            let policy_key = format!("{ns}/p1");

            let policy = policy_with_authority(
                ns,
                "p1",
                svc,
                Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: Some("probe.sandbox.internal".to_string()),
                }),
            );
            let _installed = InstalledPolicy::install(&policy_key, &service_key, policy);

            let resolved = resolve_probe_host(&service_key, Some("configured-host".to_string()));
            assert_eq!(resolved, Some("probe.sandbox.internal".to_string()));
        }

        #[test]
        fn publish_then_probe_sees_new_host() {
            let _guard = GLOBAL_STORE_TEST_LOCK.lock();
            let ns = "hcresolve-d";
            let svc = "svc";
            let service_key = format!("{ns}/{svc}");
            let policy_key = format!("{ns}/p1");

            let first = policy_with_authority(
                ns,
                "p1",
                svc,
                Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: Some("probe-old.sandbox.internal".to_string()),
                }),
            );
            let _installed = InstalledPolicy::install(&policy_key, &service_key, first);
            assert_eq!(
                resolve_probe_host(&service_key, Some("configured-host".to_string())),
                Some("probe-old.sandbox.internal".to_string())
            );

            let second = policy_with_authority(
                ns,
                "p1",
                svc,
                Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: Some("probe-new.sandbox.internal".to_string()),
                }),
            );
            let mut update = HashMap::new();
            update.insert(policy_key, second);
            get_backend_traffic_policy_store().partial_update_policies(HashMap::new(), update, HashSet::new());
            assert_eq!(
                resolve_probe_host(&service_key, Some("configured-host".to_string())),
                Some("probe-new.sandbox.internal".to_string())
            );
        }

        // No policy attached to the Service: the helper falls back to the
        // configured static host unchanged.
        #[test]
        fn no_policy_falls_back_to_config_host() {
            let _guard = GLOBAL_STORE_TEST_LOCK.lock();
            let service_key = "hcresolve-b/svc";

            let resolved = resolve_probe_host(service_key, Some("configured-host".to_string()));
            assert_eq!(resolved, Some("configured-host".to_string()));
        }

        // A policy is attached, but its authority carries no `healthCheckHost`:
        // the helper still falls back to the configured static host.
        #[test]
        fn authority_without_health_check_host_falls_back_to_config_host() {
            let _guard = GLOBAL_STORE_TEST_LOCK.lock();
            let ns = "hcresolve-c";
            let svc = "svc";
            let service_key = format!("{ns}/{svc}");
            let policy_key = format!("{ns}/p1");

            let policy = policy_with_authority(
                ns,
                "p1",
                svc,
                Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: None,
                }),
            );
            let _installed = InstalledPolicy::install(&policy_key, &service_key, policy);

            let resolved = resolve_probe_host(&service_key, Some("configured-host".to_string()));
            assert_eq!(resolved, Some("configured-host".to_string()));
        }
    }
}
