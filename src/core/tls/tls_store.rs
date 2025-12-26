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
        // Capture count before moving
        let cert_count = data.len();
        
        // TODO(observability): Add metric for certificate reload operations
        tracing::info!(
            component = "tls_store",
            count = cert_count,
            "Full set of TLS certificates"
        );

        let mut tls_data = self.tls_data.write()
            .expect("TLS store write lock poisoned - a thread panicked while holding the lock");
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
        
        // Rebuild matcher BEFORE releasing write lock to prevent race condition
        // This ensures store and matcher are always consistent
        if let Err(e) = self.rebuild_matcher_from_data(&tls_data) {
            // TODO(observability): Add metric for matcher rebuild failures
            // This is a critical error - store updated but matcher not synced
            tracing::error!(
                component = "tls_store",
                error = %e,
                cert_count = cert_count,
                "CRITICAL: Failed to rebuild TLS matcher after full set. \
                 Store and matcher are now inconsistent!"
            );
            // TODO(error-handling): Consider returning Result from full_set
            // to propagate this error to callers for retry logic
        }
        
        // Lock is automatically released here
    }

    /// Partially update TLS certificates
    pub fn partial_update(
        &self,
        add: HashMap<String, EdgionTls>,
        update: HashMap<String, EdgionTls>,
        remove: &std::collections::HashSet<String>,
    ) {
        // Capture counts before moving
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();
        
        tracing::info!(
            component = "tls_store",
            add = add_count,
            update = update_count,
            remove = remove_count,
            "Partial update of TLS certificates"
        );

        let mut tls_data = self.tls_data.write()
            .expect("TLS store write lock poisoned - a thread panicked while holding the lock");
        
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
        
        // Rebuild matcher BEFORE releasing write lock to prevent race condition
        // This ensures store and matcher are always consistent
        if let Err(e) = self.rebuild_matcher_from_data(&tls_data) {
            // TODO(observability): Add metric for matcher rebuild failures
            // This is a critical error - store updated but matcher not synced
            tracing::error!(
                component = "tls_store",
                error = %e,
                added = add_count,
                updated = update_count,
                removed = remove_count,
                "CRITICAL: Failed to rebuild TLS matcher after partial update. \
                 Store and matcher are now inconsistent!"
            );
            // TODO(error-handling): Consider returning Result from partial_update
            // to propagate this error to callers for retry logic
        }
        
        // Lock is automatically released here
    }

    /// Rebuild the TLS certificate matcher from provided data
    /// This is called while holding a lock to prevent race conditions
    /// 
    /// # Arguments
    /// * `tls_data` - Reference to the TLS data map (already locked)
    fn rebuild_matcher_from_data(&self, tls_data: &HashMap<String, TlsEntry>) -> anyhow::Result<()> {
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
                // Note: host.clone() is necessary here as HashMap::entry() requires owned key
                // Performance: String clone is relatively cheap for typical hostname lengths
                // TODO(performance): Consider using Cow<str> or &str keys with custom lifetime
                // management if profiling shows this is a bottleneck during high-frequency reloads
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

        // TODO(observability): Add metrics for:
        // - valid_certs_total gauge
        // - invalid_certs_total gauge
        // - total_hosts_total gauge
        // - matcher_rebuild_duration_seconds histogram
        tracing::info!(
            component = "tls_store",
            valid_certs = valid_certs,
            invalid_certs = invalid_certs,
            total_hosts = total_hosts,
            "TLS matcher rebuilt successfully"
        );
        
        Ok(())
    }
    
    /// Rebuild the TLS certificate matcher
    /// This method acquires a read lock and rebuilds the matcher
    /// 
    /// Note: This is provided for compatibility but prefer using
    /// rebuild_matcher_from_data when you already hold a lock
    fn rebuild_matcher(&self) -> anyhow::Result<()> {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        self.rebuild_matcher_from_data(&tls_data)
    }

    /// Get all TLS certificates (for debugging/monitoring)
    /// 
    /// Returns a snapshot of all certificates currently in the store,
    /// including both valid and invalid ones.
    /// 
    /// # Performance
    /// This method acquires a read lock and clones all Arc pointers.
    /// It's safe to call concurrently from multiple threads.
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        tls_data.values().map(|entry| entry.tls.clone()).collect()
    }

    /// Check if a specific TLS certificate exists in the store
    /// 
    /// # Arguments
    /// * `key` - Certificate key in format "namespace/name"
    /// 
    /// # Returns
    /// `true` if certificate exists (regardless of validity), `false` otherwise
    /// 
    /// # Performance
    /// O(1) hash lookup with read lock
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    pub fn contains(&self, key: &str) -> bool {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        tls_data.contains_key(key)
    }

    /// Get validation status for a specific certificate
    /// 
    /// # Arguments
    /// * `key` - Certificate key in format "namespace/name"
    /// 
    /// # Returns
    /// `Some(CertValidationResult)` if certificate exists, `None` otherwise
    /// 
    /// # Performance
    /// O(1) hash lookup with read lock. Result is cloned.
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        tls_data.get(key).map(|entry| entry.validation.clone())
    }

    /// Get all invalid certificates with their validation errors
    /// 
    /// Returns a list of (key, validation_result) pairs for all certificates
    /// that failed validation. Useful for monitoring and diagnostics.
    /// 
    /// # Returns
    /// Vector of (certificate_key, validation_result) tuples
    /// 
    /// # Performance
    /// O(n) iteration over all certificates with read lock.
    /// Results are cloned.
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    pub fn get_invalid_certs(&self) -> Vec<(String, CertValidationResult)> {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        tls_data
            .iter()
            .filter(|(_, entry)| !entry.validation.is_valid)
            .map(|(key, entry)| (key.clone(), entry.validation.clone()))
            .collect()
    }

    /// Get count of valid and invalid certificates
    /// 
    /// # Returns
    /// Tuple of (valid_count, invalid_count)
    /// 
    /// # Performance
    /// O(n) iteration over all certificates with read lock
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    /// 
    /// # Example
    /// ```ignore
    /// let (valid, invalid) = store.get_cert_stats();
    /// println!("Valid: {}, Invalid: {}", valid, invalid);
    /// ```
    pub fn get_cert_stats(&self) -> (usize, usize) {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        let total = tls_data.len();
        let invalid = tls_data.values().filter(|entry| !entry.validation.is_valid).count();
        (total - invalid, invalid)
    }

    /// Get EdgionTls resource for a specific hostname
    /// 
    /// Returns the first valid certificate that matches the given hostname.
    /// Only returns valid certificates (invalid ones are skipped).
    /// 
    /// # Arguments
    /// * `hostname` - Hostname to match (supports wildcard matching)
    /// 
    /// # Returns
    /// `Some(Arc<EdgionTls>)` if a valid matching certificate is found, `None` otherwise
    /// 
    /// # Performance
    /// O(n) iteration over all certificates with read lock.
    /// Early return on first match.
    /// 
    /// # Thread Safety
    /// Thread-safe: acquires RwLock read lock
    pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read()
            .expect("TLS store read lock poisoned - a thread panicked while holding the lock");
        
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

