use super::*;

impl OpenidConnect {
    pub(super) fn http_client(&self) -> Arc<reqwest::Client> {
        self.client.clone()
    }

    pub(super) fn admit_idp_url(url: &str, operation: OidcOperation) -> Result<(), OidcVerifyError> {
        super::admit_oidc_outbound_url(url).map_err(|failure| OidcVerifyError::dependency(operation, failure))
    }

    pub(super) async fn get_or_fetch_discovery(&self) -> Result<Arc<DiscoveryDocument>, OidcDependencyError> {
        self.jwks_provider.get_or_fetch_discovery().await
    }

    /// Fetch (or serve cached) JWKS for token verification.
    ///
    /// Task 11: the provider's `get_jwks` transparently serves a stale cached
    /// set when the upstream fetch fails, and errors outright only when no
    /// cache exists at all. This is the sole consumer of `get_jwks`, so both
    /// dye records live here rather than duplicated at each call site
    /// (initial lookup + kid-miss force-refresh in `token_verify.rs`):
    /// - stale serve → the request still proceeds with a degraded (possibly
    ///   rotated-out) key set, so it's a fail-OPEN: `jwks-stale-cache`.
    /// - no cache at all → the caller cannot validate the token, so it's a
    ///   fail-CLOSE: `jwks-unreachable`. Recorded here, well before the error
    ///   propagates up through `verify_jwt_token_internal` → `verify_token` →
    ///   `run_request`'s match on `OidcVerifyError`, which is the only place
    ///   that projects it into a fixed runtime response.
    pub(super) async fn get_or_fetch_jwks(
        &self,
        session: &mut dyn PluginSession,
        force_refresh: bool,
    ) -> Result<Arc<JwkSet>, OidcDependencyError> {
        match self.jwks_provider.get_jwks(force_refresh).await {
            Ok(lookup) => {
                if lookup.served_stale {
                    session.record_outcome(DyeOutcome::FailOpen, "jwks-stale-cache");
                }
                Ok(lookup.set)
            }
            Err(err) => {
                session.record_outcome(DyeOutcome::FailClose, "jwks-unreachable");
                Err(err)
            }
        }
    }

    pub(super) async fn cached_discovery_doc(&self) -> Option<Arc<DiscoveryDocument>> {
        self.jwks_provider.cached_discovery().await
    }
}
