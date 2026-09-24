use arc_swap::ArcSwap;
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::HealthPolicyIdentity;

static HC_CONFIG_STORE: LazyLock<Arc<HealthCheckConfigStore>> =
    LazyLock::new(|| Arc::new(HealthCheckConfigStore::new()));
static NEXT_HEALTH_POLICY_GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn get_hc_config_store() -> Arc<HealthCheckConfigStore> {
    HC_CONFIG_STORE.clone()
}

/// Per-service active health check configuration store.
///
/// A single map keyed by Service key (`namespace/service`). The
/// `EdgionBackendTrafficPolicy` conf-handler is the sole writer: it computes the
/// winning policy per Service key and pushes the effective config via
/// `set_service_config`. There are no source tiers anymore.
pub struct HealthCheckConfigStore {
    configs: ArcSwap<HashMap<String, Arc<ResolvedActiveHealthPolicy>>>,
    publication: parking_lot::RwLock<()>,
}

pub(crate) struct ResolvedActiveHealthPolicy {
    pub config: ActiveHealthCheckConfig,
    pub identity: Arc<HealthPolicyIdentity>,
}

impl Default for HealthCheckConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheckConfigStore {
    pub fn new() -> Self {
        Self {
            configs: ArcSwap::from_pointee(HashMap::new()),
            publication: parking_lot::RwLock::new(()),
        }
    }

    /// Resolve the active health check config for a Service key, if any.
    pub fn resolve(&self, service_key: &str) -> Option<ActiveHealthCheckConfig> {
        self.configs.load().get(service_key).map(|policy| policy.config.clone())
    }

    pub(crate) fn resolve_policy(&self, service_key: &str) -> Option<Arc<ResolvedActiveHealthPolicy>> {
        self.configs.load().get(service_key).cloned()
    }

    /// Set or clear the config for a Service key. `None` removes any entry.
    pub fn set_service_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let _publication = self.publication.write();
        let mut new_map = (**self.configs.load()).clone();
        match config {
            Some(cfg) => {
                if new_map.get(service_key).is_some_and(|current| current.config == cfg) {
                    return;
                }
                let Some(generation) = NEXT_HEALTH_POLICY_GENERATION
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1))
                    .ok()
                else {
                    new_map.remove(service_key);
                    self.configs.store(Arc::new(new_map));
                    return;
                };
                new_map.insert(
                    service_key.to_string(),
                    Arc::new(ResolvedActiveHealthPolicy {
                        config: cfg,
                        identity: Arc::new(HealthPolicyIdentity { generation }),
                    }),
                );
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.configs.store(Arc::new(new_map));
    }

    /// Each poll is admitted atomically with respect to configuration publication.
    /// The synchronous read guard is dropped before this wrapper yields Pending.
    pub(crate) async fn with_current_generation<F, T>(
        &self,
        service_key: &str,
        identity: &Arc<HealthPolicyIdentity>,
        future: F,
    ) -> Result<T, super::probes::ProbeError>
    where
        F: std::future::Future<Output = Result<T, super::probes::ProbeError>>,
    {
        let mut future = std::pin::pin!(future);
        futures::future::poll_fn(|cx| {
            let _publication = self.publication.read();
            let current = self.configs.load();
            if !current
                .get(service_key)
                .is_some_and(|policy| Arc::ptr_eq(&policy.identity, identity))
            {
                return std::task::Poll::Ready(Err(super::probes::ProbeError::Superseded));
            }
            future.as_mut().poll(cx)
        })
        .await
    }

    /// Number of Service keys currently configured.
    pub fn count(&self) -> usize {
        self.configs.load().len()
    }

    /// Service keys currently configured (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.configs.load().keys().cloned().collect()
    }

    /// Remove the config for a Service key (equivalent to `set_service_config(key, None)`).
    pub fn remove_service(&self, service_key: &str) {
        self.set_service_config(service_key, None);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/config_store_test/stats_tests.rs"]
mod stats_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/config_store_test.rs"]
mod tests;
