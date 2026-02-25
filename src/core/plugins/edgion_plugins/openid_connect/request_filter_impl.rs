use super::*;

#[async_trait]
impl RequestFilter for OpenidConnect {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if let Some(err) = self.config.get_validation_error() {
            plugin_log.push("Config invalid; ");
            tracing::warn!(
                plugin = "OpenidConnect",
                error = err,
                "Invalid OpenID Connect configuration"
            );
            let _ = self
                .send_unauthorized(session, "Unauthorized - Invalid OpenID Connect configuration")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        if session.get_path() == self.config.logout_path {
            let fallback_redirect_target = self.logout_redirect_target();
            let post_logout_redirect_uri = self.resolve_post_logout_redirect_uri_for_idp(session);
            let session_cookie = match self.extract_session_cookie_from_request(session) {
                Ok(Some((payload, _))) => Some(payload),
                Ok(None) => None,
                Err((_, message)) => {
                    tracing::warn!(
                        plugin = "OpenidConnect",
                        error = %message,
                        "Failed to decode session cookie for logout context"
                    );
                    None
                }
            };
            let id_token_hint = session_cookie.as_ref().and_then(|v| v.id_token.clone());
            self.maybe_revoke_tokens_on_logout(session_cookie.clone()).await;
            if let Some(ref payload) = session_cookie {
                self.remove_cached_access_token(&payload.session_ref).await;
            }
            let logout_redirect_target = self
                .build_logout_redirect_target(fallback_redirect_target, post_logout_redirect_uri, id_token_hint)
                .await;
            let clear_session_cookie = format!(
                "{}={}{}",
                self.session_cookie_name(),
                "",
                self.build_cookie_attr_suffix(session, 0, "/")
            );
            let clear_state_cookie = format!(
                "{}={}{}",
                Self::state_cookie_name(),
                "",
                self.build_cookie_attr_suffix(session, 0, &self.callback_path())
            );
            let cookies = vec![clear_session_cookie, clear_state_cookie];
            let _ = self
                .send_redirect_with_cookies(session, &logout_redirect_target, &cookies)
                .await;
            plugin_log.push("OIDC logout handled; ");
            return PluginRunningResult::ErrTerminateRequest;
        }

