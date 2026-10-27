use super::*;
use crate::plugins::http::common::auth_common::apply_auth_failure_delay as auth_delay;
use crate::plugins::runtime::PluginSessionHeaderExt;

impl OpenidConnect {
    pub(super) fn extract_bearer_token(&self, session: &dyn PluginSession) -> Option<String> {
        let header = session.header_str("authorization")?.trim();
        if header.len() < 8 {
            return None;
        }
        let (scheme, token) = header.split_at(7);
        if !scheme.eq_ignore_ascii_case("bearer ") {
            return None;
        }
        let token = token.trim();
        if token.is_empty() {
            None
        } else {
            Some(token.to_string())
        }
    }

    pub(super) async fn send_unauthorized(&self, session: &mut dyn PluginSession, body: &str) -> OidcResult<()> {
        // 401 covers both absent and invalid credentials here; omit the RFC 6750
        // error code (SHOULD NOT when no credentials were presented).
        send_gateway_auth_error_response(session, 401, &self.auth_challenge, None, body).await
    }

    pub(super) async fn send_forbidden(&self, session: &mut dyn PluginSession, body: &str) -> OidcResult<()> {
        // 403 is reached only on authorization failure (claim/audience/scope check):
        // a valid token lacking sufficient privilege -> RFC 6750 "insufficient_scope".
        send_gateway_auth_error_response(session, 403, &self.auth_challenge, Some("insufficient_scope"), body).await
    }

    /// Apply failure delay before returning a 401/403 response.
    pub(super) async fn apply_auth_failure_delay(&self) {
        auth_delay(self.config.auth_failure_delay_ms).await;
    }

