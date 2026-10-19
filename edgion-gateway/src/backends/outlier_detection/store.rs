use super::config::OutlierDetectionResolved;
use super::state::{FailureClass, FailureTransition, PassiveEjectionState};
use crate::backends::discovery::{get_service_discovery_coordinator, OutlierPolicyIdentity};
use crate::lb::{EndpointRuntimeIdentity, LbBackend};
use dashmap::DashMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

static OD_STORE: LazyLock<OutlierDetectionStore> = LazyLock::new(OutlierDetectionStore::new);

pub fn get_outlier_detection_store() -> &'static OutlierDetectionStore {
    &OD_STORE
}

#[derive(Clone, Debug)]
pub struct OutlierCompletionToken {
    policy_identity: Arc<OutlierPolicyIdentity>,
}

struct ServiceCbState {
    config: OutlierDetectionResolved,
    policy_identity: Arc<OutlierPolicyIdentity>,
    identity_endpoints: DashMap<EndpointRuntimeIdentity, Arc<PassiveEjectionState>>,
    /// Serializes the percent-check + ejection steps in identity-bound failure completion to prevent
    /// concurrent calls for distinct backends from both passing the max-ejection-percent
    /// guard simultaneously and over-ejecting beyond the configured limit.
    eject_lock: parking_lot::Mutex<()>,
    #[cfg(test)]
    percent_arbitrations: AtomicUsize,
}

impl ServiceCbState {
    /// Single canonical construction site for a fresh per-service breaker state, so a
    /// future field addition is made in one place rather than at every insert site.
    fn new(config: OutlierDetectionResolved, policy_identity: Arc<OutlierPolicyIdentity>) -> Arc<Self> {
        Arc::new(Self {
            config,
            policy_identity,
            identity_endpoints: DashMap::new(),
            eject_lock: parking_lot::Mutex::new(()),
            #[cfg(test)]
            percent_arbitrations: AtomicUsize::new(0),
        })
    }
}

pub struct OutlierDetectionStore {
    services: DashMap<String, Arc<ServiceCbState>>,
    next_policy_generation: AtomicU64,
    config_lock: parking_lot::Mutex<()>,
}

