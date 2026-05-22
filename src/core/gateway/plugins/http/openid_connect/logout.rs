use super::*;

impl OpenidConnect {
    pub(super) fn sanitize_redirect_target(original_url: &str) -> String {
        if original_url.starts_with('/') && !original_url.starts_with("//") {
            original_url.to_string()
        } else {
            "/".to_string()
        }
    }

    pub(super) fn logout_redirect_target(&self) -> String {
        match self.config.post_logout_redirect_uri.as_deref() {
            Some(v) if !v.is_empty() => {
                if v.starts_with('/') || v.starts_with("http://") || v.starts_with("https://") {
                    v.to_string()
                } else {
                    "/".to_string()
                }
            }
            _ => "/".to_string(),
        }
    }

    pub(super) fn resolve_post_logout_redirect_uri_for_idp(&self, session: &dyn PluginSession) -> Option<String> {
        let target = self.logout_redirect_target();
        if target.starts_with("https://") || target.starts_with("http://") {
            return Some(target);
        }
        if !target.starts_with('/') {
            return None;
        }
        let host = Self::request_host(session)?;
        let scheme = Self::request_scheme(session);
        Some(format!("{}://{}{}", scheme, host, target))
    }

    pub(super) async fn build_logout_redirect_target(
        &self,
        fallback_target: String,
        post_logout_redirect_uri: Option<String>,
        id_token_hint: Option<String>,
    ) -> String {
        let end_session_endpoint = self
            .jwks_provider
            .cached_discovery()
            .await
            .and_then(|d| d.end_session_endpoint.clone())
            .filter(|v| !v.is_empty());
        let Some(end_session_endpoint) = end_session_endpoint else {
            return fallback_target;
        };

        let mut end_session_url = match Url::parse(&end_session_endpoint) {
            Ok(v) => v,
            Err(_) => return fallback_target,
        };
        if let Some(post_logout_redirect_uri) = post_logout_redirect_uri {
            end_session_url
                .query_pairs_mut()
                .append_pair("post_logout_redirect_uri", &post_logout_redirect_uri);
        }
        if let Some(id_token_hint) = id_token_hint.filter(|v| !v.is_empty()) {
            end_session_url
                .query_pairs_mut()
                .append_pair("id_token_hint", &id_token_hint);
        }
        end_session_url.to_string()
    }
}
