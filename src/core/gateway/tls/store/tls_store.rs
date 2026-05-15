use crate::core::common::matcher::HashHost;
use crate::core::gateway::tls::validation::cert::{validate_cert, CertValidationResult};
use crate::types::listener_key::ListenerKey;
use crate::types::EdgionTls;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Internal entry for TLS certificate with validation status
#[derive(Clone)]
struct TlsEntry {
    tls: Arc<EdgionTls>,
    validation: CertValidationResult,
}

/// TlsStore manages all TLS certificates and builds the TLS matcher
pub struct TlsStore {
    // Key: namespace/name (or name for cluster-scoped).
    // Single writer (config-sync). Readers are lock-free.
    tls_data: ArcSwap<HashMap<String, TlsEntry>>,
}

impl TlsStore {
    pub fn new() -> Self {
        Self {
            tls_data: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Replace all TLS certificates with the provided data
    pub fn replace_all_tls(&self, data: HashMap<String, EdgionTls>) {
        // Capture count before moving
        let cert_count = data.len();

        tracing::info!(
            component = "tls_store",
            count = cert_count,
            "Full set of TLS certificates"
        );

        let mut new_map: HashMap<String, TlsEntry> = HashMap::with_capacity(cert_count);

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

            new_map.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                },
            );
        }

        self.rebuild_matcher_from_data(&new_map);

        self.tls_data.store(Arc::new(new_map));
    }

    /// Partially update TLS certificates
    pub fn apply_partial_update(
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

        let current = self.tls_data.load_full();
        let mut new_map = (*current).clone();

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

            new_map.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                },
            );
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

            new_map.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                },
            );
        }

        // Remove certificates
        for key in remove {
            new_map.remove(key);
        }

        self.rebuild_matcher_from_data(&new_map);

        self.tls_data.store(Arc::new(new_map));
    }

    /// Rebuild the TLS certificate matcher from provided data.
    /// Only listener-resolved certs are admitted into the matcher.
    ///
    /// # Arguments
    /// * `tls_data` - Reference to the TLS data map
    fn rebuild_matcher_from_data(&self, tls_data: &HashMap<String, TlsEntry>) {
        let mut listener_host_map: HashMap<ListenerKey, HashMap<String, Vec<Arc<EdgionTls>>>> = HashMap::new();
        let mut total_hosts = 0;
        let mut valid_certs = 0;
        let mut invalid_certs = 0;
        let mut skipped_unbound_certs = 0;

        for (key, entry) in tls_data.iter() {
            if !entry.validation.is_valid {
                tracing::debug!(
                    component = "tls_store",
                    key = %key,
                    "Skipping invalid certificate from matcher"
                );
                invalid_certs += 1;
                continue;
            }

            let tls = &entry.tls;

            match &tls.spec.resolved_listeners {
                Some(listeners) if !listeners.is_empty() => {
                    for listener_key in listeners {
                        let host_map = listener_host_map.entry(listener_key.clone()).or_default();
                        for host in &tls.spec.hosts {
                            host_map.entry(host.clone()).or_default().push(tls.clone());
                            total_hosts += 1;
                        }
                    }
                    valid_certs += 1;
                }
                _ => {
                    skipped_unbound_certs += 1;
                    tracing::warn!(
                        component = "tls_store",
                        key = %key,
                        hosts = ?tls.spec.hosts,
                        parent_refs = ?tls.spec.parent_refs,
                        "Skipping EdgionTls from matcher because no listener keys were resolved"
                    );
                }
            }
        }

        let mut listener_matcher = HashMap::new();
        for (listener_key, host_map) in listener_host_map {
            let mut matcher = HashHost::new();
            for (host, tls_list) in host_map {
                matcher.insert(&host, tls_list);
            }
            listener_matcher.insert(listener_key, matcher);
        }

        super::cert_matcher::get_tls_cert_matcher().set(listener_matcher);

        tracing::info!(
            component = "tls_store",
            valid_certs = valid_certs,
            invalid_certs = invalid_certs,
            skipped_unbound_certs = skipped_unbound_certs,
            total_hosts = total_hosts,
            "TLS matcher rebuilt successfully"
        );
    }

    /// Get all TLS certificates (for debugging/monitoring)
    ///
    /// Returns a snapshot of all certificates currently in the store,
    /// including both valid and invalid ones.
    ///
    /// # Performance
    /// Lock-free: clones Arc pointers from the current snapshot.
    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        let snapshot = self.tls_data.load();
        snapshot.values().map(|entry| entry.tls.clone()).collect()
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
    /// Lock-free: O(1) hash lookup on the current snapshot.
    pub fn contains(&self, key: &str) -> bool {
        let snapshot = self.tls_data.load();
        snapshot.contains_key(key)
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
    /// Lock-free: O(1) hash lookup on the current snapshot. Result is cloned.
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        let snapshot = self.tls_data.load();
        snapshot.get(key).map(|entry| entry.validation.clone())
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
    /// Lock-free: O(n) iteration over the current snapshot. Results are cloned.
    pub fn get_invalid_certs(&self) -> Vec<(String, CertValidationResult)> {
        let snapshot = self.tls_data.load();
        snapshot
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
    /// Lock-free: O(n) iteration over the current snapshot.
    ///
    /// # Example
    /// ```ignore
    /// let (valid, invalid) = store.get_cert_stats();
    /// println!("Valid: {}, Invalid: {}", valid, invalid);
    /// ```
    pub fn get_cert_stats(&self) -> (usize, usize) {
        let snapshot = self.tls_data.load();
        let total = snapshot.len();
        let invalid = snapshot.values().filter(|entry| !entry.validation.is_valid).count();
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
    /// Lock-free: O(n) iteration over the current snapshot. Early return on first match.
    pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
        let snapshot = self.tls_data.load();

        for entry in snapshot.values() {
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

impl TlsStore {
    /// Total number of TLS entries (valid + invalid).
    pub fn entry_count(&self) -> usize {
        self.tls_data.load().len()
    }
}

impl Default for TlsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Global TlsStore instance
static GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>> = LazyLock::new(|| Arc::new(TlsStore::new()));

/// Get the global TlsStore instance
pub fn get_global_tls_store() -> Arc<TlsStore> {
    GLOBAL_TLS_STORE.clone()
}
