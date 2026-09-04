use super::*;
use crate::plugins::http::common::http_util::fetch_json_limited;

impl OpenidConnect {
    pub(super) fn is_header_value_safe(value: &str) -> bool {
        !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    }

    pub(super) fn try_set_header_with_limits(
        &self,
        session: &mut dyn PluginSession,
        header_name: &str,
        header_value: &str,
        total_added_bytes: &mut usize,
    ) {
        let max_value = self.config.max_header_value_bytes as usize;
        let max_total = self.config.max_total_header_bytes as usize;

        if !Self::is_header_value_safe(header_value) {
            return;
        }
        if header_value.len() > max_value {
            return;
        }

        let add = header_name.len() + header_value.len();
        if total_added_bytes.saturating_add(add) > max_total {
            return;
        }

        *total_added_bytes = total_added_bytes.saturating_add(add);
        let _ = session.set_request_header(header_name, header_value);
    }

    pub(super) fn apply_upstream_headers(
        &self,
        session: &mut dyn PluginSession,
        access_token: &str,
        id_token: Option<&str>,
        userinfo_json: Option<&str>,
        claims: &Value,
    ) {
        let mut total_added: usize = 0;
        let max_total = self.config.max_total_header_bytes as usize;
        let max_value = self.config.max_header_value_bytes as usize;

        if self.config.set_access_token_header {
            self.try_set_header_with_limits(session, "X-Access-Token", access_token, &mut total_added);
        }

        if self.config.set_id_token_header {
            if let Some(id_token) = id_token.filter(|v| !v.trim().is_empty()) {
                self.try_set_header_with_limits(session, "X-ID-Token", id_token, &mut total_added);
            }
        }

        if self.config.set_userinfo_header {
            if let Some(userinfo) = userinfo_json {
                self.try_set_header_with_limits(session, "X-Userinfo", userinfo, &mut total_added);
            } else if let Ok(userinfo) = serde_json::to_string(claims) {
                self.try_set_header_with_limits(session, "X-Userinfo", &userinfo, &mut total_added);
            }
        }

        if self.config.access_token_in_authorization_header {
            let value = format!("Bearer {}", access_token);
            self.try_set_header_with_limits(session, "Authorization", &value, &mut total_added);
        }

        if self.config.store_claims_in_ctx {
            if let Ok(claims_json) = serde_json::to_string(claims) {
                let _ = session.set_ctx_var("oidc_claims", &claims_json);
            }
        }

        if let Some(ref mapping) = self.config.claims_to_headers {
            let remaining_total = max_total.saturating_sub(total_added);
            if remaining_total > 0 {
                set_common_claims_headers(session, claims, mapping, max_value, remaining_total);
            }
        }
    }

    pub(super) async fn fetch_userinfo_json(&self, access_token: &str) -> Option<String> {
        if !self.config.set_userinfo_header {
            return None;
        }
        let discovery = match self.get_or_fetch_discovery().await {
            Ok(v) => v,
            Err(_) => return None,
        };
        let userinfo_endpoint = match discovery.userinfo_endpoint.as_deref().filter(|v| !v.is_empty()) {
            Some(v) => v,
            None => return None,
        };

        if super::admit_oidc_outbound_url(userinfo_endpoint).is_err() {
            return None;
        }
        let client = self.http_client();
        let resp = match client
            .get(userinfo_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(v) => v,
            Err(e) => {
                super::token_exchange::note_oidc_san_mismatch(&e);
                return None;
            }
        };
        if !resp.status().is_success() {
            return None;
        }

        let value: Value =
            match fetch_json_limited(resp, self.config.userinfo_max_response_bytes, "OIDC userinfo").await {
                Ok(v) => v,
                Err(_) => return None,
            };
        serde_json::to_string(&value).ok()
    }
}
