//! Global store for Gateway-level backend TLS client certificates
//!
//! Maps gateway_key ("{namespace}/{name}") → Arc<Secret> for the resolved
//! `spec.tls.backend.clientCertificateRef` Secret. Used as a client-identity
//! fallback only after a BackendTLSPolicy has selected TLS and has not supplied
//! its own client certificate.

use arc_swap::ArcSwap;
use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
use edgion_resources::{Gateway, ResourceMeta};
use k8s_openapi::api::core::v1::Secret;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static GLOBAL_GATEWAY_BACKEND_TLS_STORE: LazyLock<Arc<GatewayBackendTlsStore>> =
    LazyLock::new(|| Arc::new(GatewayBackendTlsStore::new()));

pub fn get_global_gateway_backend_tls_store() -> Arc<GatewayBackendTlsStore> {
    GLOBAL_GATEWAY_BACKEND_TLS_STORE.clone()
}

pub(crate) struct GatewayBackendTlsEntry {
    raw: Arc<Secret>,
    #[cfg(feature = "boringssl")]
    pub(in crate::backends) prepared: Arc<super::prepared_backend_tls::PreparedGatewayIdentity>,
}

pub struct GatewayBackendTlsStore {
    store: ArcSwap<HashMap<String, Arc<GatewayBackendTlsEntry>>>,
}

impl GatewayBackendTlsStore {
    pub fn new() -> Self {
        Self {
            store: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Get the resolved client certificate Secret for a gateway.
    pub fn get(&self, gateway_key: &str) -> Option<Arc<Secret>> {
        self.store.load().get(gateway_key).map(|entry| Arc::clone(&entry.raw))
    }

    #[cfg(feature = "boringssl")]
    pub(crate) fn get_prepared(&self, gateway_key: &str) -> Option<Arc<GatewayBackendTlsEntry>> {
        self.store.load().get(gateway_key).cloned()
    }

    /// Publish only material paired with the actual Gateway backend client field.
    /// The transport slot preserves raw key semantics and is never reference authority.
    pub fn update_from_gateway(&self, gateway_key: &str, gateway: &Gateway) {
        if gateway_key != gateway.key_name() {
            self.remove(gateway_key);
            return;
        }
        let references = GatewayTlsRefs::from_gateway(gateway);
        let secret = references.backend_client().and_then(|field| {
            field
                .backend_client_material()
                .ok()
                .and_then(|pair| pair.material().cloned())
        });
        let Some(secret) = secret else {
            self.remove(gateway_key);
            return;
        };
        let current = self.store.load();
        let mut new_map = (**current).clone();
        #[cfg(feature = "boringssl")]
        let prepared = current
            .get(gateway_key)
            .filter(|entry| entry.raw.data == secret.data)
            .map(|entry| Arc::clone(&entry.prepared))
            .unwrap_or_else(|| Arc::new(super::prepared_backend_tls::PreparedGatewayIdentity::new(&secret)));
        new_map.insert(
            gateway_key.to_string(),
            Arc::new(GatewayBackendTlsEntry {
                raw: Arc::new(secret),
                #[cfg(feature = "boringssl")]
                prepared,
            }),
        );
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
#[path = "../../tests/unit/backends/gateway_backend_tls_store_test/stats_tests.rs"]
mod stats_tests;