        if session.get_path() == self.callback_path() {
            if let Some(code) = session.get_query_param("code") {
                if let Some(state) = session.get_query_param("state") {
                    return self.handle_callback(session, plugin_log, &code, &state).await;
                }
                let _ = self.send_plain_error(session, 400, "Invalid OIDC callback state").await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        }

        let token = self.extract_bearer_token(session);
        if token.is_none() {
            if self.config.bearer_only {
                plugin_log.push("No token (bearer_only); ");
                self.apply_auth_failure_delay().await;
                let _ = self
                    .send_unauthorized(session, "Unauthorized - Missing bearer token")
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }

            match self.extract_session_cookie_from_request(session) {
                Ok(Some((mut session_cookie, raw_session_cookie_value))) => {
                    let mut token = self
                        .resolve_access_token_from_session(&session_cookie)
                        .await
                        .unwrap_or_default();
                    let refresh_lock_key = Self::refresh_lock_key(&raw_session_cookie_value);

                    if token.is_empty() && session_cookie.refresh_token.as_deref().is_some() {
                        match self
                            .try_refresh_session_token_singleflight(&refresh_lock_key, &mut session_cookie)
                            .await
                        {
                            Ok(new_token) => {
                                token = new_token;
                                self.maybe_set_session_cookie_header(session, &session_cookie).await;
                                plugin_log.push("OIDC session token refreshed (cache miss); ");
                            }
                            Err((status, message)) => {
                                tracing::warn!(
                                    plugin = "OpenidConnect",
                                    status = status,
                                    error = message,
                                    "OIDC cache miss refresh failed; falling back to unauth_action"
                                );
                            }
                        }
                    }

                    if self.should_preemptive_refresh(&session_cookie) {
                        match self
                            .try_refresh_session_token_singleflight(&refresh_lock_key, &mut session_cookie)
                            .await
                        {
                            Ok(new_token) => {
                                token = new_token;
                                self.maybe_set_session_cookie_header(session, &session_cookie).await;
                                plugin_log.push("OIDC session token refreshed (preemptive); ");
                            }
                            Err((status, message)) => {
                                tracing::warn!(
                                    plugin = "OpenidConnect",
                                    status = status,
                                    error = message,
                                    "OIDC preemptive refresh failed; trying current token"
                                );
                            }
                        }
                    }

                    if token.is_empty() {
                        tracing::warn!(
                            plugin = "OpenidConnect",
                            "OIDC session has no usable access token after cache/refresh; falling back to unauth_action"
                        );
                    } else {
                        match self.verify_token(&token).await {
                            Ok(claims) => {
                                let userinfo_json = self.fetch_userinfo_json(&token).await;
                                self.apply_upstream_headers(
                                    session,
                                    &token,
                                    session_cookie.id_token.as_deref(),
                                    userinfo_json.as_deref(),
                                    &claims,
                                );
                                plugin_log.push("OIDC session token verified; ");
                                return PluginRunningResult::GoodNext;
                            }
                            Err((status, message)) => {
                                let can_refresh_on_expired = self.config.renew_access_token_on_expiry
                                    && status == 401
                                    && message == "Token expired"
                                    && session_cookie.refresh_token.as_deref().is_some();

                                if can_refresh_on_expired {
                                    match self
                                        .try_refresh_session_token_singleflight(&refresh_lock_key, &mut session_cookie)
                                        .await
                                    {
                                        Ok(new_token) => match self.verify_token(&new_token).await {
                                            Ok(claims) => {
                                                self.maybe_set_session_cookie_header(session, &session_cookie).await;
                                                let userinfo_json = self.fetch_userinfo_json(&new_token).await;
                                                self.apply_upstream_headers(
                                                    session,
                                                    &new_token,
                                                    session_cookie.id_token.as_deref(),
                                                    userinfo_json.as_deref(),
                                                    &claims,
                                                );
                                                plugin_log.push("OIDC session token refreshed; ");
                                                return PluginRunningResult::GoodNext;
                                            }
                                            Err((s2, m2)) => {
                                                tracing::warn!(
                                                    plugin = "OpenidConnect",
                                                    status = s2,
                                                    error = m2,
                                                    "OIDC refreshed token verification failed; falling back to unauth_action"
                                                );
                                            }
                                        },
                                        Err((s2, m2)) => {
                                            let refresh_err = (s2, m2.clone());
                                            if Self::is_refresh_wait_timeout(&refresh_err) {
                                                match self.verify_jwt_token_allow_expired(&token).await {
                                                    Ok(claims) if self.is_expired_within_leeway(&claims) => {
                                                        let userinfo_json = self.fetch_userinfo_json(&token).await;
                                                        self.apply_upstream_headers(
                                                            session,
                                                            &token,
                                                            session_cookie.id_token.as_deref(),
                                                            userinfo_json.as_deref(),
                                                            &claims,
                                                        );
                                                        plugin_log.push(
                                                            "OIDC refresh wait timeout; accepted stale token within leeway; ",
                                                        );
                                                        return PluginRunningResult::GoodNext;
                                                    }
                                                    Ok(_) => {}
                                                    Err(_) => {}
                                                }
                                            }
                                            tracing::warn!(
                                                plugin = "OpenidConnect",
                                                status = s2,
                                                error = m2,
                                                "OIDC token refresh failed; falling back to unauth_action"
                                            );
                                        }
                                    }
                                } else {
                                    tracing::warn!(
                                        plugin = "OpenidConnect",
                                        status = status,
                                        error = message,
                                        "OIDC session token verification failed; falling back to unauth_action"
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err((status, message)) => {
                    if status == 502 {
                        let _ = self.send_plain_error(session, 502, &message).await;
                        return PluginRunningResult::ErrTerminateRequest;
                    }
                }
            }

            match self.config.unauth_action {
                UnauthAction::Pass => {
                    plugin_log.push("No token; pass; ");
                    return PluginRunningResult::GoodNext;
                }
                UnauthAction::Deny => {
                    plugin_log.push("No token; deny; ");
                    self.apply_auth_failure_delay().await;
                    let _ = self
                        .send_unauthorized(session, "Unauthorized - Authentication required")
                        .await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
                UnauthAction::Auth => {
                    plugin_log.push("No token; redirect to IdP; ");
                    let discovery = match self.get_or_fetch_discovery().await {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = self
                                .send_plain_error(
                                    session,
                                    502,
                                    &format!("Failed to fetch OIDC discovery document: {}", e),
                                )
                                .await;
                            return PluginRunningResult::ErrTerminateRequest;
                        }
                    };

                    match self.build_authorization_redirect(session, &discovery) {
                        Ok((location, cookie)) => {
                            let cookies = vec![cookie];
                            let _ = self.send_redirect_with_cookies(session, &location, &cookies).await;
                        }
                        Err((status, message)) => {
                            if status == 502 {
                                let _ = self.send_plain_error(session, 502, &message).await;
                            } else {
                                let _ = self.send_unauthorized(session, &message).await;
                            }
                        }
                    }
                    return PluginRunningResult::ErrTerminateRequest;
                }
            }
        }

        let token = token.unwrap_or_default();
        match self.verify_token(&token).await {
            Ok(claims) => {
                let userinfo_json = self.fetch_userinfo_json(&token).await;
                self.apply_upstream_headers(session, &token, None, userinfo_json.as_deref(), &claims);
                // hide_credentials: remove the original Authorization header from the upstream
                // request. Note: apply_upstream_headers may have already set a *new*
                // Authorization header (if access_token_in_authorization_header=true), but we
                // must call hide_credentials_if_needed BEFORE that step would overwrite it.
                // The current ordering is: apply_upstream_headers first (sets new Bearer header
                // when configured), then we remove the original – but since
                // apply_upstream_headers uses set_request_header which overwrites, the new
                // Bearer value is already set before we remove.
                // Actually: the original token was already in Authorization before this block.
                // apply_upstream_headers may overwrite Authorization if
                // access_token_in_authorization_header=true. In that case removing Authorization
                // afterwards would undo the overwrite. To avoid that conflict, we only remove
                // if access_token_in_authorization_header is false.
                if !self.config.access_token_in_authorization_header {
                    self.hide_credentials_if_needed(session);
                }
                plugin_log.push("OIDC token verified; ");
                PluginRunningResult::GoodNext
            }
            Err((status, message)) => {
                plugin_log.push("OIDC verify failed; ");
                // Apply failure delay only for actual auth failures (401/403),
                // not for upstream/infra errors (502).
                if status != 502 {
                    self.apply_auth_failure_delay().await;
                }
                match status {
                    403 => {
                        let _ = self.send_forbidden(session, &message).await;
                    }
                    502 => {
                        let _ = self.send_plain_error(session, 502, &message).await;
                    }
                    _ => {
                        let _ = self.send_unauthorized(session, &message).await;
                    }
                }
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}
