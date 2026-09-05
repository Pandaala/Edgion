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

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
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
        let header_token = match session.header_value(&self.token_name) {
            Some(token) if !token.is_empty() => token,
            _ => {
                plugin_log.push("Deny missing token");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(Bytes::from_static(br#"{"error_msg":"no csrf token in headers"}"#)),
                };
            }
        };

        // 2. Get token from COOKIE
        let cookie_token = match session.get_cookie(&self.token_name) {
            Some(token) => token,
            None => {
                plugin_log.push("Deny missing token");
                return PluginRunningResult::ErrResponse {
                    status: 401,
                    body: Some(Bytes::from_static(br#"{"error_msg":"no csrf cookie"}"#)),
                };
            }
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    fn create_csrf_config() -> CsrfConfig {
        CsrfConfig {
            name: "X-CSRF-Token".to_string(),
            secret_ref: None,
            resolved_key: Some("test-secret-key-32-bytes-long!".to_string()),
            expires: 7200,
            cookie_secure: true,
        }
    }

    #[test]
    fn plugin_name_is_static_across_configurations() {
        let first = Csrf::new(&create_csrf_config());
        let mut second_config = create_csrf_config();
        second_config.name = "X-Other-CSRF-Token".to_string();
        let second = Csrf::new(&second_config);

        assert_eq!(first.name(), "Csrf");
        assert_eq!(second.name(), "Csrf");
    }

    #[test]
    fn construction_pins_only_the_runtime_generation_fields() {
        let mut config = create_csrf_config();
        let csrf = Csrf::new(&config);

        config.name = "X-Replaced-CSRF-Token".to_string();
        config.expires = 1;
        config.cookie_secure = false;
        config.resolved_key = Some("replacement-key".to_string());

        assert_eq!(csrf.token_name, "X-CSRF-Token");
        assert_eq!(csrf.expires, 7200);
        assert!(csrf.cookie_secure);
        assert_eq!(csrf.key, "test-secret-key-32-bytes-long!");
    }

    #[tokio::test]
    async fn test_safe_method_sets_cookie() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_get_method().return_const("GET".to_string());
        mock_session
            .expect_append_response_header()
            .withf(|name, value| {
                name == "Set-Cookie"
                    && value.starts_with("X-CSRF-Token=")
                    && value.contains("; Max-Age=7200")
                    && value.contains("; Path=/")
                    && value.contains("; SameSite=Lax")
                    && !value.contains("HttpOnly")
            })
            .returning(|_, _| Ok(()));

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Token set"));
    }

    #[tokio::test]
    async fn test_cookie_secure_true_emits_secure_attribute() {
        let config = create_csrf_config(); // cookie_secure = true
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_get_method().return_const("GET".to_string());
        mock_session
            .expect_append_response_header()
            .withf(|name, value| name == "Set-Cookie" && value.contains("; Secure"))
            .returning(|_, _| Ok(()));

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Token set"));
    }

    #[tokio::test]
    async fn test_cookie_secure_false_omits_secure_attribute() {
        let mut config = create_csrf_config();
        config.cookie_secure = false;
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_get_method().return_const("GET".to_string());
        mock_session
            .expect_append_response_header()
            .withf(|name, value| name == "Set-Cookie" && !value.contains("; Secure"))
            .returning(|_, _| Ok(()));

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

        mock_session.expect_get_method().return_const("POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| None);

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"no csrf token in headers"}"#)),
            }
        );
        assert!(plugin_log.contains("Deny missing token"));
    }

    #[tokio::test]
    async fn test_missing_cookie_token() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_get_method().return_const("POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("some-token".to_string()));
        mock_session
            .expect_get_cookie()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| None);

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"no csrf cookie"}"#)),
            }
        );
        assert!(plugin_log.contains("Deny missing token"));
    }

    #[tokio::test]
    async fn test_token_mismatch() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Csrf");

        mock_session.expect_get_method().return_const("POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("token1".to_string()));
        mock_session
            .expect_get_cookie()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("token2".to_string()));

        let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(br#"{"error_msg":"csrf token mismatch"}"#)),
            }
        );
        assert!(plugin_log.contains("Deny bad token"));
    }

    #[tokio::test]
    async fn malformed_matching_token_preserves_the_signature_failure_response() {
        let csrf = Csrf::new(&create_csrf_config());
        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_method().return_const("POST".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("not-a-valid-csrf-token".to_string()));
        mock_session
            .expect_get_cookie()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .returning(|_| Some("not-a-valid-csrf-token".to_string()));

        let mut plugin_log = PluginLog::new("Csrf");
        assert_eq!(
            csrf.run_request(&mut mock_session, &mut plugin_log).await,
            PluginRunningResult::ErrResponse {
                status: 401,
                body: Some(Bytes::from_static(
                    br#"{"error_msg":"Failed to verify the csrf token signature"}"#
                )),
            }
        );
        assert!(plugin_log.contains("Deny bad token"));
    }

    #[tokio::test]
    async fn padded_real_token_uses_the_canonical_cookie_accessor() {
        let config = create_csrf_config();
        let csrf = Csrf::new(&config);
        let token = CsrfToken::generate(&csrf.key).encode().unwrap();
        assert!(token.ends_with("=="));

        let mut mock_session = MockPluginSession::new();
        mock_session.expect_get_method().return_const("POST".to_string());
        let header_token = token.clone();
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .times(1)
            .return_once(move |_| Some(header_token));
        mock_session
            .expect_get_cookie()
            .with(mockall::predicate::eq("X-CSRF-Token"))
            .times(1)
            .return_once(move |_| Some(token));

        let mut plugin_log = PluginLog::new("Csrf");
        assert_eq!(
            csrf.run_request(&mut mock_session, &mut plugin_log).await,
            PluginRunningResult::GoodNext
        );
        assert!(plugin_log.contains("OK"));
    }
}
