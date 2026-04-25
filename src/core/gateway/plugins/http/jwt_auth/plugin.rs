//! JWT Authentication plugin implementation
//!
//! Verifies JWT from header (default Authorization), query, or cookie.
//! Supports HS256/HS384/HS512 (symmetric) and RS256/RS384/RS512, ES256/ES384/ES512 (asymmetric).
//! All credentials come from K8s Secret (secret_ref or secret_refs).

use std::sync::Arc;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::JwtAuthConfig;

use async_trait::async_trait;

use super::credentials::Credentials;
use super::request_stage::TokenSource;

/// JWT Authentication plugin
pub struct JwtAuth {
    pub(super) config: JwtAuthConfig,
    /// EdgionPlugins namespace (fallback for secret_ref without namespace)
    pub(super) plugin_namespace: String,
    /// Loaded credentials (lazily populated on first request)
    pub(super) credentials: Arc<parking_lot::RwLock<Option<Credentials>>>,
}

impl JwtAuth {
    /// Create from JwtAuthConfig with plugin namespace
    pub fn new(config: &JwtAuthConfig, plugin_namespace: String) -> Self {
        JwtAuth {
            config: config.clone(),
            plugin_namespace,
            credentials: Arc::new(parking_lot::RwLock::new(None)),
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
                // §3: key source
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
        match self.verify_token(&token, plugin_log) {
            Ok(result) => {
                plugin_log.push(&format!("OK u={}", result.username));
                // Set claims to headers if configured
                self.set_claims_headers(session, &result.claims);
                // Store claims in context if configured
                if let Some(ref claims_json) = result.claims_json {
                    let _ = session.set_ctx_var("jwt_claims", claims_json);
                }
                self.hide_credentials_if_needed(session, source, plugin_log);
                PluginRunningResult::GoodNext
            }
            Err(_) => {
                // Specific `Deny <kind>; ` or `err: <kind>; ` tag was already pushed inside verify_token.
                // Token present but invalid/expired — always reject, never allow anonymous
                self.apply_auth_failure_delay().await;
                let _ = self.auth_failed_return(session).await;
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}
