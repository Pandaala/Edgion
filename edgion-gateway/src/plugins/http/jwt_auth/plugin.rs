//! JWT Authentication plugin implementation
//!
//! Verifies JWT from header (default Authorization), query, or cookie.
//! Supports HS256/HS384/HS512 (symmetric) and RS256/RS384/RS512, ES256/ES384 (asymmetric).
//! All credentials come from controller-resolved issuer groups (no runtime Secret fetch).

use crate::plugins::http::common::auth_common::apply_auth_failure_delay;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{ClaimHeaderProjectionPlan, JwtAuthConfig};

use async_trait::async_trait;

use super::credentials::JwtIssuerGroupStore;
use super::request_stage::TokenSource;
use super::verify;

/// JWT Authentication plugin
pub struct JwtAuth {
    pub(super) config: JwtAuthConfig,
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
            store: JwtIssuerGroupStore::new(
                resolved_groups_by_iss,
                config.base64_secret,
                config.lifetime_grace_period,
            ),
            claim_header_projection,
            config,
        }
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
                if self.handle_anonymous_access(session, plugin_log) {
                    return PluginRunningResult::GoodNext;
                }
                // Apply failure delay before returning 401
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                let _ = self.auth_failed_return(session, None).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        // Verify token
        match verify::verify_jwt(&token, &self.store, &self.config, plugin_log) {
            Ok(ok) => {
                let username = ok.claims.get("sub").and_then(|v| v.as_str()).unwrap_or("(missing sub)");
                plugin_log.push_fmt(
                    "OK group={} iss={} u={}",
                    la![ok.group.group.as_str(), ok.iss.as_str(), sensitive(username)],
                );
                self.set_claims_headers(session, &ok.claims);
                if self.config.store_claims_in_ctx {
                    if let Ok(claims_json) = serde_json::to_string(&ok.claims) {
                        let _ = session.set_ctx_var("jwt_claims", &claims_json);
                    }
                }
                self.hide_credentials_if_needed(session, source);
                PluginRunningResult::GoodNext
            }
            Err(_) => {
                // Specific `Deny <kind>; ` tag was already pushed inside verify_jwt.
                // Token present but invalid/expired -- always reject, never allow anonymous.
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                let _ = self.auth_failed_return(session, Some("invalid_token")).await;
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
    };

    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    fn config_with_resolved_credential() -> JwtAuthConfig {
        let credential = ResolvedJwtCredential {
            key: Some("kid-1".to_string()),
            secret: Some("prepared-secret-at-least-32-bytes-long".to_string()),
            public_key: None,
        };
        let group = ResolvedJwtIssuerGroup {
            group: "group-1".to_string(),
            algorithm: JwtAlgorithm::HS256,
            audiences: None,
            credentials: HashMap::from([("kid-1".to_string(), credential)]),
        };
        JwtAuthConfig {
            resolved_groups_by_iss: Some(HashMap::from([("issuer-1".to_string(), group)])),
            ..Default::default()
        }
    }

    #[test]
    fn construction_consumes_only_the_runtime_config_resolved_credentials() {
        let config = config_with_resolved_credential();
        let auth = JwtAuth::new(&config, "default".to_string());

        assert!(config.resolved_groups_by_iss.is_some());
        assert!(auth.config.resolved_groups_by_iss.is_none());
        let prepared = auth.store.lookup_by_iss("issuer-1").unwrap();
        assert_eq!(prepared.group, "group-1");
        assert!(prepared.lookup_decoding_key("kid-1").unwrap().is_ok());
    }

    #[test]
    fn prepared_jwt_auth_is_send_and_sync() {
        assert_send_sync::<JwtAuth>();
    }
}
