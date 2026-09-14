use super::super::error::AuthResponseFailure;
use super::*;
use crate::plugins::http::common::outbound::call::stream_body_capped;

const AUTH_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

/// Token exchange, refresh, and introspection are bounded independent exchanges.
/// Never deserialize a truncated response or retain provider error text.
pub(super) async fn read_auth_response<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> VerifyResult<T> {
    if response
        .content_length()
        .is_some_and(|length| length > AUTH_RESPONSE_MAX_BYTES as u64)
    {
        return Err(OidcVerifyError::AuthResponse(AuthResponseFailure::TooLarge));
    }
    let (body, truncated) = stream_body_capped(response, AUTH_RESPONSE_MAX_BYTES)
        .await
        .map_err(|_| OidcVerifyError::AuthResponse(AuthResponseFailure::Read))?;
    if truncated {
        return Err(OidcVerifyError::AuthResponse(AuthResponseFailure::TooLarge));
    }
    serde_json::from_slice(&body).map_err(|_| OidcVerifyError::AuthResponse(AuthResponseFailure::Json))
}

/// Record an outbound TLS SAN-pinning rejection on an OIDC outbound call.
///
/// The `edgion_gateway_outbound_tls_san_mismatch_total{plugin="openid_connect"}` counter is
/// the only channel here. Every caller sits on the request hot path, which forbids `tracing::*`
/// at any level (log-safety Iron rule 3), and none of them can reach the `PluginLog`; threading a
/// typed error up to a `PluginLog`-owning frame would cost a signature cascade across the five
/// outbound call paths. A SAN mismatch is a configuration error that fails every request
/// identically, so the counter is the intended alerting signal.
pub(super) fn note_oidc_san_mismatch(err: &reqwest::Error) {
    if crate::tls::validation::outbound_san_mismatch_reason(err).is_some() {
        crate::observe::metrics::registry::record_outbound_tls_san_mismatch("openid_connect");
    }
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
        let mut persisted_payload = payload.clone();
        if persisted_payload.session_ref.is_empty() {
            persisted_payload.session_ref = Self::generate_session_ref();
        }
        if !persisted_payload.access_token.trim().is_empty() {
            self.cache_access_token(
                &persisted_payload.session_ref,
                &persisted_payload.access_token,
                persisted_payload.expires_at,
            )
            .await;
            persisted_payload.access_token.clear();
        }

        let Some(cipher) = self.session_cipher() else {
            return;
        };
        let Ok(value) = Self::encode_signed_cookie_payload(&persisted_payload, cipher) else {
            return;
        };
        let cookie = format!(
            "{}={}{}",
            self.session_cookie_name(),
            value,
            self.build_cookie_attr_suffix(session, self.config.session_lifetime, "/")
        );
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
                .ok_or(OidcVerifyError::Other {
                    status: 502,
                    message: "OIDC discovery document missing token_endpoint".to_string(),
                })?;
        let mut params: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.config.client_id.clone()),
        ];
        if let Some(verifier) = code_verifier {
            params.push(("code_verifier", verifier.to_string()));
        }

        Self::admit_idp_url(token_endpoint)?;
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
                    params.push(("client_secret", client_secret.to_string()));
                    base_request.form(&params)
                }
            }
        };

        let resp = request.send().await.map_err(|e| {
            note_oidc_san_mismatch(&e);
            OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            }
        })?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::Other {
                status: 502,
                message: format!("Token exchange request failed with status {}", resp.status()),
            });
        }
        read_auth_response(resp).await
    }

    pub(super) async fn revoke_token(
        &self,
        endpoint: &str,
        token: &str,
        token_type_hint: Option<&str>,
    ) -> OidcResult<()> {
        super::admit_oidc_outbound_url(endpoint).map_err(|error| -> OidcError { error.into() })?;
        let client = self.http_client();
        let base_request = client
            .post(endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = {
            let client_secret = self.client_secret().ok_or(CREDENTIAL_FAILURE_BODY)?;
            match self.config.token_endpoint_auth_method {
                EndpointAuthMethod::ClientSecretBasic => {
                    let mut form: Vec<(&str, String)> = vec![("token", token.to_string())];
                    if let Some(hint) = token_type_hint {
                        form.push(("token_type_hint", hint.to_string()));
                    }
                    base_request
                        .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                        .form(&form)
                }
                EndpointAuthMethod::ClientSecretPost => {
                    let mut form: Vec<(&str, String)> = vec![
                        ("token", token.to_string()),
                        ("client_id", self.config.client_id.clone()),
                        ("client_secret", client_secret.to_string()),
                    ];
                    if let Some(hint) = token_type_hint {
                        form.push(("token_type_hint", hint.to_string()));
                    }
                    base_request.form(&form)
                }
            }
        };

        let resp = request.send().await.inspect_err(note_oidc_san_mismatch)?;
        if !resp.status().is_success() {
            return Err(format!("Revocation request failed with status {}", resp.status()).into());
        }
        Ok(())
    }

    pub(super) async fn maybe_revoke_tokens_on_logout(&self, session_cookie: Option<OidcSessionCookie>) {
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

        if let Some(token) = refresh_token {
            let _ = self
                .revoke_token(revocation_endpoint, token, Some("refresh_token"))
                .await;
        }

        if let Some(token) = access_token {
            let _ = self
                .revoke_token(revocation_endpoint, token, Some("access_token"))
                .await;
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
                .ok_or(OidcVerifyError::Other {
                    status: 502,
                    message: "OIDC discovery document missing token_endpoint".to_string(),
                })?;
        let mut params: Vec<(&str, String)> = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("client_id", self.config.client_id.clone()),
        ];

        Self::admit_idp_url(token_endpoint)?;
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
                    params.push(("client_secret", client_secret.to_string()));
                    base_request.form(&params)
                }
            }
        };

        let resp = request.send().await.map_err(|e| {
            note_oidc_san_mismatch(&e);
            OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            }
        })?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::Other {
                status: 502,
                message: format!("Token refresh request failed with status {}", resp.status()),
            });
        }
        read_auth_response(resp).await
    }

    pub(super) fn should_persist_id_token(&self, discovery: &DiscoveryDocument) -> bool {
        self.config.set_id_token_header || discovery.end_session_endpoint.as_deref().is_some_and(|v| !v.is_empty())
    }

    pub(super) async fn try_refresh_session_token(
        &self,
        session_cookie: &mut OidcSessionCookie,
    ) -> VerifyResult<String> {
        if session_cookie.session_ref.is_empty() {
            session_cookie.session_ref = Self::generate_session_ref();
        }
        let refresh_token = session_cookie.refresh_token.as_deref().ok_or(OidcVerifyError::Other {
            status: 401,
            message: "Missing refresh token".to_string(),
        })?;
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|_| OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            })?;
        let token_response = self.refresh_access_token(&discovery, refresh_token).await?;

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
        Ok(session_cookie.access_token.clone())
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
    ) -> VerifyResult<String> {
        let flight = self.refresh_singleflight.get_or_create(lock_key);
        let wait_timeout = Duration::from_secs(self.config.timeout);
        let Ok(_guard) = tokio::time::timeout(wait_timeout, flight.lock().lock()).await else {
            if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
                *session_cookie = cached_payload;
                return Ok(session_cookie.access_token.clone());
            }
            return Err(OidcVerifyError::RefreshSingleflightTimeout);
        };

        if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
            *session_cookie = cached_payload;
            return Ok(session_cookie.access_token.clone());
        }

        let token = self.try_refresh_session_token(session_cookie).await?;
        self.put_refresh_result(lock_key, session_cookie.clone()).await;
        Ok(token)
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use futures::{stream, StreamExt};
    use std::io;

    fn streamed(parts: Vec<Result<Bytes, io::Error>>) -> reqwest::Response {
        http::Response::new(reqwest::Body::wrap_stream(stream::iter(parts))).into()
    }

    fn padded_json(size: usize) -> Vec<u8> {
        let mut body = br#"{"access_token":"fixture-token"}"#.to_vec();
        body.resize(size, b' ');
        body
    }

    #[tokio::test]
    async fn known_oversize_body_is_rejected() {
        let response = http::Response::new(vec![b'x'; AUTH_RESPONSE_MAX_BYTES + 1]).into();
        assert_eq!(
            read_auth_response::<Value>(response).await.unwrap_err(),
            OidcVerifyError::AuthResponse(AuthResponseFailure::TooLarge)
        );
    }

    #[tokio::test]
    async fn exact_limit_stream_preserves_token_fields() {
        let body = padded_json(AUTH_RESPONSE_MAX_BYTES);
        let parts = body.chunks(8192).map(|part| Ok(Bytes::copy_from_slice(part))).collect();
        let token: TokenEndpointResponse = read_auth_response(streamed(parts)).await.unwrap();
        assert_eq!(token.access_token, "fixture-token");
    }

    #[tokio::test]
    async fn stream_stops_at_first_oversize_chunk_without_parsing_prefix() {
        let parts = stream::iter([
            Ok::<_, io::Error>(Bytes::from(padded_json(AUTH_RESPONSE_MAX_BYTES))),
            Ok(Bytes::from_static(b" ")),
        ])
        .chain(stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
                panic!("oversized response must be dropped without polling the remainder")
            },
        ));
        let response = http::Response::new(reqwest::Body::wrap_stream(parts)).into();
        assert_eq!(
            read_auth_response::<TokenEndpointResponse>(response).await.unwrap_err(),
            OidcVerifyError::AuthResponse(AuthResponseFailure::TooLarge)
        );
    }

    #[tokio::test]
    async fn body_read_failure_is_static_and_rejects_valid_prefix() {
        let response = streamed(vec![
            Ok(Bytes::from_static(br#"{"access_token":"fixture-token"}"#)),
            Err(io::Error::other("private-provider-detail")),
        ]);
        let error = read_auth_response::<TokenEndpointResponse>(response).await.unwrap_err();
        assert_eq!(error, OidcVerifyError::AuthResponse(AuthResponseFailure::Read));
        assert_eq!(error.user_message(), "Bad Gateway");
        assert_eq!(error.log_tag(), "auth response read fail");
    }

    #[tokio::test]
    async fn malformed_or_wrong_shape_json_has_no_provider_detail() {
        for body in [
            b"{private-provider-detail".as_slice(),
            br#"{"access_token":{"private":"credential"}}"#,
        ] {
            let error = read_auth_response::<TokenEndpointResponse>(http::Response::new(body.to_vec()).into())
                .await
                .unwrap_err();
            assert_eq!(error, OidcVerifyError::AuthResponse(AuthResponseFailure::Json));
            assert_eq!(error.user_message(), "Bad Gateway");
            assert_eq!(error.log_tag(), "auth response json fail");
        }
    }

    #[test]
    fn response_errors_keep_wire_and_contextual_log_contracts() {
        for error in [
            OidcVerifyError::AuthResponse(AuthResponseFailure::TooLarge),
            OidcVerifyError::AuthResponse(AuthResponseFailure::Read),
            OidcVerifyError::AuthResponse(AuthResponseFailure::Json),
        ] {
            assert_eq!(error.http_status(), 502);
            assert_eq!(error.user_message(), "Bad Gateway");
            assert_eq!(error.response_log_tag_or("token refresh fail"), error.log_tag());
        }
        assert_eq!(
            OidcVerifyError::Expired.response_log_tag_or("token refresh fail"),
            "token refresh fail"
        );
    }
}
