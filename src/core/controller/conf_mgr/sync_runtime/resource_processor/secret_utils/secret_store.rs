//! Global Secret Store
//!
//! Provides a global store for Secret resources that can be accessed
//! from anywhere in the application (e.g., TLS callback).
//!
//! This is control-plane code with low read/write frequency,
//! so a simple RwLock<HashMap> is sufficient.

use k8s_openapi::api::core::v1::Secret;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

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
        let map = self.secrets.read().unwrap();
        map.get(&key).cloned()
    }

    /// Replace all secrets
    pub fn replace_all(&self, secrets: HashMap<String, Secret>) {
        let count = secrets.len();
        let mut map = self.secrets.write().unwrap();
        *map = secrets;
        tracing::info!(component = "secret_store", count = count, "Replaced all secrets");
    }

    /// Update secrets
    ///
    /// - `upsert`: Secrets to add or update (insert if not exists, update if exists)
    /// - `remove`: Keys of secrets to remove
    pub fn update(&self, upsert: HashMap<String, Secret>, remove: &HashSet<String>) {
        let mut map = self.secrets.write().unwrap();

        // Remove secrets first
        for key in remove {
            map.remove(key);
        }

        // Add or update secrets
        for (key, secret) in upsert {
            map.insert(key, secret);
        }

        tracing::debug!(component = "secret_store", "Updated secrets in store");
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

/// Global Secret Store instance
static GLOBAL_SECRET_STORE: LazyLock<Arc<SecretStore>> = LazyLock::new(|| Arc::new(SecretStore::new()));

/// Get the global Secret Store
pub fn get_global_secret_store() -> Arc<SecretStore> {
    GLOBAL_SECRET_STORE.clone()
}

/// Get a Secret by namespace and name from the global store
pub fn get_secret(namespace: Option<&str>, name: &str) -> Option<Secret> {
    get_global_secret_store().get(namespace, name)
}

/// Backward compatibility alias for get_secret
pub fn get_secret_by_name(namespace: Option<&str>, name: &str) -> Option<Secret> {
    get_secret(namespace, name)
}

/// Replace all secrets in the global store
pub fn replace_all_secrets(secrets: HashMap<String, Secret>) {
    get_global_secret_store().replace_all(secrets);
}

/// Update secrets in the global store
///
/// - `upsert`: Secrets to add or update
/// - `remove`: Keys of secrets to remove
pub fn update_secrets(upsert: HashMap<String, Secret>, remove: &HashSet<String>) {
    get_global_secret_store().update(upsert, remove);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::constants::secret_keys::tls::{CERT, KEY};
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

        // Initially empty
        assert!(store.get(Some("default"), "my-secret").is_none());

        // Add a secret
        let secret = create_test_secret("default", "my-secret", "cert-pem", "key-pem");
        let mut secrets = HashMap::new();
        secrets.insert("default/my-secret".to_string(), secret);
        store.replace_all(secrets);

        // Should find it now
        let found = store.get(Some("default"), "my-secret");
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.metadata.name.as_deref(), Some("my-secret"));
        assert_eq!(found.metadata.namespace.as_deref(), Some("default"));

        // Verify data
        let data = found.data.as_ref().unwrap();
        let cert = data.get(CERT).unwrap();
        assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "cert-pem");
    }

    #[test]
    fn test_secret_store_update() {
        let store = SecretStore::new();

        // Initial secret
        let secret1 = create_test_secret("prod", "cert-1", "cert1", "key1");
        let mut initial = HashMap::new();
        initial.insert("prod/cert-1".to_string(), secret1);
        store.replace_all(initial);

        // Add new secret (upsert)
        let secret2 = create_test_secret("prod", "cert-2", "cert2", "key2");
        let mut upsert = HashMap::new();
        upsert.insert("prod/cert-2".to_string(), secret2);
        store.update(upsert, &HashSet::new());

        // Both secrets should exist
        assert!(store.get(Some("prod"), "cert-1").is_some());
        assert!(store.get(Some("prod"), "cert-2").is_some());

        // Update cert-1 (upsert existing)
        let secret1_updated = create_test_secret("prod", "cert-1", "updated-cert", "updated-key");
        let mut upsert = HashMap::new();
        upsert.insert("prod/cert-1".to_string(), secret1_updated);
        store.update(upsert, &HashSet::new());

        // Verify update
        let found = store.get(Some("prod"), "cert-1").unwrap();
        let data = found.data.as_ref().unwrap();
        let cert = data.get(CERT).unwrap();
        assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "updated-cert");

        // Remove cert-2
        let mut remove = HashSet::new();
        remove.insert("prod/cert-2".to_string());
        store.update(HashMap::new(), &remove);

        // cert-2 should be gone
        assert!(store.get(Some("prod"), "cert-2").is_none());
        // cert-1 should still exist
        assert!(store.get(Some("prod"), "cert-1").is_some());
    }

    #[test]
    fn test_secret_store_no_namespace() {
        let store = SecretStore::new();

        // Secret without namespace (cluster-scoped)
        let secret = create_test_secret("", "cluster-secret", "cert", "key");
        let mut secrets = HashMap::new();
        secrets.insert("cluster-secret".to_string(), secret);
        store.replace_all(secrets);

        // Find by name only
        let found = store.get(None, "cluster-secret");
        assert!(found.is_some());
    }
}
