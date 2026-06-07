//! Load-balancing panic threshold: per-service override store, process-global
//! default, and the pure resolution / floor math.
//!
//! Panic mode engages when the fraction of backends passing the combined
//! health predicate drops below the effective threshold percent. Threshold
//! resolves as: `edgion.io/panic-threshold` annotation (per service) → the
//! `EdgionGatewayConfig.loadBalancing.panicThreshold` global default → 0.
//! A threshold of 0 disables panic mode (opt-in; matches Istio's k8s default).

use crate::types::constants::annotations::edgion::PANIC_THRESHOLD;
use dashmap::DashMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};

static PANIC_THRESHOLD_STORE: LazyLock<Arc<PanicThresholdStore>> =
    LazyLock::new(|| Arc::new(PanicThresholdStore::new()));

/// Process-global default panic threshold percent, set from EdgionGatewayConfig.
/// Initialized to 0 (panic disabled) so the feature is strictly opt-in.
static GLOBAL_DEFAULT: AtomicU8 = AtomicU8::new(0);

pub fn get_panic_threshold_store() -> Arc<PanicThresholdStore> {
    PANIC_THRESHOLD_STORE.clone()
}

/// Per-service panic-threshold overrides parsed from the Service annotation.
pub struct PanicThresholdStore {
    services: DashMap<String, u8>,
}

impl PanicThresholdStore {
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
    }

    pub fn remove_service(&self, service_key: &str) {
        self.services.remove(service_key);
    }

    fn get_override(&self, service_key: &str) -> Option<u8> {
        self.services.get(service_key).map(|v| *v)
    }
}

impl Default for PanicThresholdStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Set the process-global default panic threshold (called when EdgionGatewayConfig
/// is applied). Values > 100 are invalid and treated as 0 (disabled).
pub fn configure_lb_panic_default(threshold: u8) {
    let effective = if threshold > 100 {
        tracing::warn!(threshold, "Invalid global panicThreshold (>100), disabling panic mode");
        0
    } else {
        threshold
    };
    GLOBAL_DEFAULT.store(effective, Ordering::Relaxed);
    tracing::info!(threshold = effective, "LB global panic threshold updated");
}

pub fn get_lb_panic_default() -> u8 {
    GLOBAL_DEFAULT.load(Ordering::Relaxed)
}

/// Resolve the effective threshold for a service: per-service override, else the
/// process-global default.
pub fn resolve_panic_threshold(service_key: &str) -> u8 {
    get_panic_threshold_store()
        .get_override(service_key)
        .unwrap_or_else(get_lb_panic_default)
}

/// Survivor floor: the minimum number of healthy backends required to *avoid*
/// panic. `ceil(threshold * total / 100)`. Panic engages when survivors < floor.
/// `threshold` is assumed already validated to 0..=100; `total` >= 1.
pub fn panic_floor(threshold: u8, total: usize) -> usize {
    (threshold as usize * total).div_ceil(100)
}

/// Parse the `edgion.io/panic-threshold` annotation: a bare integer percent
/// string in 0..=100. Anything else (non-integer, > 100) is logged and ignored
/// so the service falls back to the global default.
pub fn parse_panic_threshold_annotation(meta: &ObjectMeta) -> Option<u8> {
    let raw = meta.annotations.as_ref()?.get(PANIC_THRESHOLD)?;
    match raw.trim().parse::<u8>() {
        Ok(v) if v <= 100 => Some(v),
        _ => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                value = %raw,
                "Invalid edgion.io/panic-threshold annotation (want integer 0-100), ignoring"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn meta_with(value: Option<&str>) -> ObjectMeta {
        let mut ann = BTreeMap::new();
        if let Some(v) = value {
            ann.insert(PANIC_THRESHOLD.to_string(), v.to_string());
        }
        ObjectMeta {
            name: Some("svc".to_string()),
            annotations: if ann.is_empty() { None } else { Some(ann) },
            ..Default::default()
        }
    }

    #[test]
    fn floor_is_ceil() {
        assert_eq!(panic_floor(50, 10), 5); // 50% of 10
        assert_eq!(panic_floor(50, 3), 2); // ceil(1.5)
        assert_eq!(panic_floor(100, 10), 10); // require all healthy
        assert_eq!(panic_floor(1, 1), 1);
    }

    #[test]
    fn annotation_parse_valid_and_invalid() {
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("50"))), Some(50));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("0"))), Some(0));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("100"))), Some(100));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("101"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("50%"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("abc"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(None)), None);
    }

    #[test]
    fn resolve_prefers_override_then_global() {
        let store = get_panic_threshold_store();
        configure_lb_panic_default(30);
        store.set_service_threshold("default/a", Some(70));
        assert_eq!(resolve_panic_threshold("default/a"), 70);
        assert_eq!(resolve_panic_threshold("default/no-override"), 30);
        store.set_service_threshold("default/a", None);
        assert_eq!(resolve_panic_threshold("default/a"), 30);
        configure_lb_panic_default(0); // reset global for other tests
    }

    #[test]
    fn global_default_clamps_invalid() {
        configure_lb_panic_default(200);
        assert_eq!(get_lb_panic_default(), 0);
        configure_lb_panic_default(0);
    }
}
