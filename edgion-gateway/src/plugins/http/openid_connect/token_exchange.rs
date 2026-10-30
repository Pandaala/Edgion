use super::super::error::read_oidc_json;
use super::*;
const AUTH_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

pub(super) async fn read_auth_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    operation: OidcOperation,
) -> VerifyResult<T> {
    read_oidc_json(response, AUTH_RESPONSE_MAX_BYTES)
        .await
        .map_err(|failure| OidcVerifyError::dependency(operation, failure))
}

impl OpenidConnect {
    pub(super) fn refresh_lock_key(raw_cookie_value: &str) -> String {
        let digest = Sha256::digest(raw_cookie_value.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }

    pub(super) async fn cache_access_token(&self, session_ref: &str, token: &str, expires_at: Option<u64>) {
        if session_ref.is_empty() || token.is_empty() {
            return;
        }
        let clock_skew = self.config.clock_skew_seconds;
        // TTL = token exp - now - clock_skew, capped at 1 hour, fallback 5 min
        let ttl = expires_at
            .and_then(|exp| exp.checked_sub(Self::now_unix_secs()))
            .and_then(|secs| secs.checked_sub(clock_skew))
            .map(|secs| Duration::from_secs(secs).min(Duration::from_secs(3600)))
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(300));
        self.access_token_cache.insert(
            session_ref.to_string(),
            AccessTokenCacheEntry {
                token: token.to_string(),
            },
            Some(ttl),
        );
    }

    pub(super) async fn get_cached_access_token(&self, session_ref: &str) -> Option<String> {
        if session_ref.is_empty() {
            return None;
        }
        // Expiry (including clock_skew) was baked into TTL at insert time — no manual check needed.
        self.access_token_cache.get(session_ref).map(|entry| entry.token)
    }

    pub(super) async fn remove_cached_access_token(&self, session_ref: &str) {
        if session_ref.is_empty() {
            return;
        }
        self.access_token_cache.remove(session_ref);
    }

    pub(super) async fn resolve_access_token_from_session(&self, session_cookie: &OidcSessionCookie) -> Option<String> {
        if !session_cookie.access_token.trim().is_empty() {
            return Some(session_cookie.access_token.clone());
        }
        self.get_cached_access_token(&session_cookie.session_ref).await
    }

    pub(super) fn should_preemptive_refresh(&self, resolved_access_token: &str, payload: &OidcSessionCookie) -> bool {
        if !self.config.renew_access_token_on_expiry {
            return false;
        }
        if payload.refresh_token.as_deref().is_none() {
            return false;
        }
        if resolved_access_token.trim().is_empty() {
            return true;
        }
        let Some(exp) = payload.expires_at else {
            return false;
        };
        let now = Self::now_unix_secs();
        now.saturating_add(self.config.access_token_expires_leeway) >= exp
    }

    pub(super) async fn maybe_set_session_cookie_header(
        &self,
        session: &mut dyn PluginSession,
        payload: &OidcSessionCookie,
    ) {
        if payload.remaining_lifetime(self.session_now()).is_err() {
            return;
        }
        let cache_token = !payload.access_token.trim().is_empty();
        let generated_ref = payload.session_ref.is_empty().then(Self::generate_session_ref);
        let session_ref = generated_ref.as_deref().unwrap_or(&payload.session_ref);
        let persisted_payload = payload.for_persistence(session_ref, cache_token);
        if cache_token {
            self.cache_access_token(
                persisted_payload.session_ref,
                &payload.access_token,
                persisted_payload.expires_at,
            )
            .await;
        }

        let Some(cipher) = self.session_cipher() else {
            return;
        };
        let Ok(value) = Self::encode_signed_cookie_payload(&persisted_payload, cipher) else {
            return;
        };
        let Ok(remaining_lifetime) = payload.remaining_lifetime(self.session_now()) else {
            return;
        };
        let cookie = self.build_cookie_header_value(self.session_cookie_name(), &value, remaining_lifetime, "/");
        if self.ensure_session_cookie_size_limit(&cookie).is_err() {
            return;
        }
        // Append, not set: Set-Cookie is a multi-value header; insert_header would
        // wipe every upstream/other-plugin Set-Cookie. append_header preserves them
        // (RFC 6265 §3 / RFC 7230 §3.2.2 list-syntax exception).
        let _ = session.append_response_header("Set-Cookie", &cookie);
    }

