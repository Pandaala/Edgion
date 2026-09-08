//! Key Authentication plugin implementation
//!
//! Validates requests using API keys from various sources (header, query, cookie, etc.).
//! Keys and metadata are loaded from Kubernetes Secrets.

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, strip_cookie, strip_query_param,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::{KeyAuthConfig, KeyMetadata};

static PLUGIN_NAME: &str = "KeyAuth";
const MISSING_KEY_BODY: &str = "Unauthorized - Missing API key";
const INVALID_KEY_BODY: &str = "Unauthorized - Invalid API key";

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
    key_sources: Vec<KeyGet>,
    hide_credentials: bool,
    auth_failure_delay_ms: u64,
    anonymous: Option<String>,
    realm: String,
    forward_anonymous_username: bool,
    /// SHA-256(key) → Metadata mapping. Keys are stored as hashes to avoid keeping
    /// plaintext API keys in process memory after initialization.
    keys: HashMap<[u8; 32], KeyMetadata>,
}

impl KeyAuth {
    fn from_config(config: &KeyAuthConfig) -> Self {
        let mut config = config.clone();
        let _ = config.validate();
        let keys = hash_keys(config.resolved_keys.take().unwrap_or_default());
        let forward_anonymous_username = config
            .upstream_header_fields
            .iter()
            .any(|field| field == "X-Consumer-Username");

        Self {
            key_sources: config.key_sources,
            hide_credentials: config.hide_credentials,
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            anonymous: config.anonymous,
            realm: config.realm,
            forward_anonymous_username,
            keys,
        }
    }

    /// Create a new KeyAuth plugin from configuration
    ///
    /// This method validates the config and initializes the key store.
    pub fn create(config: &KeyAuthConfig) -> Box<dyn RequestFilter> {
        let plugin = Self::from_config(config);

        tracing::debug!(
            resolved_keys_present = config.resolved_keys.is_some(),
            keys_count = plugin.keys.len(),
            "KeyAuth: Creating plugin with resolved keys"
        );

        Box::new(plugin)
    }

