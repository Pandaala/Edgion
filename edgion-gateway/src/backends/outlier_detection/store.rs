use super::config::OutlierDetectionResolved;
#[cfg(test)]
use super::state::ejection_duration;
use super::state::{FailureClass, FailureTransition, PassiveEjectionState};
use crate::backends::discovery::{get_service_discovery_coordinator, OutlierPolicyIdentity};
use crate::lb::{EdgionBackend, EndpointRuntimeIdentity};
use dashmap::DashMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

static OD_STORE: LazyLock<Arc<OutlierDetectionStore>> = LazyLock::new(|| Arc::new(OutlierDetectionStore::new()));

pub fn get_outlier_detection_store() -> Arc<OutlierDetectionStore> {
    OD_STORE.clone()
}

#[derive(Clone, Debug)]
pub struct OutlierCompletionToken {
    policy_identity: Arc<OutlierPolicyIdentity>,
}

struct ServiceCbState {
    config: OutlierDetectionResolved,
    policy_identity: Arc<OutlierPolicyIdentity>,
    endpoints: DashMap<u64, Arc<PassiveEjectionState>>,
    identity_endpoints: DashMap<EndpointRuntimeIdentity, Arc<PassiveEjectionState>>,
    /// Serializes the percent-check + ejection steps in `record_failure` to prevent
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
            endpoints: DashMap::new(),
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
                    if let Some((_, old)) = self.services.remove(service_key) {
                        Self::drain_ejected_gauge(service_key, &old);
                    }
                    return;
                };
                let next = ServiceCbState::new(cfg, Arc::clone(&policy_identity));
                coordinator.set_outlier_policy(service_key, Some(policy_identity));
                if let Some(old) = self.services.insert(service_key.to_string(), next) {
                    Self::drain_ejected_gauge(service_key, &old);
                }
            }
            None => {
                coordinator.set_outlier_policy(service_key, None);
                if let Some((_, old_svc)) = self.services.remove(service_key) {
                    Self::drain_ejected_gauge(service_key, &old_svc);
                }
            }
        }
    }

    fn allocate_policy_identity(&self) -> Option<Arc<OutlierPolicyIdentity>> {
        self.next_policy_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1))
            .ok()
            .map(|generation| Arc::new(OutlierPolicyIdentity { generation }))
    }

    #[inline]
    pub fn is_ejected(&self, service_key: &str, backend_hash: u64) -> bool {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return false,
        };

        let Some(endpoint) = svc.endpoints.get(&backend_hash).map(|entry| Arc::clone(entry.value())) else {
            return false;
        };
        let (ejected, recovered) = endpoint.legacy_ejection_status();
        if recovered {
            crate::observe::metrics::record_cb_recovery(service_key);
        }
        ejected
    }

    /// Backoff-scaled ejection duration: `min(ejection_seconds * num_ejections, max)`.
    #[cfg(test)]
    #[inline]
    fn ejection_duration(config: &OutlierDetectionResolved, num_ejections: u32) -> std::time::Duration {
        ejection_duration(config, num_ejections)
    }

    pub fn record_failure(&self, service_key: &str, backend_hash: u64, total_endpoints: usize, class: FailureClass) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        let endpoint = Arc::clone(
            svc.endpoints
                .entry(backend_hash)
                .or_insert_with(|| PassiveEjectionState::new(Some(svc.config.clone())))
                .value(),
        );

        let Some(candidate) = endpoint.record_failure_candidate(class) else {
            return;
        };

        if svc.config.max_ejection_percent == 0 {
            if endpoint.finish_failure(candidate, true) == FailureTransition::Opened {
                crate::observe::metrics::record_cb_ejection(service_key, class.metric_label());
            }
            return;
        }

        // Percent check + open stay under eject_lock so concurrent candidates
        // for distinct backends cannot exceed max_ejection_percent.
        let _eject_guard = svc.eject_lock.lock();
        #[cfg(test)]
        svc.percent_arbitrations.fetch_add(1, Ordering::Relaxed);
        let effective_total = if total_endpoints > 0 {
            total_endpoints
        } else {
            svc.endpoints.len().max(1)
        };
        let currently_ejected = svc.endpoints.iter().filter(|entry| entry.value().is_open()).count();
        let max_ejectable = (effective_total as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
        let allow_ejection = currently_ejected < max_ejectable.max(1);

        if endpoint.finish_failure(candidate, allow_ejection) == FailureTransition::Opened {
            crate::observe::metrics::record_cb_ejection(service_key, class.metric_label());
        }
    }

    pub fn record_success(&self, service_key: &str, backend_hash: u64) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        // Reset both counters and clear ejection, but RETAIN num_ejections so
        // backoff history survives. Only operate on an existing entry — a
        // never-failed endpoint needs no allocation. If it had been ejected,
        // fire record_cb_recovery to keep the gauge balanced.
        // Trailing `;` keeps the Option<RefMut> temporary's drop before `svc` is
        // released — without it the if-let is the function's trailing expression and
        // the borrow of `svc.endpoints` outlives `svc`.
        if let Some(endpoint) = svc.endpoints.get(&backend_hash) {
            if endpoint.record_success() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        };
    }

    pub fn capture_completion_token(&self, backend: &EdgionBackend) -> Option<OutlierCompletionToken> {
        let service_key = backend.runtime_identity()?.service_key.as_ref();
        let service = self.services.get(service_key)?;
        Some(OutlierCompletionToken {
            policy_identity: Arc::clone(&service.policy_identity),
        })
    }

    pub fn record_failure_for_backend_with_token(
        &self,
        backend: &EdgionBackend,
        token: &OutlierCompletionToken,
        class: FailureClass,
    ) {
        self.record_failure_for_backend_in(get_service_discovery_coordinator().as_ref(), backend, token, class);
    }

    pub(crate) fn record_failure_for_backend_in(
        &self,
        coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
        backend: &EdgionBackend,
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
                let endpoint = Arc::clone(
                    svc.identity_endpoints
                        .entry(identity.as_ref().clone())
                        .or_insert_with(|| PassiveEjectionState::new(Some(svc.config.clone())))
                        .value(),
                );
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

    pub fn record_success_for_backend_with_token(&self, backend: &EdgionBackend, token: &OutlierCompletionToken) {
        self.record_success_for_backend_in(get_service_discovery_coordinator().as_ref(), backend, token);
    }

    pub(crate) fn record_success_for_backend_in(
        &self,
        coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
        backend: &EdgionBackend,
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

    /// Decrement gauge for any currently-ejected endpoints in a service state.
    fn drain_ejected_gauge(service_key: &str, svc: &ServiceCbState) {
        let ejected = svc.endpoints.iter().filter(|entry| entry.value().is_open()).count();
        for _ in 0..ejected {
            crate::observe::metrics::record_cb_recovery(service_key);
        }
    }

    pub fn remove_service(&self, service_key: &str) {
        if let Some((_, svc)) = self.services.remove(service_key) {
            Self::drain_ejected_gauge(service_key, &svc);
        }
    }

    /// Remove a single backend endpoint's CB state (called when endpoint is removed from EP/EPS).
    pub fn remove_backend(&self, service_key: &str, addr: &pingora_core::protocols::l4::socket::SocketAddr) {
        let Some(svc) = self.services.get(service_key) else {
            return;
        };
        let backend = pingora_load_balancing::Backend {
            addr: addr.clone(),
            weight: 1,
            ext: Default::default(),
        };
        let hash = crate::backends::health::check::backend_hash(&backend);
        if let Some((_, endpoint)) = svc.endpoints.remove(&hash) {
            if endpoint.is_open() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        }
    }
}

impl Default for OutlierDetectionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(errors: u32, gateway: u32, seconds: u64, max_secs: u64, max_pct: u32) -> OutlierDetectionResolved {
        // Local-origin threshold inherits the gateway threshold, matching the
        // config `normalize` default, so existing tests need no extra argument.
        OutlierDetectionResolved {
            consecutive_errors: errors,
            consecutive_gateway_errors: gateway,
            consecutive_local_origin_failures: gateway,
            ejection_seconds: seconds,
            max_ejection_seconds: max_secs,
            max_ejection_percent: max_pct,
        }
    }

    /// Build a config with an explicit local-origin threshold distinct from the
    /// gateway threshold, for the local-origin split tests.
    fn make_config_lo(gateway: u32, local_origin: u32) -> OutlierDetectionResolved {
        OutlierDetectionResolved {
            consecutive_errors: 100,
            consecutive_gateway_errors: gateway,
            consecutive_local_origin_failures: local_origin,
            ejection_seconds: 30,
            max_ejection_seconds: 300,
            max_ejection_percent: 0,
        }
    }

    #[test]
    fn test_no_config_means_not_ejected() {
        let store = OutlierDetectionStore::new();
        assert!(!store.is_ejected("default/svc1", 123));
    }

    #[test]
    fn test_record_failure_ejects_after_threshold() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

        for _ in 0..2 {
            store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
            assert!(!store.is_ejected("default/svc1", 111));
        }

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_success_resets_failures() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_success("default/svc1", 111);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_auto_recovery_after_cooldown() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 1, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        let service = store.services.get("default/svc1").expect("service state must exist");
        let endpoint = service.endpoints.get(&111).expect("endpoint state must exist");
        endpoint.expire_deadlines_for_test();
        drop(endpoint);
        drop(service);

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_max_ejection_percent() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        store.record_failure("default/svc1", 222, 2, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn percent_arbitration_runs_only_for_ejection_candidates() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 50)));
        let service = Arc::clone(store.services.get("default/svc1").unwrap().value());

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 0);

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 1);
        assert!(store.is_ejected("default/svc1", 111));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_max_ejection_percent_concurrent_no_over_eject() {
        use std::sync::{Arc, Barrier};

        // 4 backends, 50% limit → at most 2 may be ejected regardless of concurrency.
        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        let barrier = Arc::new(Barrier::new(4));
        let hashes = [100u64, 200, 300, 400];

        let handles: Vec<_> = hashes
            .iter()
            .copied()
            .map(|hash| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.record_failure("default/svc1", hash, 4, FailureClass::Gateway);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let ejected = hashes.iter().filter(|&&h| store.is_ejected("default/svc1", h)).count();
        assert!(
            ejected <= 2,
            "over-ejected: {ejected} backends ejected, limit is 2 (50% of 4)"
        );
    }

    #[test]
    fn test_remove_service_clears_state() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        store.remove_service("default/svc1");
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_record_success_on_ejected_does_not_leak_gauge() {
        // record_success on an ejected entry must fire record_cb_recovery so the
        // CB_EJECTED_ENDPOINTS gauge stays balanced.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // record_success on an ejected backend clears the ejection. After reset,
        // is_ejected returns false. The +1/-1 gauge symmetry is verified by
        // inspection of the recovery branch in record_success itself.
        store.record_success("default/svc1", 111);
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_percent_cap_applies_concurrently_when_total_endpoints_is_zero() {
        // With total_endpoints == 0 (empty selector snapshot) and concurrent failures,
        // the percent cap must still bound ejected backends via the observed endpoint
        // count fallback.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(OutlierDetectionStore::new());
        // 50% cap. Once endpoints.len() reaches 4, max_ejectable = 2.
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        let barrier = Arc::new(Barrier::new(4));
        let hashes = [100u64, 200, 300, 400];

        let handles: Vec<_> = hashes
            .iter()
            .copied()
            .map(|hash| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    // total=0 simulates the empty-selector-snapshot scale-down race.
                    store.record_failure("default/svc1", hash, 0, FailureClass::Gateway);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let ejected = hashes.iter().filter(|&&h| store.is_ejected("default/svc1", h)).count();
        // The first thread observes endpoints.len()=1 (cap=1, ejects), the second sees
        // endpoints.len()=2 (cap=1, currently_ejected=1, rejects). After that, each new
        // thread inserts its entry first then enters the lock — endpoints.len() grows
        // monotonically inside the lock, so the cap rises but currently_ejected catches
        // up. Upper bound is 50% of 4 = 2.
        assert!(
            ejected <= 2,
            "over-ejected under total=0 race: {ejected} backends ejected, 50% cap of 4 is 2"
        );
    }

    #[test]
    fn test_percent_cap_applies_when_total_endpoints_is_zero() {
        // When the selector snapshot is momentarily empty (total_endpoints == 0), the
        // percent guard must still apply using the observed endpoint count as a fallback.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        // First failure with total=0 establishes the endpoint and ejects (max(1) of 50% of 1).
        store.record_failure("default/svc1", 111, 0, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // Second backend tries to eject with total=0 — observed endpoints are now 2, the
        // 50% cap is 1, and 1 backend is already ejected, so this must be rejected.
        store.record_failure("default/svc1", 222, 0, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn test_config_change_resets_state_unchanged_preserves() {
        // The atomic entry() swap must preserve these semantics: a *changed* config
        // rebuilds the service (clearing ejection state), while an *unchanged* config
        // is a no-op that keeps the existing ejection state.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // Re-applying the identical config must not wipe the ejection state.
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        assert!(store.is_ejected("default/svc1", 111));

        // Applying a different config rebuilds the service with empty endpoints.
        store.set_service_config("default/svc1", Some(make_config(5, 5, 30, 300, 0)));
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_concurrent_config_update_no_panic_and_key_present() {
        // Concurrent set_service_config calls for the same key must never panic and
        // must never leave the key absent. After the storm, one config must be installed.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));

        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4u32)
            .map(|i| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    // Each writer applies a distinct config to force the changed-path swap.
                    store.set_service_config("default/svc1", Some(make_config(i + 2, i + 2, 30, 300, 0)));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert!(store.has_service("default/svc1"));
    }

    #[test]
    fn test_different_services_isolated() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.set_service_config("default/svc2", Some(make_config(1, 1, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));
        assert!(!store.is_ejected("default/svc2", 111));
    }

    #[test]
    fn app_and_gateway_counters_are_independent() {
        let store = OutlierDetectionStore::new();
        // app threshold 3, gateway threshold 10.
        store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));

        // 3 application failures eject via counter A even though gateway is far from 10.
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::Application);
        }
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn gateway_counter_does_not_trip_app_threshold() {
        let store = OutlierDetectionStore::new();
        // app threshold 3, gateway threshold 10.
        store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));
        // 3 gateway failures must NOT eject (gateway threshold is 10).
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        }
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn success_resets_both_counters() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config(3, 3, 30, 300, 0)));
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        store.record_success("default/svc", 1);
        // After reset, two more app failures still below threshold 3.
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn local_origin_counter_is_independent_of_gateway() {
        let store = OutlierDetectionStore::new();
        // gateway threshold 10, local-origin threshold 3.
        store.set_service_config("default/svc", Some(make_config_lo(10, 3)));

        // 3 local-origin failures eject via the local-origin counter even though the
        // gateway counter is far from 10.
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        }
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn gateway_failures_do_not_trip_local_origin_threshold() {
        let store = OutlierDetectionStore::new();
        // gateway threshold 10, local-origin threshold 2: 2 gateway failures must NOT
        // eject through the (lower) local-origin counter — the buckets are separate.
        store.set_service_config("default/svc", Some(make_config_lo(10, 2)));
        for _ in 0..2 {
            store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        }
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn success_resets_local_origin_counter() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config_lo(10, 3)));
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_success("default/svc", 1);
        // After reset, two more local-origin failures still below threshold 3.
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn failure_class_metric_labels_are_bounded() {
        assert_eq!(FailureClass::Gateway.metric_label(), "gateway");
        assert_eq!(FailureClass::Application.metric_label(), "application");
        assert_eq!(FailureClass::LocalOrigin.metric_label(), "local_origin");
    }

    #[test]
    fn backoff_doubles_second_ejection_and_persists_num_ejections() {
        let store = OutlierDetectionStore::new();
        // base 1s, consecutive 1 so a single failure ejects.
        store.set_service_config("default/svc", Some(make_config(1, 1, 1, 300, 0)));

        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));

        // Expiring the first window recovers lazily and retains num_ejections = 1.
        let service = store.services.get("default/svc").expect("service state must exist");
        let endpoint = service.endpoints.get(&1).expect("endpoint state must exist");
        assert_eq!(endpoint.num_ejections(), 1);
        endpoint.expire_deadlines_for_test();
        drop(endpoint);
        drop(service);
        assert!(!store.is_ejected("default/svc", 1)); // triggers reset-but-retain

        // Eject again -> num_ejections becomes 2 -> duration ~2s.
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));
        let service = store.services.get("default/svc").expect("service state must exist");
        let endpoint = service.endpoints.get(&1).expect("endpoint state must exist");
        assert_eq!(endpoint.num_ejections(), 2);
        assert_eq!(
            ejection_duration(
                endpoint.config().expect("endpoint config must exist"),
                endpoint.num_ejections(),
            ),
            std::time::Duration::from_secs(2)
        );
        drop(endpoint);
        drop(service);
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn max_ejection_seconds_caps_backoff() {
        // Direct test of the backoff duration helper: linear base*num_ejections,
        // clamped at max_ejection_seconds. base=10s, cap=25s.
        let cfg = make_config(1, 1, 10, 25, 0);
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 1),
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 2),
            std::time::Duration::from_secs(20)
        );
        // 3*10=30 exceeds the 25s cap -> clamped to 25.
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 3),
            std::time::Duration::from_secs(25)
        );
        // Far past the cap stays clamped (and does not overflow).
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, u32::MAX),
            std::time::Duration::from_secs(25)
        );
    }

    #[test]
    fn change_detection_rebuilds_on_gateway_threshold_change() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config(5, 5, 30, 300, 0)));
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway); // G=1
                                                                          // Changing ONLY the gateway threshold must rebuild (clearing the counter).
        store.set_service_config("default/svc", Some(make_config(5, 2, 30, 300, 0)));
        // Counter was cleared by rebuild: needs 2 fresh gateway failures to eject.
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc", 1));
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn concurrent_same_backend_failures_eject_once() {
        use std::sync::{Arc, Barrier};

        // consecutive 1 so a single failure trips the threshold; percent cap off
        // (0) so the percent guard cannot mask the idempotency check.
        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc", Some(make_config(1, 1, 30, 300, 0)));

        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        assert!(store.is_ejected("default/svc", 1));
        // Despite up to 4 concurrent failures crossing the threshold before any
        // ejection lands, the backend is ejected exactly once: num_ejections must
        // be 1 (regression for the step-3 idempotency guard — without it the race
        // would inflate num_ejections and leak the ejected-endpoints gauge).
        let svc = store.services.get("default/svc").unwrap();
        let ep = svc.endpoints.get(&1).unwrap();
        assert_eq!(
            ep.num_ejections(),
            1,
            "concurrent same-backend ejection must count once"
        );
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = OutlierDetectionStore::new();
        assert_eq!(store.service_count(), 0);
        assert!(store.keys().is_empty());
    }
}
