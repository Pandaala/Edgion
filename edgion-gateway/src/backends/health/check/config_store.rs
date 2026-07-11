use arc_swap::ArcSwap;
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static HC_CONFIG_STORE: LazyLock<Arc<HealthCheckConfigStore>> =
    LazyLock::new(|| Arc::new(HealthCheckConfigStore::new()));

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
    configs: ArcSwap<HashMap<String, ActiveHealthCheckConfig>>,
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
        }
    }

    /// Resolve the active health check config for a Service key, if any.
    pub fn resolve(&self, service_key: &str) -> Option<ActiveHealthCheckConfig> {
        self.configs.load().get(service_key).cloned()
    }

    /// Set or clear the config for a Service key. `None` removes any entry.
    pub fn set_service_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let mut new_map = (**self.configs.load()).clone();
        match config {
            Some(cfg) => {
                new_map.insert(service_key.to_string(), cfg);
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.configs.store(Arc::new(new_map));
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
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = HealthCheckConfigStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};

    fn test_config(tp: HealthCheckType) -> ActiveHealthCheckConfig {
        ActiveHealthCheckConfig {
            r#type: tp,
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_none() {
        let store = HealthCheckConfigStore::new();
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_set_then_resolve_returns_config() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.r#type, HealthCheckType::Http);
    }

    #[test]
    fn test_set_none_removes() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
        assert!(store.resolve("default/svc").is_some());

        store.set_service_config("default/svc", None);
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_remove_service_is_set_none() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.remove_service("default/svc");
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_overwrite_replaces_config() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.r#type, HealthCheckType::Tcp);
    }

    #[test]
    fn test_keys_and_count_track_entries() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/a", Some(test_config(HealthCheckType::Http)));
        store.set_service_config("default/b", Some(test_config(HealthCheckType::Tcp)));
        assert_eq!(store.count(), 2);
        let mut keys = store.keys();
        keys.sort();
        assert_eq!(keys, vec!["default/a".to_string(), "default/b".to_string()]);
    }
}
