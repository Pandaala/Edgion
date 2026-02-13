//! Key Authentication plugin implementation
//!
//! Validates requests using API keys from various sources (header, query, cookie, etc.).
//! Keys and metadata are loaded from Kubernetes Secrets.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::plugins::edgion_plugins::common::auth_common::send_auth_error_response;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::common::KeyGet;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::plugin_configs::{KeyAuthConfig, KeyMetadata};

/// Key Authentication plugin
///
/// Authenticates requests by validating API keys against a pre-configured key store.
/// Supports key extraction from multiple sources via `KeyGet`.
pub struct KeyAuth {
    name: String,
    config: KeyAuthConfig,
    /// Key -> Metadata mapping for O(1) lookup
    keys: Arc<HashMap<String, KeyMetadata>>,
}

impl KeyAuth {
    /// Create a new KeyAuth plugin from configuration
    ///
    /// This method validates the config and initializes the key store.
    pub fn create(config: &KeyAuthConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        validated_config.validate();

        let keys = validated_config.resolved_keys.clone().unwrap_or_default();

        tracing::debug!(
            resolved_keys_present = config.resolved_keys.is_some(),
            keys_count = keys.len(),
            "KeyAuth: Creating plugin with resolved keys"
        );

        let plugin = Self {
            name: "KeyAuth".to_string(),
            config: validated_config,
            keys: Arc::new(keys),
        };

        Box::new(plugin)
    }

    /// Create a new KeyAuth plugin from configuration (for testing without Box)
    pub fn new(config: &KeyAuthConfig) -> Self {
        let keys = config.resolved_keys.clone().unwrap_or_default();
        Self {
            name: "KeyAuth".to_string(),
            config: config.clone(),
            keys: Arc::new(keys),
        }
    }

    /// Load keys directly (for testing or manual initialization)
    pub fn load_keys(&mut self, keys: HashMap<String, KeyMetadata>) {
        self.keys = Arc::new(keys);
    }

    /// Extract API key from request using configured key sources (tried in order)
    ///
    /// Returns the first non-empty value found from the configured sources.
    /// Supports all KeyGet variants including Webhook for remote key resolution.
    async fn extract_key(&self, session: &dyn PluginSession) -> Option<(String, &KeyGet)> {
        for source in &self.config.key_sources {
            // Allow supported key sources for API key extraction
            match source {
                KeyGet::Header { .. }
                | KeyGet::Query { .. }
                | KeyGet::Cookie { .. }
                | KeyGet::Ctx { .. }
                | KeyGet::Webhook { .. } => {
                    if let Some(value) = session.key_get(source).await {
                        if !value.is_empty() {
                            return Some((value, source));
                        }
                    }
                }
                // Ignore unsupported sources (ClientIp, Path, Method, etc.)
                _ => {
                    tracing::warn!(
                        source = source.source_type(),
                        "KeyAuth: Unsupported key source type, skipping"
                    );
                }
            }
        }
        None
    }

    /// Validate key and return metadata if valid
    fn validate_key(&self, key: &str) -> Option<&KeyMetadata> {
        self.keys.get(key)
    }

    /// Set upstream headers from key metadata (whitelist controlled)
    fn set_upstream_headers(&self, session: &mut dyn PluginSession, metadata: &KeyMetadata) {
        for (name, value) in &metadata.headers {
            // Use set (not append) to prevent client header forgery
            let _ = session.set_request_header(name, value);
        }
    }

    /// Hide credentials from upstream request based on the key source used
    fn hide_credentials(&self, session: &mut dyn PluginSession, used_source: &KeyGet) {
        match used_source {
            KeyGet::Header { name } => {
                let _ = session.remove_request_header(name);
            }
            KeyGet::Ctx { name } => {
                let _ = session.remove_ctx_var(name);
            }
            // Query and Cookie removal would require URI/Cookie header manipulation
            // which is complex and not commonly needed
            KeyGet::Query { name } => {
                tracing::debug!(
                    query_param = name,
                    "KeyAuth: Query parameter hiding not implemented, consider using header instead"
                );
            }
            KeyGet::Cookie { name } => {
                tracing::debug!(
                    cookie = name,
                    "KeyAuth: Cookie hiding not implemented, consider using header instead"
                );
            }
            _ => {}
        }
    }

