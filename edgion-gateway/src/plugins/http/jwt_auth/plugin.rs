//! JWT Authentication plugin implementation
//!
//! Verifies JWT from header (default Authorization), query, or cookie.
//! Supports HS256/HS384/HS512 (symmetric) and RS256/RS384/RS512, ES256/ES384 (asymmetric).
//! All credentials come from controller-resolved issuer groups (no runtime Secret fetch).

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, cookie_cleanup_failure, PreparedAuthChallenge, CLAIM_HEADER_MUTATION_FAILED,
    CREDENTIAL_REMOVAL_FAILED,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{ClaimHeaderProjectionPlan, JwtAuthConfig};

use async_trait::async_trait;

use super::credentials::JwtIssuerGroupStore;
use super::request_stage::{HideCredentialsError, TokenSource};
use super::verify;

/// JWT Authentication plugin
pub struct JwtAuth {
    pub(super) config: JwtAuthConfig,
    pub(super) auth_challenge: PreparedAuthChallenge,
    pub(super) claim_header_projection: ClaimHeaderProjectionPlan,
    /// Issuer-group store built once at construction from resolved_groups_by_iss
    pub(super) store: JwtIssuerGroupStore,
}

impl JwtAuth {
    /// Create from JwtAuthConfig with plugin namespace
    pub fn new(config: &JwtAuthConfig, _plugin_namespace: String) -> Self {
        let mut config = config.clone();
        let claim_header_projection = config.compile_claim_header_projection().unwrap_or_default();
        let resolved_groups_by_iss = config.resolved_groups_by_iss.take().unwrap_or_default();
        JwtAuth {
            auth_challenge: PreparedAuthChallenge::new("Bearer", &config.realm),
            store: JwtIssuerGroupStore::new(
                resolved_groups_by_iss,
                config.base64_secret,
                config.lifetime_grace_period,
            ),
            claim_header_projection,
            config,
        }
    }

    /// Delay, then send the 401 Bearer challenge. `error_code` is
    /// `Some("invalid_token")` when a token was present, `None` otherwise.
    async fn reject(&self, session: &mut dyn PluginSession, error_code: Option<&str>) -> PluginRunningResult {
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
        let _ = self.auth_failed_return(session, error_code).await;
        PluginRunningResult::ErrTerminateRequest
    }

    /// Deny after an identity-header mutation or credential removal failed. The
    /// response is the one a bad credential gets on the same path, so the
    /// failure is not distinguishable from outside; the error code keeps the
    /// (usually configuration-caused) failure alertable.
    async fn deny(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        tag: &'static str,
        error_code: Option<&str>,
    ) -> PluginRunningResult {
        crate::plugins::http::common::auth_common::note_auth_header_mutation_failure(session, plugin_log, tag);
        self.reject(session, error_code).await
    }
}

#[async_trait]
impl RequestFilter for JwtAuth {
    fn name(&self) -> &str {
        "JwtAuth"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let (token, source) = match self.extract_token(session) {
            Some(t) => {
                let src_tag = match t.1 {
                    TokenSource::Header => "token from header",
                    TokenSource::Query => "token from query",
                    TokenSource::Cookie => "token from cookie",
                };
                plugin_log.push(src_tag);
                t
            }
            None => {
                plugin_log.push("No token");
                return match self.handle_anonymous_access(session, plugin_log) {
                    Ok(true) => PluginRunningResult::GoodNext,
                    Ok(false) => self.reject(session, None).await,
                    Err(_) => self.deny(session, plugin_log, CLAIM_HEADER_MUTATION_FAILED, None).await,
                };
            }
        };

        // Verify token
        match verify::verify_jwt(token, &self.store, &self.config, plugin_log) {
            Ok(ok) => {
                if self.set_claims_headers(session, ok.claims()).is_err() {
                    return self
                        .deny(session, plugin_log, CLAIM_HEADER_MUTATION_FAILED, Some("invalid_token"))
                        .await;
                }
                if self.config.store_claims_in_ctx {
                    if let Ok(claims_json) = serde_json::to_string(ok.claims()) {
                        let _ = session.set_ctx_var("jwt_claims", &claims_json);
                    }
                }
                match self.hide_credentials_if_needed(session, source) {
                    Ok(()) => {}
                    Err(HideCredentialsError::Cookie(error)) => return cookie_cleanup_failure(error),
                    Err(HideCredentialsError::Removal) => {
                        return self
                            .deny(session, plugin_log, CREDENTIAL_REMOVAL_FAILED, Some("invalid_token"))
                            .await;
                    }
                }
                // Success line only after every mutation and cleanup succeeded.
                let username = ok
                    .claims()
                    .get("sub")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(missing sub)");
                plugin_log.push_fmt(
                    "OK group={} iss={} u={}",
                    la![ok.group.group.as_str(), ok.issuer(), sensitive(username)],
                );
                PluginRunningResult::GoodNext
            }
            Err(_) => {
                // Specific `Deny <kind>; ` tag was already pushed inside verify_jwt.
                // Token present but invalid/expired -- always reject, never allow anonymous.
                self.reject(session, Some("invalid_token")).await
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/jwt_auth/plugin_test.rs"]
mod tests;