    pub(super) async fn exchange_code_for_token(
        &self,
        discovery: &DiscoveryDocument,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint =
            discovery
                .token_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(OidcVerifyError::dependency(
                    OidcOperation::TokenExchange,
                    OidcDependencyFailure::MissingEndpoint,
                ))?;
        let mut params: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", self.config.client_id.as_str()),
        ];
        if let Some(verifier) = code_verifier {
            params.push(("code_verifier", verifier));
        }

        Self::admit_idp_url(token_endpoint, OidcOperation::TokenExchange)?;
        let client = self.http_client();
        let base_request = client
            .post(token_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = {
            let client_secret = self.client_secret().ok_or_else(Self::credential_failure)?;
            match self.config.token_endpoint_auth_method {
                EndpointAuthMethod::ClientSecretBasic => base_request
                    .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                    .form(&params),
                EndpointAuthMethod::ClientSecretPost => {
                    params.push(("client_secret", client_secret));
                    base_request.form(&params)
                }
            }
        };

        drop(params);

        let resp = request
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(OidcOperation::TokenExchange, &error, "openid_connect"))?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::dependency(
                OidcOperation::TokenExchange,
                OidcDependencyFailure::HttpStatus,
            ));
        }
        read_auth_response(resp, OidcOperation::TokenExchange).await
    }

    pub(super) async fn revoke_token(
        &self,
        endpoint: &str,
        token: &str,
        token_type_hint: Option<&str>,
    ) -> VerifyResult<()> {
        Self::admit_idp_url(endpoint, OidcOperation::Revocation)?;
        let client = self.http_client();
        let base_request = client
            .post(endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = {
            let client_secret = self.client_secret().ok_or_else(Self::credential_failure)?;
            match self.config.token_endpoint_auth_method {
                EndpointAuthMethod::ClientSecretBasic => {
                    let mut form: Vec<(&str, &str)> = vec![("token", token)];
                    if let Some(hint) = token_type_hint {
                        form.push(("token_type_hint", hint));
                    }
                    base_request
                        .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                        .form(&form)
                }
                EndpointAuthMethod::ClientSecretPost => {
                    let mut form: Vec<(&str, &str)> = vec![
                        ("token", token),
                        ("client_id", self.config.client_id.as_str()),
                        ("client_secret", client_secret),
                    ];
                    if let Some(hint) = token_type_hint {
                        form.push(("token_type_hint", hint));
                    }
                    base_request.form(&form)
                }
            }
        };

        let resp = request
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(OidcOperation::Revocation, &error, "openid_connect"))?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::dependency(
                OidcOperation::Revocation,
                OidcDependencyFailure::HttpStatus,
            ));
        }
        Ok(())
    }

    pub(super) async fn maybe_revoke_tokens_on_logout(
        &self,
        session_cookie: Option<&OidcSessionCookie>,
        plugin_log: &mut PluginLog,
    ) {
        if !self.config.revoke_tokens_on_logout {
            return;
        }

        let Some(session_cookie) = session_cookie else {
            return;
        };

        let refresh_token = session_cookie.refresh_token.as_deref().filter(|v| !v.trim().is_empty());
        let cached_access_token = self.get_cached_access_token(&session_cookie.session_ref).await;
        let access_token = if session_cookie.access_token.trim().is_empty() {
            cached_access_token.as_deref()
        } else {
            Some(session_cookie.access_token.as_str())
        };
        if refresh_token.is_none() && access_token.is_none() {
            return;
        }

        let Some(discovery) = self.cached_discovery_doc().await else {
            return;
        };
        let Some(revocation_endpoint) = discovery.revocation_endpoint.as_deref().filter(|v| !v.is_empty()) else {
            return;
        };

        let mut failure = None;
        if let Some(token) = refresh_token {
            failure = self
                .revoke_token(revocation_endpoint, token, Some("refresh_token"))
                .await
                .err();
        }
        if let Some(token) = access_token {
            let access_failure = self
                .revoke_token(revocation_endpoint, token, Some("access_token"))
                .await
                .err();
            failure = failure.or(access_failure);
        }
        if let Some(error) = failure {
            plugin_log.push_err(error.response_log_tag_or("revocation credential unavailable"));
        }
    }

    pub(super) async fn refresh_access_token(
        &self,
        discovery: &DiscoveryDocument,
        refresh_token: &str,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint =
            discovery
                .token_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(OidcVerifyError::dependency(
                    OidcOperation::Refresh,
                    OidcDependencyFailure::MissingEndpoint,
                ))?;
        let mut params: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.config.client_id.as_str()),
        ];

        Self::admit_idp_url(token_endpoint, OidcOperation::Refresh)?;
        let client = self.http_client();
        let base_request = client
            .post(token_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = {
            let client_secret = self.client_secret().ok_or_else(Self::credential_failure)?;
            match self.config.token_endpoint_auth_method {
                EndpointAuthMethod::ClientSecretBasic => base_request
                    .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                    .form(&params),
                EndpointAuthMethod::ClientSecretPost => {
                    params.push(("client_secret", client_secret));
                    base_request.form(&params)
                }
            }
        };

        drop(params);

        let resp = request
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(OidcOperation::Refresh, &error, "openid_connect"))?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::dependency(
                OidcOperation::Refresh,
                OidcDependencyFailure::HttpStatus,
            ));
        }
        read_auth_response(resp, OidcOperation::Refresh).await
    }

    pub(super) fn should_persist_id_token(&self, discovery: &DiscoveryDocument) -> bool {
        self.config.set_id_token_header || discovery.end_session_endpoint.as_deref().is_some_and(|v| !v.is_empty())
    }

    pub(super) async fn try_refresh_session_token(&self, session_cookie: &mut OidcSessionCookie) -> VerifyResult<()> {
        session_cookie.remaining_lifetime(self.session_now())?;
        if session_cookie.session_ref.is_empty() {
            session_cookie.session_ref = Self::generate_session_ref();
        }
        let refresh_token = session_cookie
            .refresh_token
            .as_deref()
            .ok_or(OidcVerifyError::Auth(AuthFailure::MissingRefreshToken))?;
        let discovery = self.get_or_fetch_discovery().await.map_err(OidcVerifyError::from)?;
        session_cookie.remaining_lifetime(self.session_now())?;
        let token_response = self.refresh_access_token(&discovery, refresh_token).await?;
        session_cookie.remaining_lifetime(self.session_now())?;

        let now = Self::now_unix_secs();
        let should_persist_id_token = self.should_persist_id_token(&discovery);
        session_cookie.access_token = token_response.access_token;
        session_cookie.expires_at = token_response.expires_in.map(|ttl| now.saturating_add(ttl));
        if should_persist_id_token {
            let new_id_token = token_response.id_token.filter(|v| !v.is_empty());
            if let Some(new_id_token) = new_id_token {
                session_cookie.id_token = Some(new_id_token);
            }
        }
        if token_response.refresh_token.is_some() {
            session_cookie.refresh_token = token_response.refresh_token;
        }
        Ok(())
    }

    pub(super) async fn try_get_recent_refresh_result(&self, lock_key: &str) -> Option<OidcSessionCookie> {
        self.refresh_singleflight_results.get(lock_key)
    }

    pub(super) async fn put_refresh_result(&self, lock_key: &str, payload: OidcSessionCookie) {
        self.refresh_singleflight_results
            .insert(lock_key.to_string(), payload, Some(Duration::from_secs(5)));
    }

    pub(super) async fn try_refresh_session_token_singleflight(
        &self,
        lock_key: &str,
        session_cookie: &mut OidcSessionCookie,
    ) -> VerifyResult<()> {
        session_cookie.remaining_lifetime(self.session_now())?;
        let flight = self.refresh_singleflight.get_or_create(lock_key);
        let wait_timeout = Duration::from_secs(self.config.timeout);
        let Ok(_guard) = tokio::time::timeout(wait_timeout, flight.lock().lock()).await else {
            if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
                cached_payload.remaining_lifetime(self.session_now())?;
                *session_cookie = cached_payload;
                return Ok(());
            }
            return Err(OidcVerifyError::RefreshSingleflightTimeout);
        };

        session_cookie.remaining_lifetime(self.session_now())?;
        if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
            cached_payload.remaining_lifetime(self.session_now())?;
            *session_cookie = cached_payload;
            return Ok(());
        }

        self.try_refresh_session_token(session_cookie).await?;
        self.put_refresh_result(lock_key, session_cookie.clone()).await;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/openid_connect/token_exchange_test/response_tests.rs"]
mod response_tests;
