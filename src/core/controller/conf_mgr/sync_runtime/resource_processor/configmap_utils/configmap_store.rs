//! ConfigMap Store
//!
//! Lightweight store for ConfigMap resources, primarily used by
//! BackendTLSPolicy to resolve `caCertificateRefs` with `kind: ConfigMap`.

use k8s_openapi::api::core::v1::ConfigMap;
use std::collections::{HashMap, HashSet};
use parking_lot::RwLock;

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
        let map = self.configmaps.read();
        map.get(&key).cloned()
    }

    pub fn replace_all(&self, configmaps: HashMap<String, ConfigMap>) {
        let count = configmaps.len();
        let mut map = self.configmaps.write();
        *map = configmaps;
        tracing::info!(component = "configmap_store", count = count, "Replaced all configmaps");
    }

    /// Clear all stored ConfigMaps. Called by `ControllerRuntime::reset()` on
    /// leader-change or controller restart to prevent stale entries from
    /// persisting across watch-stream reconnects. The ConfigMap handler uses
    /// delta-only `update()` calls (no full `replace_all` on reconnect), so
    /// an explicit clear is required.
    pub fn clear(&self) {
        self.configmaps.write().clear();
        tracing::info!(component = "configmap_store", "Cleared all configmaps");
    }

    pub fn update(&self, upsert: HashMap<String, ConfigMap>, remove: &HashSet<String>) {
        let mut map = self.configmaps.write();
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
