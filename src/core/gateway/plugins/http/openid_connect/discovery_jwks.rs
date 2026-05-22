use super::*;

impl OpenidConnect {
    pub(super) fn http_client(&self) -> &'static reqwest::Client {
        get_http_client_with_ssl_verify(self.config.ssl_verify)
    }

    pub(super) async fn get_or_fetch_discovery(&self) -> OidcResult<DiscoveryDocument> {
        self.jwks_provider
            .get_or_fetch_discovery()
            .await
            .map_err(OidcError::from)
    }

    pub(super) async fn get_or_fetch_jwks(&self, force_refresh: bool) -> OidcResult<JwkSet> {
        self.jwks_provider
            .get_jwks(force_refresh)
            .await
            .map_err(OidcError::from)
    }

    pub(super) async fn cached_discovery_doc(&self) -> Option<DiscoveryDocument> {
        self.jwks_provider.cached_discovery().await
    }
}
