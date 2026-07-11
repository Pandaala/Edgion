use super::*;

impl OpenidConnect {
    pub(super) fn http_client(&self) -> Arc<reqwest::Client> {
        self.client.clone()
    }

    pub(super) async fn get_or_fetch_discovery(&self) -> Result<DiscoveryDocument, JwksProviderError> {
        self.jwks_provider.get_or_fetch_discovery().await
    }

    pub(super) async fn get_or_fetch_jwks(&self, force_refresh: bool) -> Result<JwkSet, JwksProviderError> {
        self.jwks_provider.get_jwks(force_refresh).await
    }

    pub(super) async fn cached_discovery_doc(&self) -> Option<DiscoveryDocument> {
        self.jwks_provider.cached_discovery().await
    }
}
