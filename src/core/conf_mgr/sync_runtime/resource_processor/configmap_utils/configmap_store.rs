//! Global ConfigMap Store
//!
//! Lightweight store for ConfigMap resources, primarily used by
//! BackendTLSPolicy to resolve `caCertificateRefs` with `kind: ConfigMap`.

use k8s_openapi::api::core::v1::ConfigMap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

type ConfigMapMap = HashMap<String, ConfigMap>;

pub struct ConfigMapStore {
    configmaps: RwLock<ConfigMapMap>,
}

impl ConfigMapStore {
    pub fn new() -> Self {
        Self {
            configmaps: RwLock::new(HashMap::new()),
        }
    }

    pub fn get(&self, namespace: Option<&str>, name: &str) -> Option<ConfigMap> {
        let key = Self::make_key(namespace, name);
        let map = self.configmaps.read().unwrap();
        map.get(&key).cloned()
    }

    pub fn replace_all(&self, configmaps: HashMap<String, ConfigMap>) {
        let count = configmaps.len();
        let mut map = self.configmaps.write().unwrap();
        *map = configmaps;
        tracing::info!(component = "configmap_store", count = count, "Replaced all configmaps");
    }

    pub fn update(&self, upsert: HashMap<String, ConfigMap>, remove: &HashSet<String>) {
        let mut map = self.configmaps.write().unwrap();
        for key in remove {
            map.remove(key);
        }
        for (key, cm) in upsert {
            map.insert(key, cm);
        }
    }

    fn make_key(namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        }
    }
}

impl Default for ConfigMapStore {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_CONFIGMAP_STORE: LazyLock<Arc<ConfigMapStore>> = LazyLock::new(|| Arc::new(ConfigMapStore::new()));

pub fn get_global_configmap_store() -> Arc<ConfigMapStore> {
    GLOBAL_CONFIGMAP_STORE.clone()
}

pub fn get_configmap(namespace: Option<&str>, name: &str) -> Option<ConfigMap> {
    get_global_configmap_store().get(namespace, name)
}

pub fn replace_all_configmaps(configmaps: HashMap<String, ConfigMap>) {
    get_global_configmap_store().replace_all(configmaps);
}

pub fn update_configmaps(upsert: HashMap<String, ConfigMap>, remove: &HashSet<String>) {
    get_global_configmap_store().update(upsert, remove);
}
