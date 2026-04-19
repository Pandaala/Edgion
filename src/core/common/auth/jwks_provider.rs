//! JWKS (JSON Web Key Set) provider with background refresh.
//!
//! Fetches the OIDC discovery document to locate the `jwks_uri`, then periodically
//! refreshes the JWKS. Provides `DecodingKey` construction for JWT verification.

use jsonwebtoken::jwk::JwkSet;
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use super::config::AdminAuthConfig;
use crate::core::common::http_util::fetch_json_limited;

/// Lightweight discovery document — we only need issuer + jwks_uri.
#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

/// Cached JWKS state.
#[derive(Debug)]
struct JwksState {
    set: Option<JwkSet>,
    expires_at: Option<Instant>,
    last_refresh_at: Option<Instant>,
}

/// JWKS provider that fetches and caches JWKS from an OIDC discovery endpoint.
pub struct JwksProvider {
    config: AdminAuthConfig,
    client: Client,
    discovery_doc: RwLock<Option<DiscoveryDocument>>,
    discovery_refresh: Mutex<()>,
    jwks_state: RwLock<JwksState>,
    jwks_refresh: Mutex<()>,
}

impl JwksProvider {
    /// Create a new JWKS provider from admin auth config.
    pub fn new(config: &AdminAuthConfig) -> Arc<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(4)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(!config.ssl_verify)
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(component = "admin_auth", error = %e, "Failed to build HTTP client for JWKS");
                std::process::exit(1);
            });

        Arc::new(Self {
            config: config.clone(),
            client,
            discovery_doc: RwLock::new(None),
            discovery_refresh: Mutex::new(()),
            jwks_state: RwLock::new(JwksState {
                set: None,
                expires_at: None,
                last_refresh_at: None,
            }),
            jwks_refresh: Mutex::new(()),
        })
    }

    /// Get the cached issuer from the discovery document.
    pub async fn issuer(&self) -> Option<String> {
        self.discovery_doc.read().await.as_ref().map(|d| d.issuer.clone())
    }

    /// Get the cached JWKS. If expired or missing, refreshes first.
    pub async fn get_jwks(&self) -> Result<JwkSet, String> {
        // Fast path: check if cached JWKS is still valid.
        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if state.expires_at.map_or(false, |exp| Instant::now() < exp) {
                    return Ok(set.clone());
                }
            }
        }

        // Slow path: refresh with singleflight lock.
        let _guard = self.jwks_refresh.lock().await;

        // Double-check after acquiring lock.
        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if state.expires_at.map_or(false, |exp| Instant::now() < exp) {
                    return Ok(set.clone());
                }
            }
        }

        // Rate-limit refreshes.
        {
            let state = self.jwks_state.read().await;
            if let Some(last) = state.last_refresh_at {
                let min_interval = Duration::from_secs(self.config.jwks_min_refresh_interval);
                if last.elapsed() < min_interval {
                    if let Some(ref set) = state.set {
                        return Ok(set.clone());
                    }
                }
            }
        }

        // Ensure discovery document is available.
        self.ensure_discovery().await?;

        let jwks_uri = {
            let doc = self.discovery_doc.read().await;
            doc.as_ref().ok_or("Discovery document not available")?.jwks_uri.clone()
        };

        // Fetch JWKS.
        let resp = self
            .client
            .get(&jwks_uri)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch JWKS from {}: {}", jwks_uri, e))?;

        if !resp.status().is_success() {
            return Err(format!("JWKS endpoint returned status {}", resp.status()));
        }

        let jwks: JwkSet = fetch_json_limited(resp, self.config.jwks_max_response_bytes, "JWKS").await?;

        tracing::info!(
            component = "admin_auth",
            num_keys = jwks.keys.len(),
            "JWKS refreshed successfully"
        );

        // Update cache.
        {
            let mut state = self.jwks_state.write().await;
            state.set = Some(jwks.clone());
            state.expires_at = Some(Instant::now() + Duration::from_secs(self.config.jwks_cache_ttl));
            state.last_refresh_at = Some(Instant::now());
        }

        Ok(jwks)
    }

    /// Ensure the discovery document is fetched.
    async fn ensure_discovery(&self) -> Result<(), String> {
        {
            let doc = self.discovery_doc.read().await;
            if doc.is_some() {
                return Ok(());
            }
        }

        let _guard = self.discovery_refresh.lock().await;

        // Double-check.
        {
            let doc = self.discovery_doc.read().await;
            if doc.is_some() {
                return Ok(());
            }
        }

        let resp = self
            .client
            .get(&self.config.discovery)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch discovery document: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Discovery endpoint returned status {}", resp.status()));
        }

        let doc: DiscoveryDocument =
            fetch_json_limited(resp, self.config.discovery_max_response_bytes, "OIDC discovery").await?;

        if doc.issuer.is_empty() {
            return Err("Discovery document missing issuer".to_string());
        }
        if doc.jwks_uri.is_empty() {
            return Err("Discovery document missing jwks_uri".to_string());
        }

        tracing::info!(
            component = "admin_auth",
            issuer = %doc.issuer,
            jwks_uri = %doc.jwks_uri,
            "OIDC discovery document loaded"
        );

        *self.discovery_doc.write().await = Some(doc);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_document_deserialize() {
        let json = r#"{
            "issuer": "https://idp.example.com",
            "jwks_uri": "https://idp.example.com/certs",
            "authorization_endpoint": "https://idp.example.com/auth"
        }"#;
        let doc: DiscoveryDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.issuer, "https://idp.example.com");
        assert_eq!(doc.jwks_uri, "https://idp.example.com/certs");
    }
}
