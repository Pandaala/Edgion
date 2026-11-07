use super::super::error::read_oidc_json;
use super::*;
const AUTH_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
/// Access-token lifetime assumed when the token response omits `expires_in`.
const DEFAULT_ACCESS_TOKEN_TTL_SECS: u64 = 300;

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

    /// Refresh horizon for a freshly issued access token.
    ///
    /// `expires_in` falls back to 300 s when the IdP omits it or reports 0. The early-refresh margin is
    /// the larger of `clockSkewSeconds` and `accessTokenExpiresLeeway`, clamped to half the
    /// lifetime, so no configuration refreshes a fresh token on every request.
    pub(super) fn refresh_horizon(&self, now: u64, expires_in: Option<u64>) -> u64 {
        let ttl = expires_in
            .filter(|ttl| *ttl > 0)
            .unwrap_or(DEFAULT_ACCESS_TOKEN_TTL_SECS);
        let margin = self
            .config
            .clock_skew_seconds
            .max(self.config.access_token_expires_leeway)
            .min(ttl / 2);
        now.saturating_add(ttl - margin)
    }

    pub(super) fn should_preemptive_refresh(&self, payload: &OidcSessionCookie) -> bool {
        if !self.config.renew_access_token_on_expiry {
            return false;
        }
        if payload.refresh_token.as_deref().is_none() {
            return false;
        }
        if payload.access_token.trim().is_empty() {
            return true;
        }
        // `expires_at` is the refresh horizon; the leeway is already folded into it.
        payload
            .expires_at
            .is_some_and(|horizon| Self::now_unix_secs() >= horizon)
    }

    pub(super) async fn maybe_set_session_cookie_header(
        &self,
        session: &mut dyn PluginSession,
        payload: &OidcSessionCookie,
        prior_chunks: usize,
        plugin_log: &mut PluginLog,
    ) {
        let Ok(remaining_lifetime) = payload.remaining_lifetime(self.session_now()) else {
            return;
        };
        let Some(cipher) = self.session_cipher() else {
            return;
        };
        let Ok(value) = Self::encode_signed_cookie_payload(payload, cipher) else {
            return;
        };
        let cookies = match self.build_session_set_cookies(&value, remaining_lifetime, prior_chunks) {
            Ok(cookies) => cookies,
            Err(()) => {
                // Keeping the old cookie would replay a possibly rotated refresh token, so the
                // session ends instead; the current request still uses the fresh token.
                plugin_log.push_err("session cookie too large");
                self.expired_session_cookies()
            }
        };
        // Append, not set: Set-Cookie is a multi-value header; insert_header would
        // wipe every upstream/other-plugin Set-Cookie. append_header preserves them
        // (RFC 6265 §3 / RFC 7230 §3.2.2 list-syntax exception).
        for cookie in &cookies {
            let _ = session.append_response_header("Set-Cookie", cookie);
        }
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
        let access_token = Some(session_cookie.access_token.as_str()).filter(|v| !v.trim().is_empty());
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
        let refresh_token = session_cookie
            .refresh_token
            .as_deref()
            .ok_or(OidcVerifyError::Auth(AuthFailure::MissingRefreshToken))?;
        let discovery = self.get_or_fetch_discovery().await.map_err(OidcVerifyError::from)?;
        session_cookie.remaining_lifetime(self.session_now())?;
        let token_response = self.refresh_access_token(&discovery, refresh_token).await?;
        session_cookie.remaining_lifetime(self.session_now())?;
        let should_persist_id_token = self.should_persist_id_token(&discovery);
        self.apply_refresh_response(
            session_cookie,
            token_response,
            should_persist_id_token,
            Self::now_unix_secs(),
        )
    }

    /// Merge a token-endpoint refresh response into the session payload.
    pub(super) fn apply_refresh_response(
        &self,
        session_cookie: &mut OidcSessionCookie,
        token_response: TokenEndpointResponse,
        should_persist_id_token: bool,
        now: u64,
    ) -> VerifyResult<()> {
        if token_response.access_token.trim().is_empty() {
            // Never persist a token-less session: it would refresh on every request.
            return Err(OidcVerifyError::dependency(
                OidcOperation::Refresh,
                OidcDependencyFailure::InvalidDocument,
            ));
        }
        session_cookie.access_token = token_response.access_token;
        session_cookie.expires_at = Some(self.refresh_horizon(now, token_response.expires_in));
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
