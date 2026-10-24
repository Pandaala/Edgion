use super::*;

#[async_trait]
impl RequestFilter for OpenidConnect {
    fn name(&self) -> &str {
        self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if session.get_path() == self.config.logout_path {
            let fallback_redirect_target = self.logout_redirect_target();
            let post_logout_redirect_uri = self.resolve_post_logout_redirect_uri_for_idp(session);
            let mut session_cookie = match self.extract_session_cookie_from_request(session) {
                Ok(Some((payload, _))) => Some(payload),
                Ok(None) => None,
                Err(_) => {
                    plugin_log.push_err("session decode fail");
                    None
                }
            };
            let id_token_hint = session_cookie.as_mut().and_then(|v| v.id_token.take());
            self.maybe_revoke_tokens_on_logout(session_cookie.as_ref(), plugin_log)
                .await;
            if let Some(session_ref) = session_cookie.map(|v| v.session_ref) {
                self.remove_cached_access_token(&session_ref).await;
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
                self.build_cookie_attr_suffix(session, 0, self.callback_path())
            );
            let cookies = [clear_session_cookie, clear_state_cookie];
            let _ = self
                .send_redirect_with_cookies(session, &logout_redirect_target, &cookies)
                .await;
            plugin_log.push("logout handled");
            return PluginRunningResult::ErrTerminateRequest;
        }

        if session.get_path() == self.callback_path() {
            let code = session.get_query_param("code").filter(|value| !value.is_empty());
            let state = session.get_query_param("state").filter(|value| !value.is_empty());
            if let (Some(code), Some(state)) = (code, state) {
                return self.handle_callback(session, plugin_log, &code, &state).await;
            }
            plugin_log.push_err("invalid callback");
            let _ = self.send_plain_error(session, 400, "Invalid OIDC callback").await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        let token = self.extract_bearer_token(session);
        if token.is_none() {
            if self.config.bearer_only {
                plugin_log.push("no token (bearer_only)");
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
                    let mut refresh_lock_key = None;

                    let cache_miss_refresh_attempted = token.is_empty() && session_cookie.refresh_token.is_some();
                    if cache_miss_refresh_attempted {
                        match self
                            .try_refresh_session_token_singleflight(
                                refresh_lock_key
                                    .get_or_insert_with(|| Self::refresh_lock_key(&raw_session_cookie_value)),
                                &mut session_cookie,
                            )
                            .await
                        {
                            Ok(new_token) => {
                                token = new_token;
                                self.maybe_set_session_cookie_header(session, &session_cookie).await;
                                plugin_log.push("token refresh miss");
                            }
                            Err(err) => {
                                plugin_log.push_err(err.response_log_tag_or("token refresh fail"));
                            }
                        }
                    }

                    if !cache_miss_refresh_attempted && self.should_preemptive_refresh(&token, &session_cookie) {
                        match self
                            .try_refresh_session_token_singleflight(
                                refresh_lock_key
                                    .get_or_insert_with(|| Self::refresh_lock_key(&raw_session_cookie_value)),
                                &mut session_cookie,
                            )
                            .await
                        {
                            Ok(new_token) => {
                                token = new_token;
                                self.maybe_set_session_cookie_header(session, &session_cookie).await;
                                plugin_log.push("token refresh preemptive");
                            }
                            Err(err) => {
                                plugin_log.push_err(err.response_log_tag_or("token refresh fail"));
                            }
                        }
                    }

                    if token.is_empty() {
                        plugin_log.push_err("session no token");
                    } else {
                        match self.verify_token(session, &token).await {
                            Ok(claims) => {
                                let userinfo_json = self.fetch_userinfo_json(&token, plugin_log).await;
                                self.apply_upstream_headers(
                                    session,
                                    &token,
                                    session_cookie.id_token.as_deref(),
                                    userinfo_json.as_deref(),
                                    &claims,
                                );
                                plugin_log.push("session token verified");
                                self.strip_gateway_owned_cookies(session);
                                return PluginRunningResult::GoodNext;
                            }
                            Err(err) => {
                                let can_refresh_on_expired = self.config.renew_access_token_on_expiry
                                    && matches!(err, OidcVerifyError::Expired)
                                    && session_cookie.refresh_token.as_deref().is_some();

                                if can_refresh_on_expired {
                                    match self
                                        .try_refresh_session_token_singleflight(
                                            refresh_lock_key.get_or_insert_with(|| {
                                                Self::refresh_lock_key(&raw_session_cookie_value)
                                            }),
                                            &mut session_cookie,
                                        )
                                        .await
                                    {
                                        Ok(new_token) => {
                                            drop(token);
                                            match self.verify_token(session, &new_token).await {
                                                Ok(claims) => {
                                                    self.maybe_set_session_cookie_header(session, &session_cookie)
                                                        .await;
                                                    let userinfo_json =
                                                        self.fetch_userinfo_json(&new_token, plugin_log).await;
                                                    self.apply_upstream_headers(
                                                        session,
                                                        &new_token,
                                                        session_cookie.id_token.as_deref(),
                                                        userinfo_json.as_deref(),
                                                        &claims,
                                                    );
                                                    plugin_log.push("token refresh on expiry");
                                                    self.strip_gateway_owned_cookies(session);
                                                    return PluginRunningResult::GoodNext;
                                                }
                                                Err(err) => {
                                                    plugin_log.push_err(err.response_log_tag_or("token verify fail"));
                                                }
                                            }
                                        }
                                        Err(refresh_err) => {
                                            if Self::is_refresh_wait_timeout(&refresh_err) {
                                                match self.verify_jwt_token_allow_expired(session, &token).await {
                                                    Ok(claims) if self.is_expired_within_leeway(&claims) => {
                                                        let userinfo_json =
                                                            self.fetch_userinfo_json(&token, plugin_log).await;
                                                        self.apply_upstream_headers(
                                                            session,
                                                            &token,
                                                            session_cookie.id_token.as_deref(),
                                                            userinfo_json.as_deref(),
                                                            &claims,
                                                        );
                                                        plugin_log.push(
                                                            "token refresh wait timeout; stale token within leeway",
                                                        );
                                                        self.strip_gateway_owned_cookies(session);
                                                        return PluginRunningResult::GoodNext;
                                                    }
                                                    Ok(_) => {}
                                                    Err(_) => {}
                                                }
                                            }
                                            plugin_log.push_err(refresh_err.response_log_tag_or("token refresh fail"));
                                        }
                                    }
                                } else {
                                    plugin_log.push_err(err.response_log_tag_or("token verify fail"));
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if err.http_status() == 502 {
                        return Self::dependency_response();
                    }
                }
            }

            match self.config.unauth_action {
                UnauthAction::Pass => {
                    plugin_log.push("no token; pass");
                    self.strip_gateway_owned_cookies(session);
                    return PluginRunningResult::GoodNext;
                }
                UnauthAction::Deny => {
                    plugin_log.push("no token; deny");
                    self.apply_auth_failure_delay().await;
                    let _ = self
                        .send_unauthorized(session, "Unauthorized - Authentication required")
                        .await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
                UnauthAction::Auth => {
                    plugin_log.push("no token; redirect to idp");
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

                    match self.build_authorization_redirect(session, &discovery) {
                        Ok((location, cookie)) => {
                            let cookies = [cookie];
                            let _ = self.send_redirect_with_cookies(session, &location, &cookies).await;
                        }
                        Err(err) => {
                            plugin_log.push_err(err.log_tag());
                            return self.send_oidc_error(session, &err).await;
                        }
                    }
                    return PluginRunningResult::ErrTerminateRequest;
                }
            }
        }

        let token = token.unwrap_or_default();
        match self.verify_token(session, &token).await {
            Ok(claims) => {
                let userinfo_json = self.fetch_userinfo_json(&token, plugin_log).await;
                self.apply_upstream_headers(session, &token, None, userinfo_json.as_deref(), &claims);
                // Skip credential hiding when access_token_in_authorization_header is true:
                // apply_upstream_headers already wrote a fresh "Bearer <token>" into
                // Authorization; removing it afterward would erase the intended upstream header.
                if !self.config.access_token_in_authorization_header {
                    self.hide_credentials_if_needed(session);
                }
                self.strip_gateway_owned_cookies(session);
                plugin_log.push("token verified");
                PluginRunningResult::GoodNext
            }
            Err(err) => {
                drop(token);
                plugin_log.push_err(err.log_tag());
                let status = err.http_status();
                let message = err.user_message();
                // Apply failure delay only for actual auth failures (401/403),
                // not for upstream/infra errors (502).
                if status != 502 {
                    self.apply_auth_failure_delay().await;
                }
                match status {
                    403 => {
                        let _ = self.send_forbidden(session, message).await;
                    }
                    502 => {
                        return Self::dependency_response();
                    }
                    _ => {
                        let _ = self.send_unauthorized(session, message).await;
                    }
                }
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}
