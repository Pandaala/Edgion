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
//! - Token must match and have valid signature (stateless)

use async_trait::async_trait;
use bytes::Bytes;
use cookie::time::Duration;
use cookie::{CookieBuilder, SameSite};

use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::CsrfConfig;

use super::token::CsrfToken;

const SAFE_METHODS: &[&str] = &["GET", "HEAD", "OPTIONS"];

pub struct Csrf {
    token_name: String,
    expires: i64,
    cookie_secure: bool,
    key: String,
}

impl Csrf {
    /// Create a new CSRF plugin from configuration
    pub fn new(config: &CsrfConfig) -> Self {
        Csrf {
            token_name: config.name.clone(),
            expires: config.expires,
            cookie_secure: config.cookie_secure,
            // Guaranteed non-empty by the plugin_validation_error gate before the
            // plugin goes live; unwrap_or_default is a defensive fallback.
            key: config.resolved_key.clone().unwrap_or_default(),
        }
    }

    /// Generate and set CSRF token cookie in response.
    fn set_csrf_cookie(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let token = CsrfToken::generate(&self.key);
        match token.encode() {
            Ok(encoded_token) => {
                // Note: http_only is often FALSE for CSRF tokens in double-submit patterns
                // because the JS client needs to read it to populate the header.
                // Secure defaults to true; operators terminating TLS upstream (plain HTTP
                // to the gateway) set cookie_secure=false so browsers keep returning it.
                let cookie = CookieBuilder::new(&self.token_name, &encoded_token)
                    .path("/")
                    .secure(self.cookie_secure)
                    .same_site(SameSite::Lax) // Lax provides reasonable balance for CSRF
                    .max_age(Duration::seconds(self.expires))
                    .http_only(false) // Intentionally false so JS can read to set header
                    .build();

                // Append, not set: Set-Cookie is a multi-value header (RFC 6265 §3 /
                // RFC 7230 §3.2.2 list-syntax exception). insert_header would replace
                // every upstream/other-plugin Set-Cookie; append_header preserves them.
                if let Err(_e) = session.append_response_header("Set-Cookie", &cookie.to_string()) {
                    plugin_log.push("Token set failed");
                } else {
                    plugin_log.push("Token set");
                }
            }
            Err(_e) => {
                plugin_log.push("Token encode failed");
            }
        }
    }
}

#[async_trait]
impl RequestFilter for Csrf {
    fn name(&self) -> &str {
        "Csrf"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let method = session.get_method();

        // For safe methods, skip validation but set cookie for future use
        if SAFE_METHODS.contains(&method) {
            plugin_log.push("Skip safe method");
            self.set_csrf_cookie(session, plugin_log);
            return PluginRunningResult::GoodNext;
        }

        // 1. Get token from HEADER
        let Some(header_token) = session.header_str(&self.token_name).filter(|token| !token.is_empty()) else {
            plugin_log.push("Deny missing token");
            return PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"no csrf token in headers"}"#)),
            };
        };

        // 2. Get token from COOKIE
        let Some(cookie_token) = session.get_cookie(&self.token_name) else {
            plugin_log.push("Deny missing token");
            return PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"no csrf cookie"}"#)),
            };
        };

        // 3. Tokens must MATCH (Double Submit Cookie Pattern)
        if header_token != cookie_token {
            plugin_log.push("Deny bad token");
            return PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"csrf token mismatch"}"#)),
            };
        }

        // 4. Verify token SIGNATURE and EXPIRATION (Stateless checking)
        match CsrfToken::decode(&cookie_token) {
            Ok(token) => {
                if token.verify(&self.key, self.expires) {
                    plugin_log.push("OK");
                    PluginRunningResult::GoodNext
                } else {
                    plugin_log.push("Deny bad token");
                    PluginRunningResult::ErrResponse {
                        status: 401,
                        body: Some(Bytes::from_static(
                            br#"{"error_msg":"Failed to verify the csrf token signature"}"#,
                        )),
                    }
                }
            }
            Err(_e) => {
                plugin_log.push("Deny bad token");
                PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(Bytes::from_static(
                        br#"{"error_msg":"Failed to verify the csrf token signature"}"#,
                    )),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/csrf/plugin_test.rs"]
mod tests;
