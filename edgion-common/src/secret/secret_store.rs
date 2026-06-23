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
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = SecretStore::new();
        assert_eq!(store.count(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;

    fn create_test_secret(namespace: &str, name: &str, cert: &str, key: &str) -> Secret {
        let mut data = std::collections::BTreeMap::new();
        data.insert(CERT.to_string(), ByteString(cert.as_bytes().to_vec()));
        data.insert(KEY.to_string(), ByteString(key.as_bytes().to_vec()));

        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        }
    }

    #[test]
    fn test_secret_store_basic() {
        let store = SecretStore::new();

        assert!(store.get(Some("default"), "my-secret").is_none());

        let secret = create_test_secret("default", "my-secret", "cert-pem", "key-pem");
        let mut secrets = HashMap::new();
        secrets.insert("default/my-secret".to_string(), secret);
        store.replace_all(secrets);

        let found = store.get(Some("default"), "my-secret");
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.metadata.name.as_deref(), Some("my-secret"));
        assert_eq!(found.metadata.namespace.as_deref(), Some("default"));

        let data = found.data.as_ref().unwrap();
        let cert = data.get(CERT).unwrap();
        assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "cert-pem");
    }

    #[test]
    fn test_secret_store_update() {
        let store = SecretStore::new();

        let secret1 = create_test_secret("prod", "cert-1", "cert1", "key1");
        let mut initial = HashMap::new();
        initial.insert("prod/cert-1".to_string(), secret1);
        store.replace_all(initial);

        let secret2 = create_test_secret("prod", "cert-2", "cert2", "key2");
        let mut upsert = HashMap::new();
        upsert.insert("prod/cert-2".to_string(), secret2);
        store.update(upsert, &HashSet::new());

        assert!(store.get(Some("prod"), "cert-1").is_some());
        assert!(store.get(Some("prod"), "cert-2").is_some());

        let secret1_updated = create_test_secret("prod", "cert-1", "updated-cert", "updated-key");
        let mut upsert = HashMap::new();
        upsert.insert("prod/cert-1".to_string(), secret1_updated);
        store.update(upsert, &HashSet::new());

        let found = store.get(Some("prod"), "cert-1").unwrap();
        let data = found.data.as_ref().unwrap();
        let cert = data.get(CERT).unwrap();
        assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "updated-cert");

        let mut remove = HashSet::new();
        remove.insert("prod/cert-2".to_string());
        store.update(HashMap::new(), &remove);

        assert!(store.get(Some("prod"), "cert-2").is_none());
        assert!(store.get(Some("prod"), "cert-1").is_some());
    }

    #[test]
    fn test_secret_store_no_namespace() {
        let store = SecretStore::new();

        let secret = create_test_secret("", "cluster-secret", "cert", "key");
        let mut secrets = HashMap::new();
        secrets.insert("cluster-secret".to_string(), secret);
        store.replace_all(secrets);

        let found = store.get(None, "cluster-secret");
        assert!(found.is_some());
    }
}
