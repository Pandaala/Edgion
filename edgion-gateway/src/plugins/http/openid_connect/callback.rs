use super::*;
use crate::plugins::http::common::auth_common::apply_auth_failure_delay as auth_delay;

impl OpenidConnect {
    pub(super) fn extract_bearer_token(&self, session: &dyn PluginSession) -> Option<String> {
        let header = session.header_value("authorization")?;
        let header = header.trim();
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
        send_gateway_auth_error_response(session, 401, "Bearer", &self.config.realm, None, body).await
    }

    pub(super) async fn send_forbidden(&self, session: &mut dyn PluginSession, body: &str) -> OidcResult<()> {
        // 403 is reached only on authorization failure (claim/audience/scope check):
        // a valid token lacking sufficient privilege -> RFC 6750 "insufficient_scope".
        send_gateway_auth_error_response(
            session,
            403,
            "Bearer",
            &self.config.realm,
            Some("insufficient_scope"),
            body,
        )
        .await
    }

    /// Apply failure delay before returning a 401/403 response.
    pub(super) async fn apply_auth_failure_delay(&self) {
        auth_delay(self.config.auth_failure_delay_ms).await;
    }

    /// Remove the Authorization header from the upstream request if hide_credentials is enabled.
    ///
    /// This is only meaningful in bearer_only mode (where the credential IS the Authorization
    /// header). In session-cookie mode the credential is in a cookie, so there is nothing to hide.
    pub(super) fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.config.hide_credentials {
            let _ = session.remove_request_header("authorization");
        }
    }

    pub(super) async fn send_plain_error(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: &str,
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

    /// Dispatch an `OidcVerifyError` to the matching error response: an upstream-side
    /// failure (`http_status() == 502`) becomes a plain 502, anything else a 401.
    /// This centralizes the 502-vs-401 decision so it cannot drift between the
    /// callback and request-filter error paths. The caller still owns logging and
    /// the `PluginRunningResult` it returns.
    pub(super) async fn send_oidc_error(&self, session: &mut dyn PluginSession, err: &OidcVerifyError) {
        if err.http_status() == 502 {
            let _ = self.send_plain_error(session, 502, &err.user_message()).await;
        } else {
            let _ = self.send_unauthorized(session, &err.user_message()).await;
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
            .ok_or(OidcVerifyError::Other {
                status: 502,
                message: "OIDC discovery document missing authorization_endpoint".to_string(),
            })?;

        let mut auth_url = Url::parse(authorization_endpoint).map_err(|e| OidcVerifyError::Other {
            status: 502,
            message: format!("Invalid authorization endpoint URL: {}", e),
        })?;

        let redirect_uri = self.resolve_redirect_uri(session)?;
        let state = Self::random_urlsafe(24);
        let mut code_verifier: Option<String> = None;
        let mut code_challenge: Option<String> = None;
        let mut nonce: Option<String> = None;

        if self.config.use_pkce {
            let verifier = Self::random_urlsafe(64);
            let digest = Sha256::digest(verifier.as_bytes());
            let challenge = URL_SAFE_NO_PAD.encode(digest);
            code_verifier = Some(verifier);
            code_challenge = Some(challenge);
        }
        if self.config.use_nonce {
            nonce = Some(Self::random_urlsafe(24));
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

        let original_url = match session.get_query() {
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
        let session_secret = self.resolve_session_secret().map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Missing session secret for auth flow: {}", e),
        })?;
        let cookie_value = self
            .encode_signed_cookie_payload(&cookie_payload, &session_secret)
            .map_err(|e| OidcVerifyError::Other {
                status: 502,
                message: format!("Failed to encode OIDC state cookie: {}", e),
            })?;
        let cookie_suffix = self.build_cookie_attr_suffix(session, 300, self.callback_path());
        let cookie = format!("{}={}{}", Self::state_cookie_name(), cookie_value, cookie_suffix);

        Ok((auth_url.to_string(), cookie))
    }

    pub(super) async fn handle_callback(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        code: &str,
        state_param: &str,
    ) -> PluginRunningResult {
        let session_secret = match self.resolve_session_secret() {
            Ok(v) => v,
            Err(e) => {
                let _ = self
                    .send_unauthorized(session, &format!("Unauthorized - Missing session secret: {}", e))
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let Some(raw_state_cookie) = session.get_cookie(Self::state_cookie_name()) else {
            let _ = self
                .send_unauthorized(session, "Unauthorized - Missing OIDC state cookie")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        };
        let state_cookie: AuthorizationStateCookie =
            match self.decode_signed_cookie_payload(&raw_state_cookie, &session_secret) {
                Ok(v) => v,
                Err(err) => {
                    self.send_oidc_error(session, &err).await;
                    return PluginRunningResult::ErrTerminateRequest;
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
                if let JwksProviderError::TlsSanMismatch { ref reason } = e {
                    session.record_error(edgion_resources::EdgionErrorCode::OutboundTlsSanMismatch);
                    plugin_log.push_err("outbound tls san mismatch");
                    plugin_log.push_fmt("san {}", la![reason.as_str()]);
                }
                let _ = self.send_plain_error(session, 502, "Bad Gateway").await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let redirect_uri = match self.resolve_redirect_uri(session) {
            Ok(v) => v,
            Err(err) => {
                self.send_oidc_error(session, &err).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let token_response = match self
            .exchange_code_for_token(&discovery, code, &redirect_uri, state_cookie.code_verifier.as_deref())
            .await
        {
            Ok(v) => v,
            Err(err) => {
                plugin_log.push_err("token exchange fail");
                self.send_oidc_error(session, &err).await;
                return PluginRunningResult::ErrTerminateRequest;
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
                self.send_oidc_error(session, &err).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        }

        let _claims = match self.verify_token(session, &token_response.access_token).await {
            Ok(v) => v,
            Err(err) => {
                plugin_log.push_err(err.log_tag());
                self.send_oidc_error(session, &err).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

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

        let session_cookie_value = match self.encode_signed_cookie_payload(&session_cookie_payload, &session_secret) {
            Ok(v) => v,
            Err(e) => {
                let _ = self
                    .send_plain_error(session, 502, &format!("Failed to encode session cookie: {}", e))
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let session_cookie = format!(
            "{}={}{}",
            self.session_cookie_name(),
            session_cookie_value,
            self.build_cookie_attr_suffix(session, self.config.session_lifetime, "/")
        );
        if self.ensure_session_cookie_size_limit(&session_cookie).is_err() {
            let _ = self
                .send_plain_error(session, 500, "Session cookie exceeds configured size limit")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }
        let clear_state_cookie = format!(
            "{}={}{}",
            Self::state_cookie_name(),
            "",
            self.build_cookie_attr_suffix(session, 0, self.callback_path())
        );
        let redirect_target = Self::sanitize_redirect_target(&state_cookie.original_url);
        let cookies = vec![session_cookie, clear_state_cookie];
        let _ = self
            .send_redirect_with_cookies(session, &redirect_target, &cookies)
            .await;

        plugin_log.push("OIDC callback exchanged code and established session");
        PluginRunningResult::ErrTerminateRequest
    }
}