    /// Remove the client Authorization header from the upstream request when
    /// `hideCredentials` is enabled.
    ///
    /// This switch covers the bearer credential only. Gateway-owned session and
    /// state cookies are always stripped before proxying, including session-cookie
    /// mode.
    pub(super) fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.config.hide_credentials {
            let _ = session.remove_request_header("authorization");
        }
    }

    pub(super) async fn send_plain_error(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: &'static str,
    ) -> OidcResult<()> {
        let mut resp = ResponseHeader::build(status, None)?;
        resp.insert_header("Content-Type", "text/plain")?;
        session.write_response_header(Box::new(resp), false).await?;
        session
            .write_response_body(Some(Bytes::from(format!("{} {}", status, body))), true)
            .await?;
        session.shutdown().await;
        Ok(())
    }

    /// Fixed dependency replies belong to the runtime; dynamic Bearer challenges
    /// are written here. Callback authentication errors retain their 401 projection.
    pub(super) async fn send_oidc_error(
        &self,
        session: &mut dyn PluginSession,
        err: &OidcVerifyError,
    ) -> PluginRunningResult {
        if err.http_status() == 502 {
            Self::dependency_response()
        } else {
            self.apply_auth_failure_delay().await;
            let _ = self.send_unauthorized(session, err.user_message()).await;
            PluginRunningResult::ErrTerminateRequest
        }
    }

    pub(super) fn dependency_response() -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status: 502,
            body: Some(Bytes::from_static(b"502 Bad Gateway")),
        }
    }

    pub(super) async fn send_redirect_with_cookies(
        &self,
        session: &mut dyn PluginSession,
        location: &str,
        cookies: &[String],
    ) -> OidcResult<()> {
        let mut resp = ResponseHeader::build(302, None)?;
        resp.insert_header("Location", location)?;
        if let Some(first_cookie) = cookies.first() {
            resp.insert_header("Set-Cookie", first_cookie)?;
            for cookie in &cookies[1..] {
                resp.append_header("Set-Cookie", cookie)?;
            }
        }
        resp.insert_header("Cache-Control", "no-store")?;
        resp.insert_header("Pragma", "no-cache")?;
        resp.insert_header("Content-Length", "0")?;
        session.write_response_header(Box::new(resp), true).await?;
        Ok(())
    }

    pub(super) fn build_authorization_redirect(
        &self,
        session: &dyn PluginSession,
        discovery: &DiscoveryDocument,
    ) -> VerifyResult<(String, String)> {
        let authorization_endpoint = discovery
            .authorization_endpoint
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or(OidcVerifyError::dependency(
                OidcOperation::Authorization,
                OidcDependencyFailure::MissingEndpoint,
            ))?;

        let mut auth_url = Url::parse(authorization_endpoint).map_err(|_| {
            OidcVerifyError::dependency(OidcOperation::Authorization, OidcDependencyFailure::InvalidEndpoint)
        })?;

        let redirect_uri = self.resolve_redirect_uri(session)?;
        let state = Self::random_urlsafe::<24>();
        let mut code_verifier: Option<String> = None;
        let mut code_challenge: Option<String> = None;
        let mut nonce: Option<String> = None;

        if self.config.use_pkce {
            let verifier = Self::random_urlsafe::<64>();
            let digest = Sha256::digest(verifier.as_bytes());
            let challenge = URL_SAFE_NO_PAD.encode(digest);
            code_verifier = Some(verifier);
            code_challenge = Some(challenge);
        }
        if self.config.use_nonce {
            nonce = Some(Self::random_urlsafe::<24>());
        }

        {
            let mut qp = auth_url.query_pairs_mut();
            qp.append_pair("response_type", "code");
            qp.append_pair("client_id", &self.config.client_id);
            qp.append_pair("redirect_uri", &redirect_uri);
            qp.append_pair("scope", &self.config.scope);
            qp.append_pair("state", &state);
            if let Some(ref nonce_value) = nonce {
                qp.append_pair("nonce", nonce_value);
            }
            if let Some(ref challenge) = code_challenge {
                qp.append_pair("code_challenge_method", "S256");
                qp.append_pair("code_challenge", challenge);
            }
            if let Some(ref params) = self.config.authorization_params {
                for (key, value) in params {
                    if Self::is_reserved_authorization_param(key) {
                        continue;
                    }
                    qp.append_pair(key, value);
                }
            }
        }

        let original_url = match session.request_query() {
            Some(q) if !q.is_empty() => format!("{}?{}", session.get_path(), q),
            _ => session.get_path().to_string(),
        };
        let cookie_payload = AuthorizationStateCookie {
            state,
            original_url,
            code_verifier,
            nonce,
            created_at: Self::now_unix_secs(),
        };
        let cipher = self.session_cipher().ok_or_else(Self::credential_failure)?;
        let cookie_value = Self::encode_signed_cookie_payload(&cookie_payload, cipher)
            .map_err(|_| OidcVerifyError::SessionEncoding)?;
        let cookie =
            self.build_cookie_header_value(Self::state_cookie_name(), &cookie_value, 300, self.callback_path());

        Ok((auth_url.to_string(), cookie))
    }

    pub(super) async fn handle_callback(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        code: &str,
        state_param: &str,
    ) -> PluginRunningResult {
        if self.session_cipher().is_none() {
            plugin_log.push_err("session credential unavailable");
            let err = Self::credential_failure();
            return self.send_oidc_error(session, &err).await;
        }

        let Some(raw_state_cookie) = session.get_cookie(Self::state_cookie_name()) else {
            let _ = self
                .send_unauthorized(session, "Unauthorized - Missing OIDC state cookie")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        };
        let decoded_state = match self.session_cipher() {
            Some(cipher) => Self::decode_signed_cookie_payload(&raw_state_cookie, cipher),
            None => Err(Self::credential_failure()),
        };
        drop(raw_state_cookie);
        let state_cookie: AuthorizationStateCookie = match decoded_state {
            Ok(v) => v,
            Err(err) => {
                return self.send_oidc_error(session, &err).await;
            }
        };

        let now = Self::now_unix_secs();
        if now.saturating_sub(state_cookie.created_at) > 300 {
            let _ = self
                .send_unauthorized(session, "Unauthorized - OIDC state expired")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }
        if state_cookie.state != state_param {
            let _ = self.send_plain_error(session, 400, "Invalid OIDC callback state").await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        let discovery = match self.get_or_fetch_discovery().await {
            Ok(d) => d,
            Err(e) => {
                if e.failure == OidcDependencyFailure::TlsSanMismatch {
                    session.record_error(edgion_resources::EdgionErrorCode::OutboundTlsSanMismatch);
                }
                plugin_log.push_err(e.log_tag());
                return Self::dependency_response();
            }
        };

        let redirect_uri = match self.resolve_redirect_uri(session) {
            Ok(v) => v,
            Err(err) => {
                return self.send_oidc_error(session, &err).await;
            }
        };

        let token_response = match self
            .exchange_code_for_token(&discovery, code, &redirect_uri, state_cookie.code_verifier.as_deref())
            .await
        {
            Ok(v) => v,
            Err(err) => {
                plugin_log.push_err(err.response_log_tag_or("token exchange fail"));
                return self.send_oidc_error(session, &err).await;
            }
        };

        if self.config.use_nonce {
            let expected_nonce = match state_cookie.nonce.as_deref() {
                Some(v) if !v.is_empty() => v,
                _ => {
                    let _ = self.send_plain_error(session, 400, "Invalid OIDC callback nonce").await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
            };
            let id_token = match token_response.id_token.as_deref() {
                Some(v) if !v.is_empty() => v,
                _ => {
                    let _ = self.send_unauthorized(session, "Unauthorized - Missing ID token").await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
            };
            if let Err(err) = self.validate_id_token_nonce(session, id_token, expected_nonce).await {
                plugin_log.push_err(err.log_tag());
                return self.send_oidc_error(session, &err).await;
            }
        }

        if let Err(err) = self.verify_token(session, &token_response.access_token).await {
            plugin_log.push_err(err.log_tag());
            return self.send_oidc_error(session, &err).await;
        }

        let expires_at = token_response.expires_in.map(|ttl| now.saturating_add(ttl));
        let should_persist_id_token = self.should_persist_id_token(&discovery);
        let mut session_cookie_payload = OidcSessionCookie {
            session_ref: Self::generate_session_ref(),
            access_token: token_response.access_token,
            created_at: now,
            expires_at,
            id_token: if should_persist_id_token {
                token_response.id_token.filter(|v| !v.is_empty())
            } else {
                None
            },
            refresh_token: token_response.refresh_token,
        };
        self.cache_access_token(
            &session_cookie_payload.session_ref,
            &session_cookie_payload.access_token,
            session_cookie_payload.expires_at,
        )
        .await;
        session_cookie_payload.access_token.clear();

        let encoded_session = match self.session_cipher() {
            Some(cipher) => Self::encode_signed_cookie_payload(&session_cookie_payload, cipher),
            None => {
                plugin_log.push_err("session credential unavailable");
                Err(())
            }
        };
        drop(session_cookie_payload);
        let session_cookie_value = match encoded_session {
            Ok(v) => v,
            Err(()) if self.session_cipher().is_none() => {
                let err = Self::credential_failure();
                return self.send_oidc_error(session, &err).await;
            }
            Err(()) => {
                plugin_log.push_err("session encode fail");
                return Self::dependency_response();
            }
        };

        let session_cookie = self.build_cookie_header_value(
            self.session_cookie_name(),
            &session_cookie_value,
            self.config.session_lifetime,
            "/",
        );
        if self.ensure_session_cookie_size_limit(&session_cookie).is_err() {
            let _ = self
                .send_plain_error(session, 500, "Session cookie exceeds configured size limit")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }
        let clear_state_cookie = self.build_cookie_header_value(Self::state_cookie_name(), "", 0, self.callback_path());
        let redirect_target = Self::sanitize_redirect_target(&state_cookie.original_url);
        let cookies = [session_cookie, clear_state_cookie];
        let _ = self
            .send_redirect_with_cookies(session, redirect_target, &cookies)
            .await;

        plugin_log.push("OIDC callback exchanged code and established session");
        PluginRunningResult::ErrTerminateRequest
    }
}
