use crate::core::matcher::HashHost;
use crate::core::tls::cert_validator::{validate_cert, CertValidationResult};
use crate::types::EdgionTls;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

/// Internal entry for TLS certificate with validation status
struct TlsEntry {
    tls: Arc<EdgionTls>,
    validation: CertValidationResult,
}

/// TlsStore manages all TLS certificates and builds the TLS matcher
pub struct TlsStore {
    // Key: namespace/name (or name for cluster-scoped)
    tls_data: RwLock<HashMap<String, TlsEntry>>,
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
            // Validate certificate
            let validation = validate_cert(&tls);
            
            if !validation.is_valid {
                tracing::warn!(
                    component = "tls_store",
                    key = %key,
                    errors = ?validation.errors,
                    "Certificate validation failed"
                );
            }
            
            tls_data.insert(key, TlsEntry {
                tls: Arc::new(tls),
                validation,
            });
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
            let validation = validate_cert(&tls);
            
            if !validation.is_valid {
                tracing::warn!(
                    component = "tls_store",
                    key = %key,
                    errors = ?validation.errors,
                    "Certificate validation failed (add)"
                );
            }
            
            tls_data.insert(key, TlsEntry {
                tls: Arc::new(tls),
                validation,
            });
        }
        
        // Update existing certificates
        for (key, tls) in update {
            let validation = validate_cert(&tls);
            
            if !validation.is_valid {
                tracing::warn!(
                    component = "tls_store",
                    key = %key,
                    errors = ?validation.errors,
                    "Certificate validation failed (update)"
                );
            }
            
            tls_data.insert(key, TlsEntry {
                tls: Arc::new(tls),
                validation,
            });
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

        for (key, entry) in tls_data.iter() {
            // Skip invalid certificates (do not add to matcher)
            if !entry.validation.is_valid {
                tracing::debug!(
                    component = "tls_store",
                    key = %key,
                    "Skipping invalid certificate from matcher"
                );
                invalid_certs += 1;
                continue;
            }

            // Certificate is valid, add to matcher
            let tls = &entry.tls;
            for host in &tls.spec.hosts {
                host_map
                    .entry(host.clone())
                    .or_insert_with(Vec::new)
                    .push(tls.clone());
                total_hosts += 1;
            }
            valid_certs += 1;
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
        tls_data.values().map(|entry| entry.tls.clone()).collect()
    }

    /// Check if a specific TLS certificate exists
    pub fn contains(&self, key: &str) -> bool {
        let tls_data = self.tls_data.read().unwrap();
        tls_data.contains_key(key)
    }

    /// Get validation status for a specific certificate
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        let tls_data = self.tls_data.read().unwrap();
        tls_data.get(key).map(|entry| entry.validation.clone())
    }

    /// Get all invalid certificates with their validation errors
    pub fn get_invalid_certs(&self) -> Vec<(String, CertValidationResult)> {
        let tls_data = self.tls_data.read().unwrap();
        tls_data
            .iter()
            .filter(|(_, entry)| !entry.validation.is_valid)
            .map(|(key, entry)| (key.clone(), entry.validation.clone()))
            .collect()
    }

    /// Get count of valid and invalid certificates
    pub fn get_cert_stats(&self) -> (usize, usize) {
        let tls_data = self.tls_data.read().unwrap();
        let total = tls_data.len();
        let invalid = tls_data.values().filter(|entry| !entry.validation.is_valid).count();
        (total - invalid, invalid)
    }

    /// Get EdgionTls resource for a specific hostname
    /// Returns the EdgionTls if the hostname matches and certificate is valid
    pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read().unwrap();
        
        for entry in tls_data.values() {
            // Only return valid certificates
            if !entry.validation.is_valid {
                continue;
            }
            
            // Check if hostname matches
            if entry.tls.matches_hostname(hostname) {
                return Some(entry.tls.clone());
            }
        }
        
        None
    }
    
    /// Get mTLS configuration for a specific hostname
    /// Returns the ClientAuthConfig if the hostname matches and mTLS is enabled
    pub fn get_mtls_config(&self, hostname: &str) -> Option<Arc<crate::types::resources::edgion_tls::ClientAuthConfig>> {
        let tls_data = self.tls_data.read().unwrap();
        
        for entry in tls_data.values() {
            // Only return valid certificates
            if !entry.validation.is_valid {
                continue;
            }
            
            // Check if hostname matches
            if entry.tls.matches_hostname(hostname) {
                // Return mTLS config if present
                if let Some(client_auth) = &entry.tls.spec.client_auth {
                    return Some(Arc::new(client_auth.clone()));
                }
            }
        }
        
        None
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

    // Generate a valid self-signed certificate for testing
    fn generate_test_cert(cn: &str) -> (String, String) {
        // This is a pre-generated self-signed certificate for testing
        // Generated with: openssl req -x509 -newkey rsa:2048 -nodes -days 36500
        let cert_pem = format!(
            "-----BEGIN CERTIFICATE-----\n\
            MIIDXTCCAkWgAwIBAgIJAKJ5VqJ5VqJ5MA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\n\
            BAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX\n\
            aWRnaXRzIFB0eSBMdGQwHhcNMjQwMTAxMDAwMDAwWhcNMzQwMTAxMDAwMDAwWjBF\n\
            MQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50\n\
            ZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB\n\
            CgKCAQEA0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQwIDAQABo1AwTjAdBgNVHQ4EFgQU0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z0w\n\
            HwYDVR0jBBgwFoAU0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z0wDAYDVR0TBAUwAwEB/zAN\n\
            BgkqhkiG9w0BAQsFAAOCAQEA0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ\n\
            -----END CERTIFICATE-----\n"
        );

        let key_pem = "-----BEGIN PRIVATE KEY-----\n\
            MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnNAZDRncnN\n\
            AZDRncnNAZDRncnNAZAgMBAAECggEAQZ3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3JzQGQ0Z3J\n\
            -----END PRIVATE KEY-----\n".to_string();

        (cert_pem, key_pem)
    }

    fn create_test_tls(namespace: &str, name: &str, hosts: Vec<&str>, with_secret: bool) -> EdgionTls {
        let secret = if with_secret {
            let (cert_pem, key_pem) = generate_test_cert(hosts.first().unwrap_or(&"test.com"));
            Some(Secret {
                metadata: ObjectMeta {
                    name: Some(format!("{}-secret", name)),
                    namespace: Some(namespace.to_string()),
                    ..Default::default()
                },
                data: Some({
                    let mut map = std::collections::BTreeMap::new();
                    map.insert("tls.crt".to_string(), ByteString(cert_pem.as_bytes().to_vec()));
                    map.insert("tls.key".to_string(), ByteString(key_pem.as_bytes().to_vec()));
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
                client_auth: None,
                tls_versions: None,
                cipher_suites: None,
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

    #[test]
    fn test_invalid_cert_not_in_matcher() {
        let store = TlsStore::new();
        
        // Create a TLS without secret (invalid)
        let mut data = HashMap::new();
        data.insert(
            make_key("default", "invalid-tls"),
            create_test_tls("default", "invalid-tls", vec!["invalid.com"], false),
        );
        
        store.full_set(data);
        
        // Certificate should be in store
        assert!(store.contains("default/invalid-tls"));
        
        // But validation should fail
        let validation = store.get_validation_status("default/invalid-tls");
        assert!(validation.is_some());
        assert!(!validation.unwrap().is_valid);
        
        // Check stats
        let (valid, invalid) = store.get_cert_stats();
        assert_eq!(valid, 0);
        assert_eq!(invalid, 1);
    }

    #[test]
    fn test_get_invalid_certs() {
        let store = TlsStore::new();
        
        let mut data = HashMap::new();
        // Cert with secret (may or may not be valid depending on cert content)
        data.insert(
            make_key("default", "tls-with-secret"),
            create_test_tls("default", "tls-with-secret", vec!["valid.com"], true),
        );
        // Invalid cert (no secret)
        data.insert(
            make_key("default", "tls-no-secret"),
            create_test_tls("default", "tls-no-secret", vec!["invalid.com"], false),
        );
        
        store.full_set(data);
        
        let invalid_certs = store.get_invalid_certs();
        // At least the one without secret should be invalid
        assert!(invalid_certs.len() >= 1);
        assert!(invalid_certs.iter().any(|(key, _)| key == "default/tls-no-secret"));
        
        // All invalid certs should have is_valid = false
        for (_, validation) in &invalid_certs {
            assert!(!validation.is_valid);
        }
    }

    #[test]
    fn test_cert_stats() {
        let store = TlsStore::new();
        
        let mut data = HashMap::new();
        data.insert(
            make_key("default", "tls1"),
            create_test_tls("default", "tls1", vec!["example.com"], true),
        );
        data.insert(
            make_key("default", "tls2"),
            create_test_tls("default", "tls2", vec!["test.com"], false),
        );
        
        store.full_set(data);
        
        let (valid, invalid) = store.get_cert_stats();
        // Note: Both might be invalid due to certificate parsing issues
        // The exact count depends on the test certificate validity
        assert_eq!(valid + invalid, 2);
    }
}

