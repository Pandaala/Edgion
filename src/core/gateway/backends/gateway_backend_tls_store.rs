//! Global store for Gateway-level backend TLS client certificates
//!
//! Maps gateway_key ("{namespace}/{name}") → Arc<Secret> for the resolved
//! `spec.tls.backend.clientCertificateRef` Secret. Used as a fallback when
//! no per-backend BackendTLSPolicy specifies a client certificate.

use arc_swap::ArcSwap;
use k8s_openapi::api::core::v1::Secret;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static GLOBAL_GATEWAY_BACKEND_TLS_STORE: LazyLock<Arc<GatewayBackendTlsStore>> =
    LazyLock::new(|| Arc::new(GatewayBackendTlsStore::new()));

pub fn get_global_gateway_backend_tls_store() -> Arc<GatewayBackendTlsStore> {
    GLOBAL_GATEWAY_BACKEND_TLS_STORE.clone()
}

pub struct GatewayBackendTlsStore {
    store: ArcSwap<HashMap<String, Arc<Secret>>>,
}

impl GatewayBackendTlsStore {
    pub fn new() -> Self {
        Self {
            store: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Get the resolved client certificate Secret for a gateway.
    pub fn get(&self, gateway_key: &str) -> Option<Arc<Secret>> {
        self.store.load().get(gateway_key).cloned()
    }

    /// Set the resolved client certificate Secret for a gateway.
    pub fn set(&self, gateway_key: String, secret: Arc<Secret>) {
        let current = self.store.load();
        let mut new_map = (**current).clone();
        new_map.insert(gateway_key, secret);
        self.store.store(Arc::new(new_map));
    }

    /// Number of gateway entries currently in the store.
    pub fn count(&self) -> usize {
        self.store.load().len()
    }

    /// Gateway keys currently in the store (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.store.load().keys().cloned().collect()
    }

    /// Remove the client certificate for a gateway.
    pub fn remove(&self, gateway_key: &str) {
        let current = self.store.load();
        if current.contains_key(gateway_key) {
            let mut new_map = (**current).clone();
            new_map.remove(gateway_key);
            self.store.store(Arc::new(new_map));
        }
    }
}

impl Default for GatewayBackendTlsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = GatewayBackendTlsStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }
}
