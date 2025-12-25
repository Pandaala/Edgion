//! Basic Authentication plugin implementation

use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::collections::HashMap;

use crate::core::plugins::{Plugin, PluginSession, PluginLog};
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::types::resources::edgion_plugins::BasicAuthConfig;

use base64::{Engine as _, engine::general_purpose};
use async_trait::async_trait;

type BasicAuthError = Box<dyn std::error::Error + Send + Sync>;
type BasicAuthResult<T> = Result<T, BasicAuthError>;

/// Basic Authentication plugin
pub struct BasicAuth {
    name: String,
    // Simple username -> password_hash mapping
    user_passwords: HashMap<String, String>,
    config: BasicAuthConfig,
    stages: Vec<PluginRunningStage>,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        BasicAuth {
            name: "BasicAuth".to_string(),
            user_passwords: HashMap::new(),
            config: config.clone(),
            stages: vec![PluginRunningStage::Request],
        }
    }

    /// Load users from resolved Secret data (username -> plaintext password)
    /// Passwords will be hashed with bcrypt
    pub fn load_users(&mut self, users: HashMap<String, String>) -> Result<(), String> {
        self.user_passwords.clear();
        for (username, password) in users {
            let hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                .map_err(|e| format!("Failed to hash password for {}: {}", username, e))?;
            self.user_passwords.insert(username, hash);
        }
        tracing::info!("BasicAuth: Loaded {} users", self.user_passwords.len());
        Ok(())
    }

    fn authenticate_request(
        &self,
        session: &mut dyn PluginSession,
    ) -> BasicAuthResult<String> {
        // Extract authorization header
        let auth_header = session
            .header_value("authorization")
            .ok_or("Missing authorization header")?;

        if !auth_header.starts_with("Basic ") {
            return Err("Invalid authorization header format".into());
        }

        // Extract and decode credentials
        let (username, password) = self.extract_credentials(&auth_header)?;

        // Find user and verify password
        let password_hash = self.user_passwords.get(&username)
            .ok_or("Invalid username or password")?;

        if !bcrypt::verify(&password, password_hash).unwrap_or(false) {
            return Err("Invalid username or password".into());
        }

        Ok(username)
    }

    fn extract_credentials(&self, authorization: &str) -> BasicAuthResult<(String, String)> {
        // Remove "Basic " prefix
        let encoded = authorization.strip_prefix("Basic ")
            .ok_or("Invalid authorization header format")?;

        // Decode base64
        let decoded = general_purpose::STANDARD.decode(encoded)
            .map_err(|_| "Failed to decode authentication header")?;

        let decoded_str = String::from_utf8(decoded)
            .map_err(|_| "Invalid UTF-8 in decoded authorization")?;

        // Split username:password
        let parts: Vec<&str> = decoded_str.split(':').collect();
        if parts.len() != 2 {
            return Err("Invalid decoded data format".into());
        }

        let username = parts[0].trim().to_string();
        let password = parts[1].trim().to_string();

        if username.is_empty() || password.is_empty() {
            return Err("Empty username or password".into());
        }

        Ok((username, password))
    }

    fn set_consumer_headers(
        &self,
        session: &mut dyn PluginSession,
        username: &str,
    ) -> BasicAuthResult<()> {
        // Set X-Consumer-Username header
        session.set_request_header("X-Consumer-Username", username)?;
        Ok(())
    }

    fn handle_anonymous_access(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.add_plugin_log(&format!("Allowing anonymous access as '{}'; ", anonymous));
            let _ = session.set_request_header("X-Consumer-Username", anonymous);
            let _ = session.set_request_header("X-Anonymous-Consumer", "true");
            return true;
        }
        false
    }

    async fn auth_failed_return(
        &self,
        session: &mut dyn PluginSession,
    ) -> BasicAuthResult<()> {
        let mut resp = ResponseHeader::build(401, None)?;

        // WWW-Authenticate header with configured realm
        let auth_header_value = format!("Basic realm=\"{}\"", self.config.realm);
        resp.insert_header("WWW-Authenticate", auth_header_value)?;
        resp.insert_header("Content-Type", "text/plain")?;
        resp.insert_header("Connection", "close")?;

        session
            .write_response_header(Box::new(resp), false)
            .await?;
        session
            .write_response_body(
                Some(Bytes::from_static(
                    b"401 Unauthorized - Authentication required",
                )),
                true,
            )
            .await?;
        session.shutdown().await;
        Ok(())
    }
}

#[async_trait]
impl Plugin for BasicAuth {
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

