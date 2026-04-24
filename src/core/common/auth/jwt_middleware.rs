use std::collections::HashSet;
use std::sync::Arc;

use super::jwks_provider::JwksProvider;

/// OIDC JWT auth state shared with `unified_auth`.
pub struct AuthMiddlewareState {
    pub jwks_provider: Arc<JwksProvider>,
    pub audiences: Vec<String>,
    pub issuers: Vec<String>,
    pub allowed_algorithms: Vec<String>,
    pub clock_skew_seconds: u64,
    pub skip_paths: HashSet<String>,
}

impl AuthMiddlewareState {
    pub fn from_config(config: &super::config::AdminAuthConfig) -> Arc<Self> {
        let jwks_provider = JwksProvider::new(config);
        Arc::new(Self {
            jwks_provider,
            audiences: config.audiences.clone(),
            issuers: config.issuers.clone(),
            allowed_algorithms: config.allowed_algorithms.clone(),
            clock_skew_seconds: config.clock_skew_seconds,
            skip_paths: config.skip_paths_set(),
        })
    }
}
