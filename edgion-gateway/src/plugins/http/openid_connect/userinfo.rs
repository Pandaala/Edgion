use super::super::error::read_oidc_json;
use super::*;

impl OpenidConnect {
    /// Whether `value` is valid HTTP header text — the same check the write
    /// performs, so an IdP value with a control byte is skipped (destination
    /// left absent) instead of failing the write and denying the request.
    pub(super) fn is_header_value_safe(value: &str) -> bool {
        crate::plugins::http::common::auth_common::is_header_text(value)
    }

    /// Write one fixed output within the per-value and aggregate caps.
    ///
    /// The destination was removed before this call, so a skipped value (oversized,
    /// unsafe, or over budget) leaves it absent. A failed write is returned so the
    /// caller denies instead of forwarding.
    pub(super) fn try_set_header_with_limits(
        &self,
        session: &mut dyn PluginSession,
        header_name: &str,
        header_value: &str,
        total_added_bytes: &mut usize,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        let max_value = self.config.max_header_value_bytes as usize;
        let max_total = self.config.max_total_header_bytes as usize;

        if header_value.len() > max_value {
            return Ok(());
        }
        if !Self::is_header_value_safe(header_value) {
            return Ok(());
        }

        let add = header_name.len() + header_value.len();
        if total_added_bytes.saturating_add(add) > max_total {
            return Ok(());
        }

        crate::plugins::http::common::auth_common::set_sensitive_request_header(session, header_name, header_value)?;
        *total_added_bytes = total_added_bytes.saturating_add(add);
        Ok(())
    }

    /// Remove every enabled fixed output so a client-supplied copy cannot reach the
    /// upstream on any continuation. `Authorization` is owned only for a
    /// session-cookie authentication with `accessTokenInAuthorizationHeader`; in
    /// bearer mode it carries the verified client token and stays.
    pub(super) fn clear_fixed_outputs(
        &self,
        session: &mut dyn PluginSession,
        owns_authorization: bool,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        if self.config.set_access_token_header {
            session.remove_request_header("x-access-token")?;
        }
        if self.config.set_id_token_header {
            session.remove_request_header("x-id-token")?;
        }
        if self.config.set_userinfo_header {
            session.remove_request_header("x-userinfo")?;
        }
        if owns_authorization && self.config.access_token_in_authorization_header {
            session.remove_request_header("authorization")?;
        }
        Ok(())
    }

    /// Publish upstream identity headers. `from_session` selects session-cookie mode,
    /// where `Authorization` is a gateway-owned output and is cleared first.
    pub(super) fn apply_upstream_headers(
        &self,
        session: &mut dyn PluginSession,
        access_token: &str,
        id_token: Option<&str>,
        userinfo_json: Option<&str>,
        claims: &Value,
        from_session: bool,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        // Clear configured claim destinations and fixed outputs before any upstream
        // header write, so a cleanup failure leaves no gateway-written value.
        crate::plugins::http::common::auth_common::clear_claim_headers(session, &self.claim_header_projection)?;
        self.clear_fixed_outputs(session, from_session)?;
        let mut total_added: usize = 0;
        let max_total = self.config.max_total_header_bytes as usize;
        let max_value = self.config.max_header_value_bytes as usize;

        if self.config.set_access_token_header {
            self.try_set_header_with_limits(session, "X-Access-Token", access_token, &mut total_added)?;
        }

        if self.config.set_id_token_header {
            if let Some(id_token) = id_token.filter(|v| !v.trim().is_empty()) {
                self.try_set_header_with_limits(session, "X-ID-Token", id_token, &mut total_added)?;
            }
        }

        if self.config.set_userinfo_header {
            if let Some(userinfo) = userinfo_json {
                self.try_set_header_with_limits(session, "X-Userinfo", userinfo, &mut total_added)?;
            } else if let Ok(userinfo) = serde_json::to_string(claims) {
                self.try_set_header_with_limits(session, "X-Userinfo", &userinfo, &mut total_added)?;
            }
        }

        if self.config.access_token_in_authorization_header
            && access_token
                .len()
                .checked_add("Bearer ".len())
                .is_some_and(|value_len| {
                    value_len <= max_value
                        && total_added
                            .saturating_add("Authorization".len())
                            .saturating_add(value_len)
                            <= max_total
                })
        {
            let value = format!("Bearer {}", access_token);
            self.try_set_header_with_limits(session, "Authorization", &value, &mut total_added)?;
        }

        if self.config.store_claims_in_ctx {
            if let Ok(claims_json) = serde_json::to_string(claims) {
                let _ = session.set_ctx_var("oidc_claims", &claims_json);
            }
        }

        // Destinations were cleared above, so a zero remaining budget retains no input.
        crate::plugins::http::common::auth_common::project_cleared_claim_headers(
            session,
            claims,
            &self.claim_header_projection,
            max_value,
            max_total.saturating_sub(total_added),
        )
    }

    pub(super) async fn fetch_userinfo_json(&self, access_token: &str, plugin_log: &mut PluginLog) -> Option<String> {
        if !self.config.set_userinfo_header {
            return None;
        }
        match self.fetch_userinfo(access_token).await {
            Ok(value) => Some(value),
            Err(error) => {
                plugin_log.push_err(error.log_tag());
                None
            }
        }
    }

    async fn fetch_userinfo(&self, access_token: &str) -> VerifyResult<String> {
        let discovery = self.get_or_fetch_discovery().await?;
        let endpoint =
            discovery
                .userinfo_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(OidcVerifyError::dependency(
                    OidcOperation::Userinfo,
                    OidcDependencyFailure::MissingEndpoint,
                ))?;
        Self::admit_idp_url(endpoint, OidcOperation::Userinfo)?;
        let response = self
            .http_client()
            .get(endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(OidcOperation::Userinfo, &error, "openid_connect"))?;
        if !response.status().is_success() {
            return Err(OidcVerifyError::dependency(
                OidcOperation::Userinfo,
                OidcDependencyFailure::HttpStatus,
            ));
        }
        let value: Value = read_oidc_json(response, self.config.userinfo_max_response_bytes)
            .await
            .map_err(|failure| OidcVerifyError::dependency(OidcOperation::Userinfo, failure))?;
        serde_json::to_string(&value)
            .map_err(|_| OidcVerifyError::dependency(OidcOperation::Userinfo, OidcDependencyFailure::InvalidDocument))
    }
}
