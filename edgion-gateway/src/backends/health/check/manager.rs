use super::backend_hash;
use super::config_store::{get_hc_config_store, HealthCheckConfigStore};
use super::probes::{build_http_probe_client, probe_grpc, probe_http, probe_http_with_client, probe_tcp};
use super::status_store::{get_health_status_store, HealthStatusStore};
use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};
use parking_lot::RwLock;
use pingora_load_balancing::Backend;
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
        }
    }

    /// Reconcile a service after annotation/config changes.
    pub fn reconcile_service(&self, service_key: &str) {
        match self.config_store.resolve(service_key) {
            Some(resolved) => {
                self.upsert_task(service_key.to_string(), resolved.config);
                tracing::info!(
                    service = %service_key,
                    source = ?resolved.source,
                    "Health check reconciled"
                );
            }
            None => {
                self.cancel_task(service_key);
                self.health_store.unregister_service(service_key);
            }
        }
    }

    fn upsert_task(&self, service_key: String, config: ActiveHealthCheckConfig) {
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
        let handle = runtime_handle.spawn(async move {
            health_check_loop(task_service_key, config, health_store).await;
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

async fn health_check_loop(service_key: String, config: ActiveHealthCheckConfig, health_store: Arc<HealthStatusStore>) {
    let interval = parse_probe_duration(&config.interval, Duration::from_secs(10)).max(Duration::from_secs(1));
    let timeout = parse_probe_duration(&config.timeout, Duration::from_secs(3)).max(Duration::from_millis(100));
    let http_path = config.path.clone().unwrap_or_else(|| "/".to_string());
    let http_expected_statuses = if config.expected_statuses.is_empty() {
        vec![200]
    } else {
        config.expected_statuses.clone()
    };
    let http_host = config.host.clone();
    let grpc_service_name = config.grpc_service_name.clone().unwrap_or_default();

    let mut http_client = if matches!(config.r#type, HealthCheckType::Http) {
        match build_http_probe_client(timeout) {
            Ok(client) => Some(client),
            Err(err) => {
                tracing::warn!(
                    service = %service_key,
                    error = %err,
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

    loop {
        ticker.tick().await;

        let backends = get_service_backends(&service_key);
        let hashes: Vec<u64> = backends.iter().map(backend_hash).collect();
        health_store.register_service(&service_key, hashes);

        for backend in backends {
            let hash = backend_hash(&backend);
            let Some(addr) = backend.addr.as_inet() else {
                tracing::debug!(
                    service = %service_key,
                    backend = %backend.addr,
                    "Skipping health check for non-IP backend address"
                );
                health_store.record_check(
                    &service_key,
                    hash,
                    false,
                    config.healthy_threshold,
                    config.unhealthy_threshold,
                );
                continue;
            };

            // Acquire a probe permit per individual backend probe (not per cycle)
            // so the global cap bounds total in-flight probes. The permit is held
            // only for this single probe call and released immediately after, so a
            // service with many backends interleaves fairly with other services
            // instead of monopolizing one permit for its entire cycle.
            let permit = semaphore.acquire().await.expect("probe semaphore closed");
            let result = match config.r#type {
                HealthCheckType::Http => {
                    match http_client.as_ref() {
                        Some(client) => {
                            probe_http_with_client(
                                client,
                                *addr,
                                &http_path,
                                config.port,
                                http_host.as_deref(),
                                &http_expected_statuses,
                            )
                            .await
                        }
                        None => {
                            // Best-effort retry to restore shared client.
                            if let Ok(client) = build_http_probe_client(timeout) {
                                let probe_result = probe_http_with_client(
                                    &client,
                                    *addr,
                                    &http_path,
                                    config.port,
                                    http_host.as_deref(),
                                    &http_expected_statuses,
                                )
                                .await;
                                http_client = Some(client);
                                probe_result
                            } else {
                                // Fallback preserves behavior.
                                probe_http(
                                    *addr,
                                    &http_path,
                                    config.port,
                                    http_host.as_deref(),
                                    &http_expected_statuses,
                                    timeout,
                                )
                                .await
                            }
                        }
                    }
                }
                HealthCheckType::Tcp => probe_tcp(*addr, config.port, timeout).await,
                HealthCheckType::Grpc => probe_grpc(*addr, config.port, &grpc_service_name, timeout).await,
            };
            drop(permit);

            health_store.record_check(
                &service_key,
                hash,
                result.is_ok(),
                config.healthy_threshold,
                config.unhealthy_threshold,
            );

            if let Err(err) = result {
                tracing::debug!(
                    service = %service_key,
                    backend = %backend.addr,
                    error = ?err,
                    "Health check probe failed"
                );
            }
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
fn get_service_backends(service_key: &str) -> Vec<Backend> {
    get_roundrobin_store()
        .get_backends_for_service(service_key)
        .into_iter()
        .map(|b| b.into_inner())
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
}
