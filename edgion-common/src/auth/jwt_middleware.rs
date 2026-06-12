use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

use super::config::AdminAuthConfig;
use super::jwks_provider::{no_op_validator, JwksProvider, JwksProviderConfig, LeanDiscoveryDocument};

/// Admin JWT auth state shared with `unified_auth`.
pub struct AuthMiddlewareState {
    pub jwks_provider: Arc<JwksProvider<LeanDiscoveryDocument>>,
    pub audiences: Vec<String>,
    pub issuers: Vec<String>,
    pub allowed_algorithms: Vec<String>,
    pub clock_skew_seconds: u64,
}

impl AuthMiddlewareState {
    /// Build state from config. Returns Err on HTTP-client construction failure
    /// (e.g. TLS backend init issues) instead of `process::exit` (closes g3-012).
    pub fn from_config(config: &AdminAuthConfig) -> Result<Arc<Self>, String> {
        let client = Client::builder()
            .pool_max_idle_per_host(4)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(!config.ssl_verify)
            .build()
            .map_err(|e| format!("Failed to build JWKS HTTP client: {}", e))?;

        let provider_config = JwksProviderConfig {
            discovery_url: config.discovery.clone(),
            jwks_cache_ttl: Duration::from_secs(config.jwks_cache_ttl),
            jwks_min_refresh_interval: Duration::from_secs(config.jwks_min_refresh_interval),
            discovery_max_response_bytes: config.discovery_max_response_bytes,
            jwks_max_response_bytes: config.jwks_max_response_bytes,
            request_timeout: Duration::from_secs(10),
            component_label: "admin_auth",
        };
        let jwks_provider = JwksProvider::new(provider_config, Arc::new(client), no_op_validator());

        Ok(Arc::new(Self {
            jwks_provider,
            audiences: config.audiences.clone(),
            issuers: config.issuers.clone(),
            allowed_algorithms: config.allowed_algorithms.clone(),
            clock_skew_seconds: config.clock_skew_seconds,
        }))
    }
}
