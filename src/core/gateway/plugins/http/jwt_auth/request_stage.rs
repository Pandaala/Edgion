//! Request-stage operations: token extraction, anonymous fallback, 401 response,
//! auth-failure delay, hide_credentials, claims-to-headers propagation.

use std::time::Duration;

use super::{JwtAuth, JwtAuthResult};
use crate::core::gateway::plugins::http::common::auth_common::{
    send_auth_error_response, set_claims_headers as set_common_claims_headers,
};
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession};

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
    pub(super) fn extract_token(&self, session: &dyn PluginSession) -> Option<(String, TokenSource)> {
        if let Some(v) = session.header_value(&self.config.header) {
            let token = if v.to_lowercase().starts_with("bearer ") {
                v[7..].trim().to_string()
            } else {
                v.trim().to_string()
            };
            if !token.is_empty() {
                return Some((token, TokenSource::Header));
            }
        }
        if let Some(t) = session.get_query_param(&self.config.query) {
            if !t.is_empty() {
                return Some((t, TokenSource::Query));
            }
        }
        if let Some(t) = session.get_cookie(&self.config.cookie) {
            if !t.is_empty() {
                return Some((t, TokenSource::Cookie));
            }
        }
        None
    }

    /// Set headers from JWT claims based on claims_to_headers configuration.
    pub(super) fn set_claims_headers(&self, session: &mut dyn PluginSession, claims: &serde_json::Value) {
        if let Some(ref mapping) = self.config.claims_to_headers {
            // Enforce hardcoded size limits to prevent a valid JWT from injecting arbitrarily
            // large claim values into upstream headers.
            set_common_claims_headers(
                session,
                claims,
                mapping,
                MAX_JWT_CLAIM_HEADER_VALUE_BYTES,
                MAX_JWT_CLAIM_TOTAL_HEADER_BYTES,
            );
        }
    }

    /// Handle anonymous access.
    /// Sets X-Anonymous-Consumer and X-Consumer-Username headers.
    pub(super) fn handle_anonymous_access(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.push_fmt("Anon={}", la![sensitive(anonymous)]);
            let _ = session.set_request_header("X-Anonymous-Consumer", "true");
            let _ = session.set_request_header("X-Consumer-Username", anonymous);
            return true;
        }
        false
    }

    /// Return 401 Unauthorized response with WWW-Authenticate header.
    pub(super) async fn auth_failed_return(&self, session: &mut dyn PluginSession) -> JwtAuthResult<()> {
        send_auth_error_response(
            session,
            401,
            "Bearer",
            &self.config.realm,
            "Unauthorized - Invalid or missing JWT",
        )
        .await
    }

    /// Delay before returning auth failure (timing attack protection).
    pub(super) async fn apply_auth_failure_delay(&self) {
        if self.config.auth_failure_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.auth_failure_delay_ms)).await;
        }
    }

    /// Hide credentials based on token source.
    ///
    /// Only header-based tokens can be fully hidden. Query parameter and cookie tokens
    /// cannot be removed from the upstream request due to HTTP protocol limitations
    /// (would require URI/Cookie header rewriting). In those cases a short PluginLog
    /// tag surfaces the limitation via access log.
    pub(super) fn hide_credentials_if_needed(
        &self,
        session: &mut dyn PluginSession,
        source: TokenSource,
        plugin_log: &mut PluginLog,
    ) {
        if !self.config.hide_credentials {
            return;
        }
        match source {
            TokenSource::Header => {
                let _ = session.remove_request_header(&self.config.header);
            }
            TokenSource::Query => {
                plugin_log.push("Skip hide query");
            }
            TokenSource::Cookie => {
                plugin_log.push("Skip hide cookie");
            }
        }
    }
}
