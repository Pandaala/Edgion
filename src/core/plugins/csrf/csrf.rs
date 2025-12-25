//! CSRF (Cross-Site Request Forgery) protection plugin implementation
//!
//! This plugin protects against CSRF attacks by:
//! 1. Generating and setting CSRF tokens in response cookies (ResponseHeader stage)
//! 2. Validating tokens in requests (Request stage)
//!
//! ## Protection Mechanism:
//! - Safe methods (GET, HEAD, OPTIONS) skip CSRF validation
//! - Unsafe methods (POST, PUT, DELETE, etc.) require valid CSRF token
//! - Token must be present in both request header and cookie
//! - Token must match and have valid signature

use async_trait::async_trait;

use crate::core::filters::{Plugin, PluginSession, PluginLog};
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::types::resources::edgion_plugins::CsrfConfig;

use super::token::CsrfToken;

const SAFE_METHODS: &[&str] = &["GET", "HEAD", "OPTIONS"];

pub struct Csrf {
    name: String,
    config: CsrfConfig,
    stages: Vec<PluginRunningStage>,
}

impl Csrf {
    /// Create a new CSRF plugin from configuration
    pub fn new(config: &CsrfConfig) -> Self {
        Csrf {
            name: "Csrf".to_string(),
            config: config.clone(),
            stages: vec![PluginRunningStage::Request],
        }
    }

    /// Extract cookie value by name from Cookie header
    fn get_cookie_value(&self, session: &mut dyn PluginSession, cookie_name: &str) -> Option<String> {
        let cookie_header = session.header_value("cookie")?;

        for part in cookie_header.split(';') {
            let part = part.trim();
            if let Some(eq_pos) = part.find('=') {
                let key = &part[..eq_pos];
                if key == cookie_name {
                    let value = &part[eq_pos + 1..];
                    return Some(value.to_string());
                }
            }
        }

        None
    }

    /// Generate and set CSRF token cookie in response
    fn set_csrf_cookie(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let token = CsrfToken::generate(&self.config.key);
        match token.encode() {
            Ok(encoded_token) => {
                let cookie_value = format!(
                    "{}={}; Path=/; SameSite=Lax; Max-Age={}",
                    self.config.name,
                    encoded_token,
                    self.config.expires
                );

                if let Err(e) = session.set_response_header("Set-Cookie", &cookie_value) {
                    plugin_log.add_plugin_log(&format!("Failed to set cookie: {}; ", e));
                } else {
                    plugin_log.add_plugin_log("Token set in cookie; ");
                }
            }
            Err(e) => {
                plugin_log.add_plugin_log(&format!("Failed to encode token: {}; ", e));
            }
        }
    }
}

#[async_trait]
impl Plugin for Csrf {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        if stage != PluginRunningStage::Request {
            return PluginRunningResult::Nothing;
        }

        let method = session.method();

        // For safe methods, skip validation but set cookie for future use
        if SAFE_METHODS.contains(&method.as_str()) {
            plugin_log.add_plugin_log(&format!("Safe method {}, setting token; ", method));
            self.set_csrf_cookie(session, plugin_log);
            return PluginRunningResult::GoodNext;
        }

        plugin_log.add_plugin_log(&format!("Checking token for method {}; ", method));

        // Get token from header
        let header_token = match session.header_value(&self.config.name) {
            Some(token) if !token.is_empty() => token,
            _ => {
                plugin_log.add_plugin_log("No token in headers; ");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"no csrf token in headers"}"#.to_string()),
                };
            }
        };

        // Get token from cookie
        let cookie_token = match self.get_cookie_value(session, &self.config.name) {
            Some(token) => token,
            None => {
                plugin_log.add_plugin_log("No csrf cookie; ");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"no csrf cookie"}"#.to_string()),
                };
            }
        };

        // Tokens must match
        if header_token != cookie_token {
            plugin_log.add_plugin_log("Token mismatch; ");
            return PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(r#"{"error_msg":"csrf token mismatch"}"#.to_string()),
            };
        }

        // Verify token signature and expiration
        match CsrfToken::decode(&cookie_token) {
            Ok(token) => {
                if token.verify(&self.config.key, self.config.expires) {
                    plugin_log.add_plugin_log("Token verified successfully; ");
                    PluginRunningResult::GoodNext
                } else {
                    plugin_log.add_plugin_log("Failed to verify token signature; ");
                    PluginRunningResult::ErrResponse {
                        status: 401,
                        body: Some(r#"{"error_msg":"Failed to verify the csrf token signature"}"#.to_string()),
                    }
                }
            }
            Err(e) => {
                plugin_log.add_plugin_log(&format!("Failed to decode token: {}; ", e));
                PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"Failed to verify the csrf token signature"}"#.to_string()),
                }
            }
        }
    }

    fn get_stages(&self) -> Vec<PluginRunningStage> {
        self.stages.clone()
    }

    fn check_schema(&self, _conf: &PluginConf) {
        // Schema validation can be implemented here if needed
    }
}