impl OutlierDetectionStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
            next_policy_generation: AtomicU64::new(1),
            config_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Number of services with outlier-detection state.
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Service keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.iter().map(|kv| kv.key().clone()).collect()
    }

    pub fn set_service_config(&self, service_key: &str, config: Option<OutlierDetectionResolved>) {
        self.set_service_config_in(get_service_discovery_coordinator().as_ref(), service_key, config);
    }

    pub(crate) fn set_service_config_in(
        &self,
        coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
        service_key: &str,
        config: Option<OutlierDetectionResolved>,
    ) {
        let _config_guard = self.config_lock.lock();
        match config {
            Some(cfg) => {
                let previous = self.services.get(service_key).map(|entry| Arc::clone(entry.value()));
                if let Some(existing) = previous.as_ref() {
                    let changed = existing.config.consecutive_errors != cfg.consecutive_errors
                        || existing.config.consecutive_gateway_errors != cfg.consecutive_gateway_errors
                        || existing.config.consecutive_local_origin_failures != cfg.consecutive_local_origin_failures
                        || existing.config.ejection_seconds != cfg.ejection_seconds
                        || existing.config.max_ejection_seconds != cfg.max_ejection_seconds
                        || existing.config.max_ejection_percent != cfg.max_ejection_percent;
                    if !changed {
                        return;
                    }
                }
                let Some(policy_identity) = self.allocate_policy_identity() else {
                    coordinator.set_outlier_policy(service_key, None);
                    self.services.remove(service_key);
                    return;
                };
                let next = ServiceCbState::new(cfg, Arc::clone(&policy_identity));
                coordinator.set_outlier_policy(service_key, Some(policy_identity));
                self.services.insert(service_key.to_string(), next);
            }
            None => {
                coordinator.set_outlier_policy(service_key, None);
                self.services.remove(service_key);
            }
        }
    }

    fn allocate_policy_identity(&self) -> Option<Arc<OutlierPolicyIdentity>> {
        self.next_policy_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1))
            .ok()
            .map(|generation| Arc::new(OutlierPolicyIdentity { generation }))
    }

    pub fn capture_completion_token(&self, backend: &LbBackend) -> Option<OutlierCompletionToken> {
        let service_key = backend.runtime_identity()?.service_key.as_ref();
        let service = self.services.get(service_key)?;
        Some(OutlierCompletionToken {
            policy_identity: Arc::clone(&service.policy_identity),
        })
    }

    pub fn record_failure_for_backend_with_token(
        &self,
        backend: &LbBackend,
        token: &OutlierCompletionToken,
        class: FailureClass,
    ) {
        self.record_failure_for_backend_in(get_service_discovery_coordinator().as_ref(), backend, token, class);
    }

    pub(crate) fn record_failure_for_backend_in(
        &self,
        coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
        backend: &LbBackend,
        token: &OutlierCompletionToken,
        class: FailureClass,
    ) {
        let Some(identity) = backend.runtime_identity() else {
            return;
        };
        let Some(membership) = backend.runtime_membership_identity() else {
            return;
        };
        let service_key = identity.service_key.as_ref();
        let Some(svc) = self.services.get(service_key).map(|entry| Arc::clone(entry.value())) else {
            return;
        };
        if !Arc::ptr_eq(&svc.policy_identity, &token.policy_identity) {
            return;
        }
        let _ = coordinator.with_current_outlier_endpoint(
            service_key,
            membership,
            identity,
            &token.policy_identity,
            |runtime, total| {
                // Register only after the Coordinator has established that this
                // exact identity is still active. A late completion must not
                // recreate retired outlier state after activation revoked it.
                let existing = svc
                    .identity_endpoints
                    .get(identity.as_ref())
                    .map(|entry| Arc::clone(entry.value()));
                let endpoint = existing.unwrap_or_else(|| {
                    Arc::clone(
                        svc.identity_endpoints
                            .entry(identity.as_ref().clone())
                            .or_insert_with(|| PassiveEjectionState::new(Some(svc.config.clone())))
                            .value(),
                    )
                });
                if endpoint.expire_legacy_ejection() {
                    runtime.clear_outlier_ejected();
                }
                let Some(candidate) = endpoint.record_failure_candidate(class) else {
                    return;
                };
                let transition = if svc.config.max_ejection_percent == 0 {
                    endpoint.finish_failure(candidate, true)
                } else {
                    // Candidate revalidation, percent check, and the open commit
                    // share one service-policy lock. Dropping this guard before
                    // finish_failure lets concurrent identities over-eject.
                    let _guard = svc.eject_lock.lock();
                    #[cfg(test)]
                    svc.percent_arbitrations.fetch_add(1, Ordering::Relaxed);
                    let currently_ejected = svc
                        .identity_endpoints
                        .iter()
                        .filter(|entry| entry.value().is_open())
                        .count();
                    let max = (total as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
                    endpoint.finish_failure(candidate, currently_ejected < max.max(1))
                };
                if transition == FailureTransition::Opened {
                    if let Some(remaining) = endpoint.legacy_ejection_remaining() {
                        runtime.set_outlier_ejected_for(remaining, Arc::clone(&endpoint));
                        runtime.set_gauge_accounted(true);
                        crate::observe::metrics::record_cb_ejection(service_key, class.metric_label());
                    }
                }
            },
        );
    }

    pub fn record_success_for_backend_with_token(&self, backend: &LbBackend, token: &OutlierCompletionToken) {
        self.record_success_for_backend_in(get_service_discovery_coordinator().as_ref(), backend, token);
    }

    pub(crate) fn record_success_for_backend_in(
        &self,
        coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
        backend: &LbBackend,
        token: &OutlierCompletionToken,
    ) {
        let Some(identity) = backend.runtime_identity() else {
            return;
        };
        let Some(membership) = backend.runtime_membership_identity() else {
            return;
        };
        let service_key = identity.service_key.as_ref();
        let Some(svc) = self.services.get(service_key).map(|entry| Arc::clone(entry.value())) else {
            return;
        };
        if !Arc::ptr_eq(&svc.policy_identity, &token.policy_identity) {
            return;
        }
        let Some(endpoint) = svc
            .identity_endpoints
            .get(identity.as_ref())
            .map(|entry| Arc::clone(entry.value()))
        else {
            return;
        };
        let _ = coordinator.with_current_outlier_endpoint(
            service_key,
            membership,
            identity,
            &token.policy_identity,
            |runtime, _active_endpoint_count| {
                endpoint.record_success();
                runtime.clear_outlier_ejected();
            },
        );
    }

    pub(crate) fn remove_runtime_identity(&self, identity: &EndpointRuntimeIdentity) {
        let service_key = identity.service_key.as_ref();
        let Some(service) = self.services.get(service_key) else {
            return;
        };
        service.identity_endpoints.remove(identity);
    }

    #[cfg(test)]
    pub(crate) fn has_runtime_identity(&self, identity: &EndpointRuntimeIdentity) -> bool {
        self.services
            .get(identity.service_key.as_ref())
            .is_some_and(|service| service.identity_endpoints.contains_key(identity))
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.contains_key(service_key)
    }
}

impl Default for OutlierDetectionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/backends/outlier_detection/store_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/backends/outlier_detection/store_test/stats_tests.rs"]
mod stats_tests;
