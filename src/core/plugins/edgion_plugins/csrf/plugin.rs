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
use cookie::time::Duration;
use cookie::{Cookie, CookieBuilder, SameSite};

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::CsrfConfig;

use super::token::CsrfToken;

const SAFE_METHODS: &[&str] = &["GET", "HEAD"];

pub struct Csrf {
    name: String,
    config: CsrfConfig,
}

impl Csrf {
    /// Create a new CSRF plugin from configuration
    pub fn new(config: &CsrfConfig) -> Self {
        Csrf {
            name: "Csrf".to_string(),
            config: config.clone(),
        }
    }

    /// Extract cookie value by name from Cookie header using Cookie crate
    fn get_cookie_value(&self, session: &mut dyn PluginSession, cookie_name: &str) -> Option<String> {
        let cookie_header = session.header_value("cookie")?;

        // Parse cookies using the cookie crate to handle edge cases correctly
        for cookie_str in cookie_header.split(';') {
            if let Ok(c) = Cookie::parse(cookie_str.trim()) {
                if c.name() == cookie_name {
                    return Some(c.value().to_string());
                }
            }
        }

        None
    }

    /// Generate and set CSRF token cookie in response
    /// Enforces rigorous security practices for the cookie.
    fn set_csrf_cookie(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let token = CsrfToken::generate(&self.config.key);
        match token.encode() {
            Ok(encoded_token) => {
                // Build a secure cookie
                // Note: http_only is often FALSE for CSRF tokens in double-submit patterns
                // because the JS client needs to read it to populate the header.
                // We enforce Secure (HTTPS) and Lax SameSite.
                let cookie = CookieBuilder::new(&self.config.name, &encoded_token)
                    .path("/")
                    .secure(true) // Enforce Secure (assumes HTTPS, critical for security)
                    .same_site(SameSite::Lax) // Lax provides reasonable balance for CSRF
                    .max_age(Duration::seconds(self.config.expires))
                    .http_only(false) // Intentionally false so JS can read to set header
                    .build();

                if let Err(_e) = session.set_response_header("Set-Cookie", &cookie.to_string()) {
                    plugin_log.push("Token set failed; ");
                } else {
                    plugin_log.push("Token set; ");
                }
            }
            Err(_e) => {
                plugin_log.push("Token encode failed; ");
            }
        }
    }
}

#[async_trait]
impl RequestFilter for Csrf {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let method = session.method();

        // For safe methods, skip validation but set cookie for future use
        if SAFE_METHODS.contains(&method.as_str()) {
            self.set_csrf_cookie(session, plugin_log);
            return PluginRunningResult::GoodNext;
        }

        // 1. Get token from HEADER
        let header_token = match session.header_value(&self.config.name) {
            Some(token) if !token.is_empty() => token,
            _ => {
                plugin_log.push("No token in header; ");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"no csrf token in headers"}"#.to_string()),
                };
            }
        };

        // 2. Get token from COOKIE
        let cookie_token = match self.get_cookie_value(session, &self.config.name) {
            Some(token) => token,
            None => {
                plugin_log.push("No token in cookie; ");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"no csrf cookie"}"#.to_string()),
                };
            }
        };

        // 3. Tokens must MATCH (Double Submit Cookie Pattern)
        if header_token != cookie_token {
            plugin_log.push("Token mismatch; ");
            return PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(r#"{"error_msg":"csrf token mismatch"}"#.to_string()),
            };
        }

        // 4. Verify token SIGNATURE and EXPIRATION (Stateless checking)
        match CsrfToken::decode(&cookie_token) {
            Ok(token) => {
                if token.verify(&self.config.key, self.config.expires) {
                    plugin_log.push("Token verified; ");
                    PluginRunningResult::GoodNext
                } else {
                    plugin_log.push("Token invalid; ");
                    PluginRunningResult::ErrResponse {
                        status: 401,
                        body: Some(r#"{"error_msg":"Failed to verify the csrf token signature"}"#.to_string()),
                    }
                }
            }
            Err(_e) => {
                plugin_log.push("Token decode failed; ");
                PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(r#"{"error_msg":"Failed to verify the csrf token signature"}"#.to_string()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

    fn create_csrf_config() -> CsrfConfig {
        CsrfConfig {
            name: "X-CSRF-Token".to_string(),
            key: "test-secret-key-32-bytes-long!".to_string(),
            expires: 7200,
        }
    }

    #[tokio::test]
    async fn test_safe_method_sets_cookie() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Token set"));
    }

    #[tokio::test]
    async fn test_missing_header_token() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_method().returning(|| "POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| None);

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        match result {
            PluginRunningResult::ErrResponse { status, .. } => {
                assert_eq!(status, 401);
            }
            _ => panic!("Expected ErrResponse"),
        }
        assert!(plugin_log.contains("No token in header"));
    }

    #[tokio::test]
    async fn test_missing_cookie_token() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_method().returning(|| "POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("some-token".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("cookie"))
            .returning(|_| None);

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        match result {
            PluginRunningResult::ErrResponse { status, .. } => {
                assert_eq!(status, 401);
            }
            _ => panic!("Expected ErrResponse"),
        }
        assert!(plugin_log.contains("No token in cookie"));
    }

    #[tokio::test]
    async fn test_token_mismatch() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_method().returning(|| "POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("token1".to_string()));
        // In real usage, cookie::Cookie::parse would handle this string
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("cookie"))
            .returning(|_| Some("X-CSRF-Token=token2".to_string()));

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        match result {
            PluginRunningResult::ErrResponse { status, .. } => {
                assert_eq!(status, 401);
            }
            _ => panic!("Expected ErrResponse"),
        }
        assert!(plugin_log.contains("Token mismatch"));
    }
}
