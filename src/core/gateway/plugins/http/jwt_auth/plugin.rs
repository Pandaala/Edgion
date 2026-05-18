//! JWT Authentication plugin implementation
//!
//! Verifies JWT from header (default Authorization), query, or cookie.
//! Supports HS256/HS384/HS512 (symmetric) and RS256/RS384/RS512, ES256/ES384 (asymmetric).
//! All credentials come from controller-resolved issuer groups (no runtime Secret fetch).

use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::JwtAuthConfig;

use async_trait::async_trait;

use super::credentials::JwtIssuerGroupStore;
use super::request_stage::TokenSource;
use super::verify;

/// JWT Authentication plugin
pub struct JwtAuth {
    pub(super) config: JwtAuthConfig,
    /// Issuer-group store built once at construction from resolved_groups_by_iss
    pub(super) store: JwtIssuerGroupStore,
}

impl JwtAuth {
    /// Create from JwtAuthConfig with plugin namespace
    pub fn new(config: &JwtAuthConfig, _plugin_namespace: String) -> Self {
        JwtAuth {
            store: JwtIssuerGroupStore::new(config.resolved_groups_by_iss.clone().unwrap_or_default()),
            config: config.clone(),
        }
    }
}

#[async_trait]
impl RequestFilter for JwtAuth {
    fn name(&self) -> &str {
        "JwtAuth"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Extract token
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
                // Check anonymous
                if self.handle_anonymous_access(session, plugin_log) {
                    return PluginRunningResult::GoodNext;
                }
                // Apply failure delay before returning 401
                self.apply_auth_failure_delay().await;
                let _ = self.auth_failed_return(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        // Verify token
        match verify::verify_jwt(&token, &self.store, &self.config, plugin_log) {
            Ok(ok) => {
                let username = ok.claims.get("sub").and_then(|v| v.as_str()).unwrap_or("(missing sub)");
                plugin_log.push_fmt(
                    "OK group={} iss={} u={}",
                    la![ok.group.group.as_str(), ok.iss.as_str(), sensitive(&username)],
                );
                // Set claims to headers if configured
                self.set_claims_headers(session, &ok.claims);
                // Store claims in context if configured
                if self.config.store_claims_in_ctx {
                    if let Ok(claims_json) = serde_json::to_string(&ok.claims) {
                        let _ = session.set_ctx_var("jwt_claims", &claims_json);
                    }
                }
                self.hide_credentials_if_needed(session, source, plugin_log);
                PluginRunningResult::GoodNext
            }
            Err(_) => {
                // Specific `Deny <kind>; ` tag was already pushed inside verify_jwt.
                // Token present but invalid/expired -- always reject, never allow anonymous.
                self.apply_auth_failure_delay().await;
                let _ = self.auth_failed_return(session).await;
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}
