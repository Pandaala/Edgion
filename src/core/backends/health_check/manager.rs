use super::backend_hash;
use super::config_store::{get_hc_config_store, HealthCheckConfigStore};
use super::probes::{build_http_probe_client, probe_http, probe_http_with_client, probe_tcp};
use super::status_store::{get_health_status_store, HealthStatusStore};
use crate::core::backends::endpoint::get_endpoint_roundrobin_store;
use crate::core::backends::endpoint_slice::get_roundrobin_store;
use crate::core::backends::try_get_global_endpoint_mode;
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::core::utils::duration::parse_duration;
use crate::types::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};
use pingora_load_balancing::Backend;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use tokio::task::JoinHandle;

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
                self.remove_task(service_key);
                self.health_store.unregister_service(service_key);
            }
        }
    }

    fn upsert_task(&self, service_key: String, config: ActiveHealthCheckConfig) {
        self.cancel_task(&service_key);

        let Ok(runtime_handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                service = %service_key,
                "No active Tokio runtime, skip starting health check task"
            );
            return;
        };

        let health_store = self.health_store.clone();
        let task_service_key = service_key.clone();
        let handle = runtime_handle.spawn(async move {
            health_check_loop(task_service_key, config, health_store).await;
        });

        self.tasks.write().expect("lock tasks").insert(service_key, handle);
    }

    fn remove_task(&self, service_key: &str) {
        self.cancel_task(service_key);
    }

    fn cancel_task(&self, service_key: &str) {
        if let Some(handle) = self.tasks.write().expect("lock tasks").remove(service_key) {
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
            };

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

fn get_service_backends(service_key: &str) -> Vec<Backend> {
    let mode = try_get_global_endpoint_mode().unwrap_or(EndpointMode::EndpointSlice);
    let mut backends: BTreeSet<Backend> = BTreeSet::new();

    match mode {
        EndpointMode::EndpointSlice => {
            backends.extend(get_roundrobin_store().get_backends_for_service(service_key));
        }
        EndpointMode::Endpoint => {
            backends.extend(get_endpoint_roundrobin_store().get_backends_for_service(service_key));
        }
        EndpointMode::Both | EndpointMode::Auto => {
            backends.extend(get_roundrobin_store().get_backends_for_service(service_key));
            backends.extend(get_endpoint_roundrobin_store().get_backends_for_service(service_key));
        }
    }

    backends.into_iter().collect()
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
