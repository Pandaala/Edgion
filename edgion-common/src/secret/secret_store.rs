//! Global Secret Store
//!
//! Provides a global store for Secret resources that can be accessed
//! from anywhere in the application (e.g., TLS callback).
//!
//! Write frequency is low (controller-driven reconcile),
//! so a simple RwLock<HashMap> is sufficient.

use k8s_openapi::api::core::v1::Secret;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

/// Type alias for the secret map: "namespace/name" -> Secret
type SecretMap = HashMap<String, Secret>;

/// Global Secret Store
pub struct SecretStore {
    secrets: RwLock<SecretMap>,
}

impl SecretStore {
    pub fn new() -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
        }
    }

    /// Get a Secret by namespace and name
    pub fn get(&self, namespace: Option<&str>, name: &str) -> Option<Secret> {
        let key = Self::make_key(namespace, name);
        let map = self.secrets.read();
        map.get(&key).cloned()
    }

    /// Replace all secrets
    pub fn replace_all(&self, secrets: HashMap<String, Secret>) {
        let count = secrets.len();
        let mut map = self.secrets.write();
        *map = secrets;
        tracing::info!(component = "secret_store", count = count, "Replaced all secrets");
    }

    /// Update secrets
    ///
    /// - `upsert`: Secrets to add or update (insert if not exists, update if exists)
    /// - `remove`: Keys of secrets to remove
    pub fn update(&self, upsert: HashMap<String, Secret>, remove: &HashSet<String>) {
        let mut map = self.secrets.write();

        for key in remove {
            map.remove(key);
        }

        for (key, secret) in upsert {
            map.insert(key, secret);
        }

        tracing::debug!(component = "secret_store", "Updated secrets in store");
    }

    /// Number of secrets currently in the store.
    pub fn count(&self) -> usize {
        self.secrets.read().len()
    }

    /// Stable-sorted resource identities without Secret values.
    ///
    /// This is used by the integration-only Controller diagnostics to prove
    /// cache cleanup by identity without exposing credential material.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = self.secrets.read().keys().cloned().collect::<Vec<_>>();
        keys.sort();
        keys
    }

    /// Create a key from namespace and name
    fn make_key(namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        }
    }
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Global Secret Store instance (used by Gateway-side code for fallback access).
///
/// NOTE: Controller-side code uses `ControllerRuntime::secret_store` instead.
/// This global is kept for Gateway binary compatibility where plugins may fall
/// back to reading secrets directly (e.g., BasicAuth fallback when resolved_users
/// is absent).
static GLOBAL_SECRET_STORE: LazyLock<Arc<SecretStore>> = LazyLock::new(|| Arc::new(SecretStore::new()));

/// Get the global Secret Store
///
/// Gateway-side use only. Controller code should use `runtime.secret_store`.
pub fn get_global_secret_store() -> Arc<SecretStore> {
    GLOBAL_SECRET_STORE.clone()
}

/// Get a Secret by namespace and name from the global store
///
/// Gateway-side use only. Controller code should use `runtime.secret_store.get(...)`.
pub fn get_secret(namespace: Option<&str>, name: &str) -> Option<Secret> {
    get_global_secret_store().get(namespace, name)
}

/// Alias for [`get_secret`].
pub fn get_secret_by_name(namespace: Option<&str>, name: &str) -> Option<Secret> {
    get_secret(namespace, name)
}

#[cfg(test)]
#[path = "../../tests/unit/secret/secret_store_test/stats_tests.rs"]
mod stats_tests;

#[cfg(test)]
#[path = "../../tests/unit/secret/secret_store_test.rs"]
mod tests;
