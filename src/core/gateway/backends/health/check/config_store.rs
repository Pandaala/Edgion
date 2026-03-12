use crate::types::resources::health_check::ActiveHealthCheckConfig;
use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static HC_CONFIG_STORE: LazyLock<Arc<HealthCheckConfigStore>> =
    LazyLock::new(|| Arc::new(HealthCheckConfigStore::new()));

pub fn get_hc_config_store() -> Arc<HealthCheckConfigStore> {
    HC_CONFIG_STORE.clone()
}

/// Resolved health check config for a service, including selected source.
#[derive(Debug, Clone)]
pub struct ResolvedHealthCheckConfig {
    pub config: ActiveHealthCheckConfig,
    pub source: HealthCheckConfigSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckConfigSource {
    Service,
    EndpointSlice,
    Endpoint,
}

/// Resolve and store service health check configs with priority:
/// EndpointSlice > Endpoint > Service.
pub struct HealthCheckConfigStore {
    service_configs: ArcSwap<HashMap<String, ActiveHealthCheckConfig>>,
    endpoint_slice_configs: ArcSwap<HashMap<String, ActiveHealthCheckConfig>>,
    endpoint_configs: ArcSwap<HashMap<String, ActiveHealthCheckConfig>>,
}

impl Default for HealthCheckConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheckConfigStore {
    pub fn new() -> Self {
        Self {
            service_configs: ArcSwap::from_pointee(HashMap::new()),
            endpoint_slice_configs: ArcSwap::from_pointee(HashMap::new()),
            endpoint_configs: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    pub fn resolve(&self, service_key: &str) -> Option<ResolvedHealthCheckConfig> {
        if let Some(config) = self.endpoint_slice_configs.load().get(service_key) {
            return Some(ResolvedHealthCheckConfig {
                config: config.clone(),
                source: HealthCheckConfigSource::EndpointSlice,
            });
        }

        if let Some(config) = self.endpoint_configs.load().get(service_key) {
            return Some(ResolvedHealthCheckConfig {
                config: config.clone(),
                source: HealthCheckConfigSource::Endpoint,
            });
        }

        self.service_configs
            .load()
            .get(service_key)
            .map(|config| ResolvedHealthCheckConfig {
                config: config.clone(),
                source: HealthCheckConfigSource::Service,
            })
    }

    pub fn all_configured_services(&self) -> Vec<String> {
        let mut keys = HashSet::new();
        keys.extend(self.service_configs.load().keys().cloned());
        keys.extend(self.endpoint_slice_configs.load().keys().cloned());
        keys.extend(self.endpoint_configs.load().keys().cloned());
        keys.into_iter().collect()
    }

    pub fn service_keys(&self) -> Vec<String> {
        self.service_configs.load().keys().cloned().collect()
    }

    pub fn endpoint_slice_keys(&self) -> Vec<String> {
        self.endpoint_slice_configs.load().keys().cloned().collect()
    }

    pub fn endpoint_keys(&self) -> Vec<String> {
        self.endpoint_configs.load().keys().cloned().collect()
    }

    pub fn set_service_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let mut new_map = (**self.service_configs.load()).clone();
        match config {
            Some(cfg) => {
                new_map.insert(service_key.to_string(), cfg);
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.service_configs.store(Arc::new(new_map));
    }

    pub fn set_endpoint_slice_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let mut new_map = (**self.endpoint_slice_configs.load()).clone();
        match config {
            Some(cfg) => {
                new_map.insert(service_key.to_string(), cfg);
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.endpoint_slice_configs.store(Arc::new(new_map));
    }

    pub fn set_endpoint_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let mut new_map = (**self.endpoint_configs.load()).clone();
        match config {
            Some(cfg) => {
                new_map.insert(service_key.to_string(), cfg);
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.endpoint_configs.store(Arc::new(new_map));
    }

    pub fn remove_service(&self, service_key: &str) {
        self.set_service_config(service_key, None);
        self.set_endpoint_slice_config(service_key, None);
        self.set_endpoint_config(service_key, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};

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
    fn test_resolve_service_only() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.source, HealthCheckConfigSource::Service);
        assert_eq!(resolved.config.r#type, HealthCheckType::Http);
    }

    #[test]
    fn test_resolve_endpoint_only() {
        let store = HealthCheckConfigStore::new();
        store.set_endpoint_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.source, HealthCheckConfigSource::Endpoint);
        assert_eq!(resolved.config.r#type, HealthCheckType::Tcp);
    }

    #[test]
    fn test_resolve_endpoint_slice_priority() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.set_endpoint_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
        store.set_endpoint_slice_config("default/svc", Some(test_config(HealthCheckType::Http)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.source, HealthCheckConfigSource::EndpointSlice);
    }

    #[test]
    fn test_remove_endpoint_fallback_to_service() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.set_endpoint_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

        store.set_endpoint_config("default/svc", None);
        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.source, HealthCheckConfigSource::Service);
        assert_eq!(resolved.config.r#type, HealthCheckType::Http);
    }

    #[test]
    fn test_remove_service_with_endpoint_keeps_endpoint() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.set_endpoint_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

        store.set_service_config("default/svc", None);
        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.source, HealthCheckConfigSource::Endpoint);
        assert_eq!(resolved.config.r#type, HealthCheckType::Tcp);
    }
}
