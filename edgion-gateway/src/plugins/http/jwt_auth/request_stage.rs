//! Request-stage operations: token extraction, anonymous fallback, 401 response,
//! hide_credentials, claims-to-headers propagation.

use super::{JwtAuth, JwtAuthResult};
use crate::plugins::http::common::auth_common::{
    send_gateway_auth_error_response, set_claims_headers as set_common_claims_headers, set_sensitive_request_header,
    strip_cookie, strip_query_param,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt};

/// Maximum size for a single JWT claim header value propagated upstream (8KB).
const MAX_JWT_CLAIM_HEADER_VALUE_BYTES: usize = 8 * 1024;
/// Maximum aggregate size of all JWT claim headers propagated upstream (64KB).
const MAX_JWT_CLAIM_TOTAL_HEADER_BYTES: usize = 64 * 1024;

/// Token source for hide_credentials.
#[derive(Debug, Clone, Copy)]
pub(super) enum TokenSource {
    Header,
    Query,
    Cookie,
}

impl JwtAuth {
    /// Extract token from request (priority: Header > Query > Cookie).
    pub(super) fn extract_token<'a>(&self, session: &'a dyn PluginSession) -> Option<(&'a str, TokenSource)> {
        if let Some(v) = session.header_str(&self.config.header) {
            let trimmed = v.trim();
            let token = if trimmed.len() >= 7 && trimmed.as_bytes()[..7].eq_ignore_ascii_case(b"bearer ") {
                trimmed[7..].trim_start()
            } else {
                trimmed
            };
            if !token.is_empty() {
                return Some((token, TokenSource::Header));
            }
        }
        // RFC 6750 §2.3/§5.3: bearer tokens in URLs leak into logs/history; opt-in only.
        if self.config.allow_token_in_query {
            if let Some(t) = session.get_query_param_ref(&self.config.query) {
                if !t.is_empty() {
                    return Some((t, TokenSource::Query));
                }
            }
        }
        if let Some(t) = session.get_cookie_ref(&self.config.cookie) {
            if !t.is_empty() {
                return Some((t, TokenSource::Cookie));
            }
        }
        None
    }

    /// Set headers from JWT claims based on claims_to_headers configuration.
    pub(super) fn set_claims_headers(
        &self,
        session: &mut dyn PluginSession,
        claims: &serde_json::Value,
    ) -> crate::plugins::runtime::PluginSessionResult<()> {
        crate::plugins::http::common::auth_common::sanitize_claim_headers(session, &self.claim_header_projection)?;
        if !self.claim_header_projection.entries().is_empty() {
            // Enforce hardcoded size limits to prevent a valid JWT from injecting arbitrarily
            // large claim values into upstream headers.
            set_common_claims_headers(
                session,
                claims,
                &self.claim_header_projection,
                MAX_JWT_CLAIM_HEADER_VALUE_BYTES,
                MAX_JWT_CLAIM_TOTAL_HEADER_BYTES,
            );
        }
        Ok(())
    }

    /// Handle anonymous access.
    /// Sets X-Anonymous-Consumer and X-Consumer-Username headers.
    pub(super) fn handle_anonymous_access(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.push_fmt("Anon={}", la![sensitive(anonymous)]);
            let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");
            let _ = set_sensitive_request_header(session, "X-Consumer-Username", anonymous);
            return true;
        }
        false
    }

    /// Return 401 Unauthorized response with WWW-Authenticate header.
    ///
    /// `error_code` is `Some("invalid_token")` when a token was present but failed
    /// validation, and `None` when the request carried no credentials at all (per
    /// RFC 6750 §3.1, the bare challenge is correct in the absent-credentials case).
    pub(super) async fn auth_failed_return(
        &self,
        session: &mut dyn PluginSession,
        error_code: Option<&str>,
    ) -> JwtAuthResult<()> {
        send_gateway_auth_error_response(
            session,
            401,
            &self.auth_challenge,
            error_code,
            "Unauthorized - Invalid or missing JWT",
        )
        .await
        .map_err(|_| ())
    }

    /// Hide credentials based on token source.
    pub(super) fn hide_credentials_if_needed(
        &self,
        session: &mut dyn PluginSession,
        source: TokenSource,
    ) -> Result<(), crate::request_cookie::CookieEditError> {
        if !self.config.hide_credentials {
            return Ok(());
        }
        match source {
            TokenSource::Header => {
                let _ = session.remove_request_header(&self.config.header);
            }
            TokenSource::Query => {
                strip_query_param(session, &self.config.query);
            }
            TokenSource::Cookie => {
                strip_cookie(session, &self.config.cookie)?;
            }
        }
        Ok(())
    }
}
