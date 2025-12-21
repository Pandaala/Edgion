use crate::core::matcher::HashHost;
use crate::types::EdgionTls;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

/// TlsStore manages all TLS certificates and builds the TLS matcher
pub struct TlsStore {
    // Key: namespace/name (or name for cluster-scoped)
    tls_data: RwLock<HashMap<String, Arc<EdgionTls>>>,
}

impl TlsStore {
    pub fn new() -> Self {
        Self {
            tls_data: RwLock::new(HashMap::new()),
        }
    }

    /// Replace all TLS certificates with the provided data
    pub fn full_set(&self, data: HashMap<String, EdgionTls>) {
        tracing::info!(
            component = "tls_store",
            count = data.len(),
            "Full set of TLS certificates"
        );

        let mut tls_data = self.tls_data.write().unwrap();
        tls_data.clear();
        
        for (key, tls) in data {
            tls_data.insert(key, Arc::new(tls));
        }
        
        drop(tls_data);
        
        // Rebuild matcher after full set
        if let Err(e) = self.rebuild_matcher() {
            tracing::error!(
                component = "tls_store",
                error = %e,
                "Failed to rebuild TLS matcher after full set"
            );
        }
    }

    /// Partially update TLS certificates
    pub fn partial_update(
        &self,
        add: HashMap<String, EdgionTls>,
        update: HashMap<String, EdgionTls>,
        remove: &std::collections::HashSet<String>,
    ) {
        tracing::info!(
            component = "tls_store",
            add = add.len(),
            update = update.len(),
            remove = remove.len(),
            "Partial update of TLS certificates"
        );

        let mut tls_data = self.tls_data.write().unwrap();
        
        // Add new certificates
        for (key, tls) in add {
            tls_data.insert(key, Arc::new(tls));
        }
        
        // Update existing certificates
        for (key, tls) in update {
            tls_data.insert(key, Arc::new(tls));
        }
        
        // Remove certificates
        for key in remove {
            tls_data.remove(key);
        }
        
        drop(tls_data);
        
        // Rebuild matcher after partial update
        if let Err(e) = self.rebuild_matcher() {
            tracing::error!(
                component = "tls_store",
                error = %e,
                "Failed to rebuild TLS matcher after partial update"
            );
        }
    }

    /// Rebuild the TLS certificate matcher
    /// This method:
    /// 1. Iterates over all EdgionTls resources
    /// 2. Groups them by hostname (from spec.hosts)
    /// 3. Validates that each EdgionTls has a valid Secret
    /// 4. Updates the global TlsCertMatcher
    fn rebuild_matcher(&self) -> anyhow::Result<()> {
        let tls_data = self.tls_data.read().unwrap();
        
        let mut host_map: HashMap<String, Vec<Arc<EdgionTls>>> = HashMap::new();
        let mut total_hosts = 0;
        let mut valid_certs = 0;
        let mut invalid_certs = 0;

        for (key, tls) in tls_data.iter() {
            // Validate that this EdgionTls has a Secret
            if tls.spec.secret.is_none() {
                tracing::warn!(
                    component = "tls_store",
                    key = %key,
                    "EdgionTls has no Secret, skipping"
                );
                invalid_certs += 1;
                continue;
            }

            // Try to extract cert and key to validate
            match (tls.cert_pem(), tls.key_pem()) {
                (Ok(_cert), Ok(_key)) => {
                    // Certificate is valid, add to matcher
                    for host in &tls.spec.hosts {
                        host_map
                            .entry(host.clone())
                            .or_insert_with(Vec::new)
                            .push(tls.clone());
                        total_hosts += 1;
                    }
                    valid_certs += 1;
                }
                (Err(e), _) | (_, Err(e)) => {
                    tracing::warn!(
                        component = "tls_store",
                        key = %key,
                        error = %e,
                        "Failed to extract cert/key from EdgionTls, skipping"
                    );
                    invalid_certs += 1;
                }
            }
        }

        // Build HashHost matcher
        let mut matcher = HashHost::new();
        for (host, tls_list) in host_map {
            matcher.insert(&host, tls_list);
        }

        // Update global TlsCertMatcher
        super::tls_cert_matcher::set_tls_cert_matcher(matcher)?;

        tracing::info!(
            component = "tls_store",
            valid_certs = valid_certs,
            invalid_certs = invalid_certs,
            total_hosts = total_hosts,
            "TLS matcher rebuilt successfully"
        );

        Ok(())
    }

