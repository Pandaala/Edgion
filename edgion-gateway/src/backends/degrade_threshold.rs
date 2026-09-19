//! Load-balancing degrade threshold: per-service override store, process-global
//! default, and the pure resolution / floor math.
//!
//! Degrade mode engages when the fraction of backends passing the combined
//! health predicate drops below the effective threshold percent. The gateway
//! then ignores health filtering and load-balances across all backends.
//! Threshold resolves as: the `EdgionBackendTrafficPolicy` policy-store override
//! for the service (`loadBalancer.degradeThreshold`, written by
//! `backends/policy/backend_traffic/store.rs`) → the
//! `EdgionGatewayConfig.loadBalancing.degradeThreshold` global default → 0.
//! A threshold of 0 disables degrade mode (opt-in). Envoy documents the same
//! fail-open behavior as "panic threshold".

use dashmap::DashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};

static DEGRADE_THRESHOLD_STORE: LazyLock<Arc<DegradeThresholdStore>> =
    LazyLock::new(|| Arc::new(DegradeThresholdStore::new()));

/// Process-global default degrade threshold percent, set from EdgionGatewayConfig.
/// Initialized to 0 (degrade disabled) so the feature is strictly opt-in.
static GLOBAL_DEFAULT: AtomicU8 = AtomicU8::new(0);

/// Serializes cross-module global-store tests and restores the degrade default
/// and overrides on both normal return and unwind.
#[cfg(test)]
pub(crate) static GLOBAL_STORE_TEST_LOCK: GlobalStoreTestLock = GlobalStoreTestLock(parking_lot::Mutex::new(()));

#[cfg(test)]
pub(crate) struct GlobalStoreTestLock(parking_lot::Mutex<()>);

#[cfg(test)]
impl GlobalStoreTestLock {
    pub(crate) fn lock(&self) -> GlobalStoreTestGuard<'_> {
        let lock = self.0.lock();
        GlobalStoreTestGuard {
            _state: DegradeTestState::capture(),
            _lock: lock,
        }
    }
}

#[cfg(test)]
pub(crate) struct GlobalStoreTestGuard<'a> {
    // Fields drop in declaration order: restore while the shared lock is held.
    _state: DegradeTestState,
    _lock: parking_lot::MutexGuard<'a, ()>,
}

#[cfg(test)]
struct DegradeTestState {
    default: u8,
    overrides: Vec<(String, u8)>,
}

#[cfg(test)]
impl DegradeTestState {
    fn capture() -> Self {
        Self {
            default: get_lb_degrade_default(),
            overrides: get_degrade_threshold_store()
                .services
                .iter()
                .map(|entry| (entry.key().clone(), *entry.value()))
                .collect(),
        }
    }
}

#[cfg(test)]
impl Drop for DegradeTestState {
    fn drop(&mut self) {
        let store = get_degrade_threshold_store();
        store.services.clear();
        for (key, value) in self.overrides.drain(..) {
            store.services.insert(key, value);
        }
        GLOBAL_DEFAULT.store(self.default, Ordering::Relaxed);
        crate::backends::discovery::get_service_discovery_coordinator().refresh_all_degrade_thresholds();
    }
}

pub fn get_degrade_threshold_store() -> Arc<DegradeThresholdStore> {
    DEGRADE_THRESHOLD_STORE.clone()
}

/// Per-service degrade-threshold overrides, written by the
/// `EdgionBackendTrafficPolicy` policy store (`loadBalancer.degradeThreshold`).
pub struct DegradeThresholdStore {
    services: DashMap<String, u8>,
}

impl DegradeThresholdStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Set or clear the override for a service. `None` removes any override so
    /// the service falls back to the global default.
    pub fn set_service_threshold(&self, service_key: &str, threshold: Option<u8>) {
        match threshold {
            Some(v) => {
                self.services.insert(service_key.to_string(), v);
            }
            None => {
                self.services.remove(service_key);
            }
        }
        crate::backends::discovery::get_service_discovery_coordinator().refresh_degrade_threshold(service_key);
    }

    fn get_override(&self, service_key: &str) -> Option<u8> {
        self.services.get(service_key).map(|v| *v)
    }
}

impl Default for DegradeThresholdStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Set the process-global default degrade threshold (called when EdgionGatewayConfig
/// is applied). Values > 100 are invalid and treated as 0 (disabled).
pub fn configure_lb_degrade_default(threshold: u8) {
    let effective = if threshold > 100 {
        tracing::warn!(
            threshold,
            "Invalid global degradeThreshold (>100), disabling degrade mode"
        );
        0
    } else {
        threshold
    };
    GLOBAL_DEFAULT.store(effective, Ordering::Relaxed);
    tracing::info!(threshold = effective, "LB global degrade threshold updated");
    crate::backends::discovery::get_service_discovery_coordinator().refresh_all_degrade_thresholds();
}

pub fn get_lb_degrade_default() -> u8 {
    GLOBAL_DEFAULT.load(Ordering::Relaxed)
}

/// Resolve the effective threshold for a service: per-service override, else the
/// process-global default.
pub fn resolve_degrade_threshold(service_key: &str) -> u8 {
    get_degrade_threshold_store()
        .get_override(service_key)
        .unwrap_or_else(get_lb_degrade_default)
}

/// Survivor floor: the minimum number of healthy backends required to *avoid*
/// degrade mode. `ceil(threshold * total / 100)`. Degrade engages when
/// survivors < floor. `threshold` is assumed already validated to 0..=100;
/// `total` >= 1.
pub fn degrade_floor(threshold: u8, total: usize) -> usize {
    (threshold as usize * total).div_ceil(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_is_ceil() {
        assert_eq!(degrade_floor(50, 10), 5); // 50% of 10
        assert_eq!(degrade_floor(50, 3), 2); // ceil(1.5)
        assert_eq!(degrade_floor(100, 10), 10); // require all healthy
        assert_eq!(degrade_floor(1, 1), 1);
    }

    #[test]
    fn resolve_prefers_override_then_global() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_degrade_threshold_store();
        configure_lb_degrade_default(30);
        store.set_service_threshold("default/a", Some(70));
        assert_eq!(resolve_degrade_threshold("default/a"), 70);
        assert_eq!(resolve_degrade_threshold("default/no-override"), 30);
        store.set_service_threshold("default/a", None);
        assert_eq!(resolve_degrade_threshold("default/a"), 30);
    }

    #[test]
    fn global_default_clamps_invalid() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(200);
        assert_eq!(get_lb_degrade_default(), 0);
    }

    #[test]
    fn test_state_restores_default_and_all_overrides_on_unwind() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_degrade_threshold_store();
        configure_lb_degrade_default(25);
        store.set_service_threshold("guard/retained", Some(40));
        store.set_service_threshold("guard/removed", Some(60));
        let snapshot = DegradeTestState::capture();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _restore = snapshot;
            configure_lb_degrade_default(90);
            store.set_service_threshold("guard/retained", Some(80));
            store.set_service_threshold("guard/removed", None);
            store.set_service_threshold("guard/added", Some(100));
            panic!("exercise unwind restoration");
        }));
        assert!(result.is_err());
        assert_eq!(get_lb_degrade_default(), 25);
        assert_eq!(store.get_override("guard/retained"), Some(40));
        assert_eq!(store.get_override("guard/removed"), Some(60));
        assert_eq!(store.get_override("guard/added"), None);
    }
}