    /// Return 401 Unauthorized response
    async fn unauthorized(&self, session: &mut dyn PluginSession, message: &str) -> PluginRunningResult {
        let body = format!("Unauthorized - {}", message);
        let _ = send_auth_error_response(session, 401, "ApiKey", &self.config.realm, &body).await;
        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for KeyAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // 1. Extract key from request using configured sources
        let (key, used_source) = match self.extract_key(session).await {
            Some((k, s)) => (k, s.clone()),
            None => {
                // No key provided - check anonymous access
                if let Some(ref anon_user) = self.config.anonymous {
                    plugin_log.push("Anonymous; ");

                    // Set anonymous consumer header if in whitelist
                    if self
                        .config
                        .upstream_header_fields
                        .contains(&"X-Consumer-Username".to_string())
                    {
                        let _ = session.set_request_header("X-Consumer-Username", anon_user);
                    }
                    let _ = session.set_request_header("X-Anonymous-Consumer", "true");

                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("No key; ");
                return self.unauthorized(session, "Missing API key").await;
            }
        };

        // 2. Validate key
        let metadata = match self.validate_key(&key) {
            Some(m) => m,
            None => {
                plugin_log.push("Invalid key; ");
                return self.unauthorized(session, "Invalid API key").await;
            }
        };

        plugin_log.push(&format!("Auth OK ({}); ", used_source.as_log_str()));

        // 3. Set upstream headers (from whitelist)
        self.set_upstream_headers(session, metadata);

        // 4. Hide credentials if configured
        if self.config.hide_credentials {
            self.hide_credentials(session, &used_source);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

    fn create_test_config() -> KeyAuthConfig {
        KeyAuthConfig {
            key_sources: vec![
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
                KeyGet::Query {
                    name: "api_key".to_string(),
                },
            ],
            hide_credentials: false,
            anonymous: None,
            realm: "Test API".to_string(),
            key_field: "key".to_string(),
            secret_refs: None,
            upstream_header_fields: vec!["X-Consumer-Username".to_string(), "X-Customer-ID".to_string()],
            resolved_keys: None,
            validation_error: None,
        }
    }

    fn create_key_auth_with_keys() -> KeyAuth {
        let config = create_test_config();
        let mut auth = KeyAuth::new(&config);

        let mut keys = HashMap::new();

        // Key 1: jack
        let mut jack_headers = HashMap::new();
        jack_headers.insert("X-Consumer-Username".to_string(), "jack".to_string());
        jack_headers.insert("X-Customer-ID".to_string(), "cust-001".to_string());
        keys.insert("jack-key-12345".to_string(), KeyMetadata { headers: jack_headers });

        // Key 2: alice
        let mut alice_headers = HashMap::new();
        alice_headers.insert("X-Consumer-Username".to_string(), "alice".to_string());
        alice_headers.insert("X-Customer-ID".to_string(), "cust-002".to_string());
        keys.insert("alice-key-67890".to_string(), KeyMetadata { headers: alice_headers });

        auth.load_keys(keys);
        auth
    }

    #[tokio::test]
    async fn test_extract_key_from_header() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        // key_get is called with KeyGet::Header
        mock_session.expect_key_get().returning(|key| {
            if let KeyGet::Header { name } = key {
                if name == "X-API-Key" {
                    return Some("jack-key-12345".to_string());
                }
            }
            None
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, source) = result.unwrap();
        assert_eq!(key, "jack-key-12345");
        assert!(matches!(source, KeyGet::Header { name } if name == "X-API-Key"));
    }

    #[tokio::test]
    async fn test_extract_key_from_query() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        // Header returns None, Query returns value
        mock_session.expect_key_get().returning(|key| match key {
            KeyGet::Header { .. } => None,
            KeyGet::Query { name } if name == "api_key" => Some("alice-key-67890".to_string()),
            _ => None,
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, source) = result.unwrap();
        assert_eq!(key, "alice-key-67890");
        assert!(matches!(source, KeyGet::Query { name } if name == "api_key"));
    }

    #[tokio::test]
    async fn test_header_priority_over_query() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        // Header returns value first
        mock_session.expect_key_get().returning(|key| match key {
            KeyGet::Header { name } if name == "X-API-Key" => Some("header-key".to_string()),
            KeyGet::Query { name } if name == "api_key" => Some("query-key".to_string()),
            _ => None,
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, source) = result.unwrap();
        assert_eq!(key, "header-key");
        assert!(matches!(source, KeyGet::Header { .. }));
    }

    #[tokio::test]
    async fn test_extract_key_none() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        mock_session.expect_key_get().returning(|_| None);

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_extract_key_from_cookie() {
        let config = KeyAuthConfig {
            key_sources: vec![
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
                KeyGet::Cookie {
                    name: "api_token".to_string(),
                },
            ],
            ..Default::default()
        };
        let auth = KeyAuth::new(&config);
        let mut mock_session = MockPluginSession::new();

        mock_session.expect_key_get().returning(|key| match key {
            KeyGet::Header { .. } => None,
            KeyGet::Cookie { name } if name == "api_token" => Some("cookie-key-123".to_string()),
            _ => None,
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, source) = result.unwrap();
        assert_eq!(key, "cookie-key-123");
        assert!(matches!(source, KeyGet::Cookie { name } if name == "api_token"));
    }

    #[test]
    fn test_validate_key_valid() {
        let auth = create_key_auth_with_keys();

        let metadata = auth.validate_key("jack-key-12345");
        assert!(metadata.is_some());

        let meta = metadata.unwrap();
        assert_eq!(meta.headers.get("X-Consumer-Username"), Some(&"jack".to_string()));
        assert_eq!(meta.headers.get("X-Customer-ID"), Some(&"cust-001".to_string()));
    }

    #[test]
    fn test_validate_key_invalid() {
        let auth = create_key_auth_with_keys();

        let metadata = auth.validate_key("invalid-key");
        assert!(metadata.is_none());
    }

    #[tokio::test]
    async fn test_successful_auth() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("KeyAuth");

        mock_session.expect_method().returning(|| "GET".to_string());

        mock_session.expect_key_get().returning(|key| {
            if let KeyGet::Header { name } = key {
                if name == "X-API-Key" {
                    return Some("jack-key-12345".to_string());
                }
            }
            None
        });

        mock_session.expect_set_request_header().returning(|_, _| Ok(()));

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Auth OK"));
    }

    #[tokio::test]
    async fn test_missing_key_returns_401() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("KeyAuth");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_key_get().returning(|_| None);
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));
        mock_session.expect_shutdown().returning(|| {});

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("No key"));
    }

    #[tokio::test]
    async fn test_invalid_key_returns_401() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("KeyAuth");

        mock_session.expect_method().returning(|| "GET".to_string());

        mock_session.expect_key_get().returning(|key| {
            if let KeyGet::Header { name } = key {
                if name == "X-API-Key" {
                    return Some("invalid-key".to_string());
                }
            }
            None
        });

        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));
        mock_session.expect_shutdown().returning(|| {});

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Invalid key"));
    }

    #[tokio::test]
    async fn test_anonymous_access() {
        let mut config = create_test_config();
        config.anonymous = Some("anonymous-user".to_string());

        let auth = KeyAuth::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("KeyAuth");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_key_get().returning(|_| None);
        mock_session.expect_set_request_header().returning(|_, _| Ok(()));

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Anonymous"));
    }

    #[tokio::test]
    async fn test_hide_credentials_header() {
        let mut config = create_test_config();
        config.hide_credentials = true;

        let mut auth = KeyAuth::new(&config);

        let mut keys = HashMap::new();
        keys.insert("test-key".to_string(), KeyMetadata::default());
        auth.load_keys(keys);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("KeyAuth");

        mock_session.expect_method().returning(|| "GET".to_string());

        mock_session.expect_key_get().returning(|key| {
            if let KeyGet::Header { name } = key {
                if name == "X-API-Key" {
                    return Some("test-key".to_string());
                }
            }
            None
        });

        mock_session.expect_set_request_header().returning(|_, _| Ok(()));

        mock_session
            .expect_remove_request_header()
            .with(mockall::predicate::eq("X-API-Key"))
            .times(1)
            .returning(|_| Ok(()));

        let result = auth.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[test]
    fn test_upstream_headers_whitelist() {
        let auth = create_key_auth_with_keys();

        // Jack has headers in metadata
        let metadata = auth.validate_key("jack-key-12345").unwrap();

        // Only whitelisted headers should be in metadata
        assert!(metadata.headers.contains_key("X-Consumer-Username"));
        assert!(metadata.headers.contains_key("X-Customer-ID"));
    }

    #[tokio::test]
    async fn test_empty_value_fallback_to_next_source() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        // Header returns empty string, should fallback to query
        mock_session.expect_key_get().returning(|key| match key {
            KeyGet::Header { name } if name == "X-API-Key" => Some("".to_string()),
            KeyGet::Query { name } if name == "api_key" => Some("alice-key-67890".to_string()),
            _ => None,
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, source) = result.unwrap();
        assert_eq!(key, "alice-key-67890");
        assert!(matches!(source, KeyGet::Query { .. }));
    }

    #[tokio::test]
    async fn test_unsupported_source_skipped() {
        // Create config with unsupported source (ClientIp)
        let config = KeyAuthConfig {
            key_sources: vec![
                KeyGet::ClientIp, // Should be skipped
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
            ],
            ..Default::default()
        };
        let auth = KeyAuth::new(&config);
        let mut mock_session = MockPluginSession::new();

        mock_session.expect_key_get().returning(|key| {
            if let KeyGet::Header { name } = key {
                if name == "X-API-Key" {
                    return Some("valid-key".to_string());
                }
            }
            None
        });

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_some());
        let (key, _) = result.unwrap();
        assert_eq!(key, "valid-key");
    }
}