    /// Extract API key from request using configured key sources (tried in order)
    ///
    /// Returns the first non-empty value found from the configured sources.
    /// Supports header, query, cookie, and ctx KeyGet sources.
    fn extract_key(&self, session: &dyn PluginSession) -> Option<(String, &KeyGet)> {
        for source in &self.key_sources {
            match source {
                KeyGet::Header { .. } | KeyGet::Query { .. } | KeyGet::Cookie { .. } | KeyGet::Ctx { .. } => {
                    if let Some(value) = session.key_get(source) {
                        if !value.is_empty() {
                            return Some((value, source));
                        }
                    }
                }
                // Ignore unsupported sources (RemoteIp, Path, Method, RequestPath, etc.)
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
            let _ = set_sensitive_request_header(session, name, value);
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
            _ => {}
        }
    }

    /// Return 401 Unauthorized response
    async fn unauthorized(&self, session: &mut dyn PluginSession, body: &'static str) -> PluginRunningResult {
        let _ = send_gateway_auth_error_response(session, 401, "ApiKey", &self.realm, None, body).await;
        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for KeyAuth {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // 1. Extract key from request using configured sources
        let (key, used_source) = match self.extract_key(session) {
            Some(found) => found,
            None => {
                // No key provided - check anonymous access
                if let Some(ref anon_user) = self.anonymous {
                    plugin_log.push("Allow anon");

                    // Set anonymous consumer header if in whitelist
                    if self.forward_anonymous_username {
                        let _ = set_sensitive_request_header(session, "X-Consumer-Username", anon_user);
                    }
                    let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");

                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Deny no key");
                apply_auth_failure_delay(self.auth_failure_delay_ms).await;
                return self.unauthorized(session, MISSING_KEY_BODY).await;
            }
        };

        // 2. Validate key
        let metadata = match self.validate_key(&key) {
            Some(m) => m,
            None => {
                plugin_log.push("Deny bad key");
                apply_auth_failure_delay(self.auth_failure_delay_ms).await;
                return self.unauthorized(session, INVALID_KEY_BODY).await;
            }
        };

        // Log key source (header / query / cookie / etc.)
        let key_source_label = match used_source {
            KeyGet::Header { .. } => "key header",
            KeyGet::Query { .. } => "key query",
            KeyGet::Cookie { .. } => "key cookie",
            KeyGet::Ctx { .. } => "key ctx",
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
        if self.hide_credentials {
            self.hide_credentials(session, used_source);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use bytes::Bytes;
    use edgion_resources::sensitive::SENSITIVE_MODE;
    use mockall::Sequence;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
        let mut config = create_test_config();
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

        config.resolved_keys = Some(keys);
        KeyAuth::from_config(&config)
    }

    fn expect_auth_response(mock: &mut MockPluginSession, expected_body: &'static [u8]) {
        let mut sequence = Sequence::new();
        mock.expect_write_response_header()
            .withf(|response, end_of_stream| {
                !end_of_stream
                    && response.status.as_u16() == 401
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("text/plain")
                    && response
                        .headers
                        .get("www-authenticate")
                        .and_then(|value| value.to_str().ok())
                        == Some("ApiKey realm=\"Test API\"")
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        mock.expect_write_response_body()
            .withf(move |body, end_of_stream| {
                *end_of_stream && body.as_deref() == Some(Bytes::from_static(expected_body).as_ref())
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        mock.expect_shutdown()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|| {});
    }

    #[test]
    fn name_is_static() {
        let auth = KeyAuth::from_config(&create_test_config());

        assert_eq!(auth.name(), PLUGIN_NAME);
        assert_eq!(auth.name().as_ptr(), PLUGIN_NAME.as_ptr());
    }

    #[test]
    fn constructor_leaves_source_config_unchanged_and_keeps_only_runtime_state() {
        let mut config = create_test_config();
        config.hide_credentials = true;
        config.auth_failure_delay_ms = 23;
        config.anonymous = Some("guest".to_string());
        config.key_field = "controller-key-field".to_string();
        config.resolved_keys = Some(HashMap::from([(
            "raw-runtime-key".to_string(),
            metadata_with_group("partner-a", Some("alice")),
        )]));
        let before = serde_json::to_value(&config).unwrap();

        let auth = KeyAuth::from_config(&config);

        assert_eq!(serde_json::to_value(&config).unwrap(), before);
        assert!(config
            .resolved_keys
            .as_ref()
            .is_some_and(|keys| keys.contains_key("raw-runtime-key")));
        assert_eq!(auth.key_sources, config.key_sources);
        assert!(auth.hide_credentials);
        assert_eq!(auth.auth_failure_delay_ms, 23);
        assert_eq!(auth.anonymous.as_deref(), Some("guest"));
        assert_eq!(auth.realm, "Test API");
        assert!(auth.forward_anonymous_username);
        assert_eq!(auth.keys.len(), 1);
        assert!(auth.keys.contains_key(&sha256_of("raw-runtime-key")));
        assert!(auth.validate_key("raw-runtime-key").is_some());
    }

    #[test]
    fn generations_keep_independent_hashed_key_maps() {
        let mut first = create_test_config();
        first.resolved_keys = Some(HashMap::from([(
            "generation-k1".to_string(),
            metadata_with_group("one", None),
        )]));
        let mut second = create_test_config();
        second.resolved_keys = Some(HashMap::from([(
            "generation-k2".to_string(),
            metadata_with_group("two", None),
        )]));

        let k1 = KeyAuth::from_config(&first);
        let k2 = KeyAuth::from_config(&second);

        assert!(k1.validate_key("generation-k1").is_some());
        assert!(k1.validate_key("generation-k2").is_none());
        assert!(k2.validate_key("generation-k1").is_none());
        assert!(k2.validate_key("generation-k2").is_some());
    }

    #[test]
    fn test_extract_key_none() {
        let auth = create_key_auth_with_keys();
        let mut mock_session = MockPluginSession::new();

        mock_session.expect_key_get().returning(|_| None);

        let result = auth.extract_key(&mock_session);
        assert!(result.is_none());
    }

    #[test]
    fn extraction_returns_the_configured_source_by_reference() {
        let auth = create_key_auth_with_keys();
        let query_reads = Arc::new(AtomicUsize::new(0));
        let query_reads_for_mock = Arc::clone(&query_reads);
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(move |source| match source {
            KeyGet::Header { .. } => Some("header-present".to_string()),
            KeyGet::Query { .. } => {
                query_reads_for_mock.fetch_add(1, Ordering::SeqCst);
                Some("query-value".to_string())
            }
            _ => None,
        });

        let (key, source) = auth.extract_key(&session).expect("header key should win");
        assert_eq!(key, "header-present");
        assert!(std::ptr::eq(source, &auth.key_sources[0]));
        assert_eq!(query_reads.load(Ordering::SeqCst), 0);
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

    #[tokio::test]
    async fn anonymous_path_uses_precomputed_username_forwarding_policy() {
        let mut config = create_test_config();
        config.anonymous = Some("guest".to_string());
        let auth = KeyAuth::from_config(&config);
        assert!(auth.forward_anonymous_username);

        let mut sequence = Sequence::new();
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(|_| None);
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Consumer-Username" && value == "guest" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Anonymous-Consumer" && value == "true" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("Allow anon"));

        config.upstream_header_fields.clear();
        assert!(!KeyAuth::from_config(&config).forward_anonymous_username);
    }

    #[tokio::test]
    async fn valid_key_forwards_metadata_then_hides_credential_and_masks_username() {
        const CHILD_ENV: &str = "EDGION_KEY_AUTH_MASKED_LOG_CHILD";
        const TEST_NAME: &str = "valid_key_forwards_metadata_then_hides_credential_and_masks_username";
        const TEST_MODULE: &str = "plugins::http::key_auth::plugin::tests";

        if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
            let exact_test_name = format!("{TEST_MODULE}::{TEST_NAME}");
            let output = std::process::Command::new(
                std::env::current_exe().expect("the Rust test harness must have a current executable"),
            )
            .args(["--exact", exact_test_name.as_str(), "--nocapture"])
            .env(CHILD_ENV, TEST_NAME)
            .output()
            .expect("the isolated KeyAuth masked-log test process must start");
            assert!(
                output.status.success(),
                "isolated KeyAuth masked-log test failed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        // This child runs only the exact test selected above, so changing the
        // process-global mode cannot race another lib test.
        SENSITIVE_MODE.store(1, Ordering::SeqCst);

        let mut config = create_test_config();
        config.hide_credentials = true;
        config.resolved_keys = Some(HashMap::from([(
            "valid-api-key".to_string(),
            metadata_with_group("partner-a", Some("private-user")),
        )]));
        let auth = KeyAuth::from_config(&config);
        let mut sequence = Sequence::new();
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(|source| match source {
            KeyGet::Header { .. } => Some("valid-api-key".to_string()),
            _ => None,
        });
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Consumer-Username" && value == "private-user" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|name| name == "X-API-Key")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("key header"));
        assert!(log.contains("OK group=partner-a user ***"));
        let serialized = serde_json::to_string(&log).unwrap();
        assert!(!serialized.contains("valid-api-key"));
        assert!(!serialized.contains("private-user"));
    }

    #[tokio::test]
    async fn missing_key_returns_exact_response() {
        const BODY: &[u8] = b"401 Unauthorized - Missing API key";

        let auth = KeyAuth::from_config(&create_test_config());
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(|_| None);
        expect_auth_response(&mut session, BODY);
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(log.contains("Deny no key"));
    }

    #[tokio::test]
    async fn present_invalid_key_does_not_use_fallback_or_anonymous_access() {
        const BODY: &[u8] = b"401 Unauthorized - Invalid API key";

        let mut config = create_test_config();
        config.anonymous = Some("guest".to_string());
        config.resolved_keys = Some(HashMap::from([(
            "query-valid".to_string(),
            metadata_with_group("partner-a", None),
        )]));
        let auth = KeyAuth::from_config(&config);
        let query_reads = Arc::new(AtomicUsize::new(0));
        let query_reads_for_mock = Arc::clone(&query_reads);
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(move |source| match source {
            KeyGet::Header { .. } => Some("header-invalid".to_string()),
            KeyGet::Query { .. } => {
                query_reads_for_mock.fetch_add(1, Ordering::SeqCst);
                Some("query-valid".to_string())
            }
            _ => None,
        });
        expect_auth_response(&mut session, BODY);
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(query_reads.load(Ordering::SeqCst), 0);
        assert!(log.contains("Deny bad key"));
        let value: Value = serde_json::to_value(&log).unwrap();
        assert!(!value.to_string().contains("header-invalid"));
    }
}
