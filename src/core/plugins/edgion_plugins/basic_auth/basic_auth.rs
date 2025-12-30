//! Basic Authentication plugin implementation

use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::BasicAuthConfig;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use dashmap::DashMap;

type BasicAuthError = Box<dyn std::error::Error + Send + Sync>;
type BasicAuthResult<T> = Result<T, BasicAuthError>;

/// Basic Authentication plugin
pub struct BasicAuth {
    name: String,
    // Simple username -> password_hash mapping
    // We store whatever is verified: either a bcrypt hash (generated locally)
    // or a pre-supplied hash (from htpasswd).
    user_passwords: Arc<HashMap<String, String>>,
    // Cache for successful authentications: AuthHeader -> (Username, Expiry)
    // We only cache VALID credentials to avoid memory DoS attacks with random invalid headers.
    auth_cache: Arc<DashMap<String, (String, Instant)>>,
    config: BasicAuthConfig,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        BasicAuth {
            name: "BasicAuth".to_string(),
            user_passwords: Arc::new(HashMap::new()),
            auth_cache: Arc::new(DashMap::new()),
            config: config.clone(),
        }
    }

    /// Load users from resolved Secret data
    ///
    /// Supports adaptive behavior:
    /// - If password looks like a hash (bcrypt, md5, sha1, etc.), store as-is.
    /// - If password looks like plaintext, hash it using Bcrypt (default security).
    pub fn load_users(&mut self, users: HashMap<String, String>) -> Result<(), String> {
        let mut new_map = HashMap::new();
        for (username, password) in users {
            if self.is_pre_hashed(&password) {
                // Store pre-hashed password directly
                new_map.insert(username, password);
            } else {
                // Hash plaintext password with bcrypt (cost 10) for storage
                let hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                    .map_err(|e| format!("Failed to hash password for {}: {}", username, e))?;
                new_map.insert(username, hash);
            }
        }
        self.user_passwords = Arc::new(new_map);
        // Clear cache on user reload to ensure revoked passwords take effect immediately
        self.auth_cache.clear();

        tracing::info!(
            "BasicAuth: Loaded {} users (mixed hashed/plaintext source)",
            self.user_passwords.len()
        );
        Ok(())
    }

    /// Check if a password string looks like a supported hash
    fn is_pre_hashed(&self, s: &str) -> bool {
        // Bcrypt
        if s.starts_with("$2a$") || s.starts_with("$2b$") || s.starts_with("$2y$") {
            return true;
        }
        // Apache MD5 ($apr1$)
        if s.starts_with("$apr1$") {
            return true;
        }
        // SHA1
        if s.starts_with("{SHA}") {
            return true;
        }
        // Crypt (Unix) / Shadow
        if s.starts_with("$1$") || s.starts_with("$5$") || s.starts_with("$6$") {
            return true;
        }
        false
    }

    async fn authenticate_request(&self, session: &mut dyn PluginSession) -> BasicAuthResult<String> {
        // Extract authorization header
        let auth_header_value = session
            .header_value("authorization")
            .ok_or("Missing authorization header")?;

        if !auth_header_value.starts_with("Basic ") {
            return Err("Invalid authorization header format".into());
        }

        // 1. Check Cache (Fast Path)
        // If we have verified this exact header recently, skip cost.
        if let Some(entry) = self.auth_cache.get(auth_header_value) {
            let (username, expiry) = entry.value();
            if Instant::now() < *expiry {
                return Ok(username.clone());
            } else {
                // Expired
                drop(entry); // unlock
                self.auth_cache.remove(auth_header_value);
            }
        }

        // 2. Slow Path: Full Verification
        // Extract and decode credentials
        let (username, password) = self.extract_credentials(auth_header_value)?;

        // Find user
        let stored_hash = self
            .user_passwords
            .get(&username)
            .ok_or("Invalid username or password")?
            .clone();

        // Verify password - OFF-LOADED TO BLOCKING THREAD
        // This prevents blocking the async runtime with expensive crypto operations (bcrypt/scrypt etc.)
        let password_clone = password.clone();

        let is_valid = tokio::task::spawn_blocking(move || {
            // 2.1 Try generic htpasswd verification (supports apr1, sha1, bcrypt, etc.)
            if htpasswd_verify::verify(&stored_hash, &password_clone).unwrap_or(false) {
                return true;
            }

            // 2.2 Fallback for pure bcrypt crate hashes that might differ slightly or if htpasswd lib fails
            if stored_hash.starts_with("$2") {
                return bcrypt::verify(&password_clone, &stored_hash).unwrap_or(false);
            }

            false
        })
        .await
        .map_err(|e| format!("Password verification task failed: {}", e))?;

        if !is_valid {
            return Err("Invalid username or password".into());
        }

        // 3. Cache Success
        // Cache this header for 5 minutes
        self.auth_cache.insert(
            auth_header_value.to_string(),
            (username.clone(), Instant::now() + Duration::from_secs(300)),
        );

        Ok(username)
    }

    fn extract_credentials(&self, authorization: &str) -> BasicAuthResult<(String, String)> {
        // Remove "Basic " prefix
        let encoded = authorization
            .strip_prefix("Basic ")
            .ok_or("Invalid authorization header format")?;

        // Decode base64
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| "Failed to decode authentication header")?;

        let decoded_str = String::from_utf8(decoded).map_err(|_| "Invalid UTF-8 in decoded authorization")?;

        // Split username:password
        let parts: Vec<&str> = decoded_str.splitn(2, ':').collect();
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

    fn set_consumer_headers(&self, session: &mut dyn PluginSession, username: &str) -> BasicAuthResult<()> {
        // Set X-Consumer-Username header
        session.set_request_header("X-Consumer-Username", username)?;
        Ok(())
    }

    fn handle_anonymous_access(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.add_plugin_log(&format!("Allowing anonymous access as '{}'; ", anonymous));
            let _ = session.set_request_header("X-Consumer-Username", anonymous);
            let _ = session.set_request_header("X-Anonymous-Consumer", "true");
            return true;
        }
        false
    }

    async fn auth_failed_return(&self, session: &mut dyn PluginSession) -> BasicAuthResult<()> {
        let mut resp = ResponseHeader::build(401, None)?;

        // WWW-Authenticate header with configured realm
        let auth_header_value = format!("Basic realm=\"{}\"", self.config.realm);
        resp.insert_header("WWW-Authenticate", auth_header_value)?;
        resp.insert_header("Content-Type", "text/plain")?;
        resp.insert_header("Connection", "close")?;

        session.write_response_header(Box::new(resp), false).await?;
        session
            .write_response_body(
                Some(Bytes::from_static(b"401 Unauthorized - Authentication required")),
                true,
            )
            .await?;
        session.shutdown().await;
        Ok(())
    }
}

#[async_trait]
impl RequestFilter for BasicAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Try to authenticate
        let username = match self.authenticate_request(session).await {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
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
    async fn test_successful_auth_with_cache() {
        let auth = create_basic_auth_with_users();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");

        let auth_header = encode_credentials("testuser", "testpass");

        mock_session.expect_method().returning(|| "GET".to_string());
        // Called once normally, then potentially cache hit logic checks header existence
        // Note: Mockall expectations consume calls.

        // Setup mock to allow multiple calls to header_value("authorization")
        // because cache check reads it, and slow path reads it again if miss.
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .returning(move |_| Some(auth_header.clone()));

        mock_session.expect_set_request_header().returning(|_, _| Ok(()));

        // First run: Cache Miss -> Populate
        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(auth.auth_cache.len() == 1);

        // Second run: Cache Hit
        // We reuse the same mock calls, just ensuring it doesn't crash/fail
        let result2 = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result2, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_auth_failed_returns_401() {
        let auth = create_basic_auth_with_users();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("BasicAuth");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_header_value().returning(|_| None);
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));
        mock_session.expect_shutdown().returning(|| {});

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;

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

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;

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

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
