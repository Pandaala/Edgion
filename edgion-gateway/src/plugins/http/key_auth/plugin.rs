//! Key Authentication plugin implementation
//!
//! Validates requests using API keys from various sources (header, query, cookie, etc.).
//! Keys and metadata are loaded from Kubernetes Secrets.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, strip_cookie, strip_query_param,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::{KeyAuthConfig, KeyMetadata};

fn sha256_of(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

fn hash_keys(raw: HashMap<String, KeyMetadata>) -> HashMap<[u8; 32], KeyMetadata> {
    raw.into_iter().map(|(k, v)| (sha256_of(&k), v)).collect()
}

/// Key Authentication plugin
///
/// Authenticates requests by validating API keys against a pre-configured key store.
/// Supports key extraction from multiple sources via `KeyGet`.
pub struct KeyAuth {
    name: String,
    config: KeyAuthConfig,
    /// SHA-256(key) → Metadata mapping. Keys are stored as hashes to avoid keeping
    /// plaintext API keys in process memory after initialization.
    keys: Arc<HashMap<[u8; 32], KeyMetadata>>,
}

impl KeyAuth {
    /// Create a new KeyAuth plugin from configuration
    ///
    /// This method validates the config and initializes the key store.
    pub fn create(config: &KeyAuthConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate();

        let keys = hash_keys(validated_config.resolved_keys.take().unwrap_or_default());

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

    #[cfg(test)]
    pub fn new(config: &KeyAuthConfig) -> Self {
        let keys = hash_keys(config.resolved_keys.clone().unwrap_or_default());
        Self {
            name: "KeyAuth".to_string(),
            config: config.clone(),
            keys: Arc::new(keys),
        }
    }

    #[cfg(test)]
    pub fn load_keys(&mut self, keys: HashMap<String, KeyMetadata>) {
        self.keys = Arc::new(hash_keys(keys));
    }

    /// Extract API key from request using configured key sources (tried in order)
    ///
    /// Returns the first non-empty value found from the configured sources.
    /// Supports all KeyGet variants including Webhook for remote key resolution.
    async fn extract_key(&self, session: &dyn PluginSession) -> Option<(String, &KeyGet)> {
        for source in &self.config.key_sources {
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
                _ => {}
            }
        }
        None
    }

    fn validate_key(&self, key: &str) -> Option<&KeyMetadata> {
        self.keys.get(&sha256_of(key))
    }

    /// Set upstream headers from key metadata (whitelist controlled)
    fn set_upstream_headers(&self, session: &mut dyn PluginSession, metadata: &KeyMetadata) {
        for (name, value) in &metadata.headers {
            if name.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
            if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
                continue;
            }
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
            KeyGet::Query { name } => {
                strip_query_param(session, name);
            }
            KeyGet::Cookie { name } => {
                strip_cookie(session, name);
            }
            // Webhook keys are resolved server-side and never travel in the
            // forwarded request, so there is nothing to strip.
            _ => {}
        }
    }

    /// Return 401 Unauthorized response
    async fn unauthorized(&self, session: &mut dyn PluginSession, message: &str) -> PluginRunningResult {
        let body = format!("Unauthorized - {}", message);
        let _ = send_gateway_auth_error_response(session, 401, "ApiKey", &self.config.realm, None, &body).await;
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
                    plugin_log.push("Allow anon");

                    // Set anonymous consumer header if in whitelist
                    if self
                        .config
                        .upstream_header_fields
                        .iter()
                        .any(|f| f == "X-Consumer-Username")
                    {
                        let _ = session.set_request_header("X-Consumer-Username", anon_user);
                    }
                    let _ = session.set_request_header("X-Anonymous-Consumer", "true");

                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Deny no key");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session, "Missing API key").await;
            }
        };

        // 2. Validate key
        let metadata = match self.validate_key(&key) {
            Some(m) => m,
            None => {
                plugin_log.push("Deny bad key");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session, "Invalid API key").await;
            }
        };

        // Log key source (header / query / cookie / etc.)
        let key_source_label = match &used_source {
            KeyGet::Header { .. } => "key header",
            KeyGet::Query { .. } => "key query",
            KeyGet::Cookie { .. } => "key cookie",
            KeyGet::Ctx { .. } => "key ctx",
            KeyGet::Webhook { .. } => "key webhook",
            _ => "key other",
        };
        plugin_log.push(key_source_label);

        // Log OK with group attribution + optional username
        if let Some(username) = metadata.headers.get("X-Consumer-Username") {
            plugin_log.push_fmt("OK group={} user {}", la![metadata.group.as_ref(), sensitive(username)]);
        } else {
            plugin_log.push_fmt("OK group={}", la![metadata.group.as_ref()]);
        }

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
    use crate::plugins::runtime::traits::session::MockPluginSession;

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
            auth_failure_delay_ms: 0,
            anonymous: None,
            realm: "Test API".to_string(),
            key_field: "key".to_string(),
            secret_groups: None,
            upstream_header_fields: vec!["X-Consumer-Username".to_string(), "X-Customer-ID".to_string()],
            resolved_keys: None,
        }
    }

    fn create_key_auth_with_keys() -> KeyAuth {
        let config = create_test_config();
        let mut auth = KeyAuth::new(&config);

        let mut keys = HashMap::new();
        let group: String = "partner-a".to_string();

        // Key 1: jack
        let mut jack_headers = HashMap::new();
        jack_headers.insert("X-Consumer-Username".to_string(), "jack".to_string());
        jack_headers.insert("X-Customer-ID".to_string(), "cust-001".to_string());
        keys.insert(
            "jack-key-12345".to_string(),
            KeyMetadata {
                headers: jack_headers,
                group: group.clone(),
            },
        );

        // Key 2: alice
        let mut alice_headers = HashMap::new();
        alice_headers.insert("X-Consumer-Username".to_string(), "alice".to_string());
        alice_headers.insert("X-Customer-ID".to_string(), "cust-002".to_string());
        keys.insert(
            "alice-key-67890".to_string(),
            KeyMetadata {
                headers: alice_headers,
                group: group.clone(),
            },
        );

        auth.load_keys(keys);
        auth
    }

    #[tokio::test]
    async fn test_extract_key_none() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        mock_session.expect_key_get().returning(|_| None);

        let result = auth.extract_key(&mock_session).await;
        assert!(result.is_none());
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

    use crate::plugins::runtime::PluginLog;
    use serde_json::Value;

    fn metadata_with_group(group: &str, username: Option<&str>) -> KeyMetadata {
        let mut headers = HashMap::new();
        if let Some(u) = username {
            headers.insert("X-Consumer-Username".to_string(), u.to_string());
        }
        KeyMetadata {
            headers,
            group: group.to_string(),
        }
    }

    fn render_log(log: &PluginLog) -> Vec<String> {
        let v: Value = serde_json::to_value(log).unwrap();
        v.get("log")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn hit_log_contains_group_and_user() {
        let mut log = PluginLog::new("KeyAuth");
        let meta = metadata_with_group("partner-a", Some("jack"));
        if let Some(user) = meta.headers.get("X-Consumer-Username") {
            log.push_fmt("OK group={} user {}", la![meta.group.as_ref(), sensitive(user)]);
        }
        let entries = render_log(&log);
        assert!(
            entries
                .iter()
                .any(|e| e.starts_with("OK group=partner-a") && e.contains("user ")),
            "expected 'OK group=partner-a user ...', got entries: {:?}",
            entries
        );
    }

    #[test]
    fn hit_log_without_user_still_has_group() {
        let mut log = PluginLog::new("KeyAuth");
        let meta = metadata_with_group("internal", None);
        if meta.headers.get("X-Consumer-Username").is_none() {
            log.push_fmt("OK group={}", la![meta.group.as_ref()]);
        }
        let entries = render_log(&log);
        assert!(
            entries.iter().any(|e| e == "OK group=internal"),
            "expected exact 'OK group=internal', got: {:?}",
            entries
        );
    }
}
