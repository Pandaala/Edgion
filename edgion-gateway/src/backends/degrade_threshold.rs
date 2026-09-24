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

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/27_static_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/30_struct_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/33_impl_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/44_struct_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/51_struct_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/57_impl_item.rs");

#[cfg(test)]
include!("../../tests/support/backends/degrade_threshold_test/71_impl_item.rs");

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
#[path = "../../tests/unit/backends/degrade_threshold_test.rs"]
mod tests;