    /// Get all TLS certificates (for debugging/monitoring)
    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read().unwrap();
        tls_data.values().cloned().collect()
    }

    /// Check if a specific TLS certificate exists
    pub fn contains(&self, key: &str) -> bool {
        let tls_data = self.tls_data.read().unwrap();
        tls_data.contains_key(key)
    }
}

impl Default for TlsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Global TlsStore instance
static GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>> = 
    LazyLock::new(|| Arc::new(TlsStore::new()));

/// Get the global TlsStore instance
pub fn get_global_tls_store() -> Arc<TlsStore> {
    GLOBAL_TLS_STORE.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;
    use crate::types::resources::edgion_tls::EdgionTlsSpec;
    use crate::types::resources::gateway::SecretObjectReference;

    fn create_test_tls(namespace: &str, name: &str, hosts: Vec<&str>, with_secret: bool) -> EdgionTls {
        let secret = if with_secret {
            Some(Secret {
                metadata: ObjectMeta {
                    name: Some(format!("{}-secret", name)),
                    namespace: Some(namespace.to_string()),
                    ..Default::default()
                },
                data: Some({
                    let mut map = std::collections::BTreeMap::new();
                    map.insert("tls.crt".to_string(), ByteString(b"fake-cert".to_vec()));
                    map.insert("tls.key".to_string(), ByteString(b"fake-key".to_vec()));
                    map
                }),
                ..Default::default()
            })
        } else {
            None
        };

        EdgionTls {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: hosts.iter().map(|h| h.to_string()).collect(),
                secret_ref: SecretObjectReference {
                    name: format!("{}-secret", name),
                    namespace: Some(namespace.to_string()),
                    group: None,
                    kind: None,
                },
                secret,
            },
            status: None,
        }
    }

    fn make_key(namespace: &str, name: &str) -> String {
        format!("{}/{}", namespace, name)
    }

    #[test]
    fn test_full_set() {
        let store = TlsStore::new();
        
        let mut data = HashMap::new();
        data.insert(
            make_key("default", "tls1"),
            create_test_tls("default", "tls1", vec!["example.com"], true),
        );
        data.insert(
            make_key("default", "tls2"),
            create_test_tls("default", "tls2", vec!["test.com"], true),
        );
        
        store.full_set(data);
        
        assert!(store.contains("default/tls1"));
        assert!(store.contains("default/tls2"));
        assert!(!store.contains("default/tls3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = TlsStore::new();
        
        let mut add = HashMap::new();
        add.insert(
            make_key("default", "tls1"),
            create_test_tls("default", "tls1", vec!["example.com"], true),
        );
        
        store.partial_update(add, HashMap::new(), &std::collections::HashSet::new());
        
        assert!(store.contains("default/tls1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = TlsStore::new();
        
        // First add
        let mut data = HashMap::new();
        data.insert(
            make_key("default", "tls1"),
            create_test_tls("default", "tls1", vec!["example.com"], true),
        );
        store.full_set(data);
        assert!(store.contains("default/tls1"));
        
        // Then remove
        let mut remove = std::collections::HashSet::new();
        remove.insert(make_key("default", "tls1"));
        store.partial_update(HashMap::new(), HashMap::new(), &remove);
        
        assert!(!store.contains("default/tls1"));
    }

    #[test]
    fn test_list_all() {
        let store = TlsStore::new();
        
        let mut data = HashMap::new();
        data.insert(
            make_key("default", "tls1"),
            create_test_tls("default", "tls1", vec!["example.com"], true),
        );
        data.insert(
            make_key("default", "tls2"),
            create_test_tls("default", "tls2", vec!["test.com"], true),
        );
        
        store.full_set(data);
        
        let all = store.list_all();
        assert_eq!(all.len(), 2);
    }
}

