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
    ///
    /// # Panics
    /// This method will NOT panic on lock poisoning. Instead, it will log an error
    /// and attempt to recover the lock state.
    pub fn full_set(&self, data: HashMap<String, EdgionTls>) {
        // Capture count before moving
        let cert_count = data.len();

        // TODO(observability): Add metric for certificate reload operations
        tracing::info!(
            component = "tls_store",
            count = cert_count,
            "Full set of TLS certificates"
        );

        let mut tls_data = self.tls_data.write().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store write lock poisoned - recovering data from poisoned lock. \
                    A thread previously panicked while holding this lock."
            );
            // TODO(observability): Add metric for lock poison recovery
            poisoned.into_inner()
        });
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

            tls_data.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                },
            );
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

        let mut tls_data = self.tls_data.write().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store write lock poisoned during partial update - recovering. \
                    A thread previously panicked while holding this lock."
            );
            // TODO(observability): Add metric for lock poison recovery
            poisoned.into_inner()
        });

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

            tls_data.insert(
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

            tls_data.insert(
                key,
                TlsEntry {
                    tls: Arc::new(tls),
                    validation,
                },
            );
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
                host_map.entry(host.clone()).or_default().push(tls.clone());
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
    #[allow(dead_code)]
    fn rebuild_matcher(&self) -> anyhow::Result<()> {
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
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
    ///
    /// # Panics
    /// Panics if the lock is poisoned (a thread panicked while holding this lock)
    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read().expect(
            "TLS store read lock poisoned - a thread panicked while holding the lock. \
                     Data integrity cannot be guaranteed.",
        );
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
    ///
    /// # Panics
    /// Panics if the lock is poisoned
    pub fn contains(&self, key: &str) -> bool {
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
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
    ///
    /// # Panics
    /// Panics if the lock is poisoned
    pub fn get_validation_status(&self, key: &str) -> Option<CertValidationResult> {
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
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
    ///
    /// # Panics
    /// Panics if the lock is poisoned
    pub fn get_invalid_certs(&self) -> Vec<(String, CertValidationResult)> {
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
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
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
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
    ///
    /// # Panics
    /// Panics if the lock is poisoned
    pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");

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
static GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>> = LazyLock::new(|| Arc::new(TlsStore::new()));

/// Get the global TlsStore instance
pub fn get_global_tls_store() -> Arc<TlsStore> {
    GLOBAL_TLS_STORE.clone()
}