        // Hard-coded: Skip OPTIONS requests
        // CORS preflight is handled by CORS plugin
        if session.method() == "OPTIONS" {
            return PluginRunningResult::GoodNext;
        }

        // Try to authenticate
        let username = match self.authenticate_request(session) {
            Ok(user) => user,
            Err(e) => {
                plugin_log.add_plugin_log(&format!("Authentication failed: {}; ", e));

                // Check if anonymous access is allowed
                if self.handle_anonymous_access(session, plugin_log) {
                    // Hide credentials if configured
                    if self.config.hide_credentials {
                        let _ = session.remove_request_header("authorization");
                    }
                    return PluginRunningResult::GoodNext;
                }

                // No anonymous access, return 401
                let _ = self.auth_failed_return(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        plugin_log.add_plugin_log(&format!("Auth successful for user: {}; ", username));

        // Set consumer headers for upstream
        if let Err(e) = self.set_consumer_headers(session, &username) {
            plugin_log.add_plugin_log(&format!("Failed to set headers: {}; ", e));
        }

        // Hide credentials if configured
        if self.config.hide_credentials {
            if let Err(e) = session.remove_request_header("authorization") {
                plugin_log.add_plugin_log(&format!("Failed to remove auth header: {}; ", e));
            }
        }

        PluginRunningResult::GoodNext
    }

    fn get_stages(&self) -> Vec<PluginRunningStage> {
        self.stages.clone()
    }

    fn check_schema(&self, _conf: &PluginConf) {
        // Schema validation can be implemented here if needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::MockPluginSession;
    use base64::engine::general_purpose;

    fn create_basic_auth_with_users() -> BasicAuth {
        let config = BasicAuthConfig {
            secret_refs: None,
            realm: "Test Realm".to_string(),
            hide_credentials: false,
            anonymous: None,
        };
        let mut auth = BasicAuth::new(&config);
        
        let mut users = HashMap::new();
        users.insert("testuser".to_string(), "testpass".to_string());
        auth.load_users(users).unwrap();
        
        auth
    }

    fn encode_credentials(username: &str, password: &str) -> String {
        let credentials = format!("{}:{}", username, password);
        format!("Basic {}", general_purpose::STANDARD.encode(credentials.as_bytes()))
    }

    #[tokio::test]
    async fn test_successful_auth() {
        let auth = create_basic_auth_with_users();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");
        
        let auth_header = encode_credentials("testuser", "testpass");
        
        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .returning(move |_| Some(auth_header.clone()));
        mock_session
            .expect_set_request_header()
            .returning(|_, _| Ok(()));
        
        let result = auth.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;
        
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("Auth successful"));
    }

    #[tokio::test]
    async fn test_auth_failed_returns_401() {
        let auth = create_basic_auth_with_users();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");
        
        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session
            .expect_header_value()
            .returning(|_| None);
        mock_session
            .expect_write_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_body()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_shutdown()
            .returning(|| {});
        
        let result = auth.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;
        
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[tokio::test]
    async fn test_anonymous_access() {
        let config = BasicAuthConfig {
            secret_refs: None,
            realm: "Test".to_string(),
            hide_credentials: false,
            anonymous: Some("anonymous-user".to_string()),
        };
        let auth = BasicAuth::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");
        
        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_header_value().returning(|_| None);
        mock_session.expect_set_request_header().returning(|_, _| Ok(()));
        
        let result = auth.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;
        
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("anonymous"));
    }

    #[tokio::test]
    async fn test_hide_credentials() {
        let config = BasicAuthConfig {
            secret_refs: None,
            realm: "Test".to_string(),
            hide_credentials: true,
            anonymous: None,
        };
        let mut auth = BasicAuth::new(&config);
        let mut users = HashMap::new();
        users.insert("testuser".to_string(), "testpass".to_string());
        auth.load_users(users).unwrap();
        
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");
        let auth_header = encode_credentials("testuser", "testpass");
        
        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session
            .expect_header_value()
            .returning(move |_| Some(auth_header.clone()));
        mock_session.expect_set_request_header().returning(|_, _| Ok(()));
        mock_session
            .expect_remove_request_header()
            .with(mockall::predicate::eq("authorization"))
            .times(1)
            .returning(|_| Ok(()));
        
        let result = auth.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;
        
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_options_method_skip() {
        let auth = create_basic_auth_with_users();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");
        
        mock_session.expect_method().returning(|| "OPTIONS".to_string());
        
        let result = auth.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;
        
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}