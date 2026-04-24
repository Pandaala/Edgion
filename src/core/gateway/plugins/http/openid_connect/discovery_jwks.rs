use super::*;
use crate::core::common::http_util::fetch_json_limited;

impl OpenidConnect {
    pub(super) fn http_client(&self) -> &'static reqwest::Client {
        get_http_client_with_ssl_verify(self.config.ssl_verify)
    }

    pub(super) fn min_refresh_duration(&self) -> Duration {
        Duration::from_secs(self.config.jwks_min_refresh_interval)
    }

    pub(super) fn jwks_ttl_duration(&self) -> Duration {
        Duration::from_secs(self.config.jwks_cache_ttl)
    }

    pub(super) fn can_refresh(last_refresh_at: Option<Instant>, now: Instant, min_refresh: Duration) -> bool {
        match last_refresh_at {
            Some(last) => now.saturating_duration_since(last) >= min_refresh,
            None => true,
        }
    }

    pub(super) async fn get_or_fetch_discovery(&self) -> OidcResult<DiscoveryDocument> {
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }

        let _lock = self.discovery_refresh.lock().await;
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }

        let client = self.http_client();
        let resp = client
            .get(&self.config.discovery)
            .timeout(Duration::from_secs(self.config.timeout))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("OIDC discovery request failed with status {}", resp.status()).into());
        }

        let doc: DiscoveryDocument =
            fetch_json_limited(resp, self.config.discovery_max_response_bytes, "OIDC discovery")
                .await
                .map_err(OidcError::from)?;
        if doc.issuer.is_empty() {
            return Err("OIDC discovery document missing issuer".into());
        }
        if doc.jwks_uri.is_empty() {
            return Err("OIDC discovery document missing jwks_uri".into());
        }

        *self.discovery_doc.write().await = Some(doc.clone());
        Ok(doc)
    }

    pub(super) async fn get_or_fetch_jwks(&self, force_refresh: bool) -> OidcResult<JwkSet> {
        let now = Instant::now();
        let min_refresh = self.min_refresh_duration();
        let ttl = self.jwks_ttl_duration();

        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(set.clone());
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(set.clone());
                }
            }
        }

        let _guard = self.jwks_refresh.lock().await;
        let now = Instant::now();

        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(set.clone());
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(set.clone());
                }
            } else if !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                return Err("JWKS refresh blocked by minimum refresh interval".into());
            }
        }

        let discovery = self.get_or_fetch_discovery().await?;
        let fetch_result: OidcResult<JwkSet> = async {
            let client = self.http_client();
            let resp = client
                .get(&discovery.jwks_uri)
                .timeout(Duration::from_secs(self.config.timeout))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(format!("JWKS request failed with status {}", resp.status()).into());
            }
            let jwks: JwkSet = fetch_json_limited(resp, self.config.jwks_max_response_bytes, "JWKS")
                .await
                .map_err(OidcError::from)?;
            if jwks.keys.is_empty() {
                return Err("JWKS endpoint returned empty keys".into());
            }
            Ok(jwks)
        }
        .await;

        let mut state = self.jwks_state.write().await;
        state.last_refresh_at = Some(now);

        match fetch_result {
            Ok(jwks) => {
                state.set = Some(jwks.clone());
                state.expires_at = Some(now + ttl);
                Ok(jwks)
            }
            Err(err) => {
                if let Some(ref cached) = state.set {
                    // Stale-cache fallback: JWKS endpoint unreachable but a prior set exists.
                    // Per-request logging is handled via plugin_log at the call site.
                    Ok(cached.clone())
                } else {
                    Err(err)
                }
            }
        }
    }

    pub(super) async fn cached_discovery_doc(&self) -> Option<DiscoveryDocument> {
        let cached = self.discovery_doc.read().await;
        cached.clone()
    }
}
