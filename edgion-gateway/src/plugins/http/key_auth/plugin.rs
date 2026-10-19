//! Key Authentication plugin implementation
//!
//! Validates requests using API keys from various sources (header, query, cookie, etc.).
//! Keys and metadata are loaded from Kubernetes Secrets.

use crate::plugins::http::common::auth_common::{prepare_sensitive_headers, set_sensitive_request_header};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, strip_cookie, strip_query_param,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::session_adapter::key_value;
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

struct PreparedKeyMetadata {
    group: String,
    username: Option<String>,
    headers: Vec<(String, http::HeaderValue)>,
}

impl PreparedKeyMetadata {
    fn new(metadata: KeyMetadata) -> Self {
        // Logging retains the exact-case username even when forwarding skips it.
        let username = metadata.headers.get("X-Consumer-Username").cloned();
        let headers = prepare_sensitive_headers(metadata.headers);
        Self {
            group: metadata.group,
            username,
            headers,
        }
    }
}

fn hash_keys(raw: HashMap<String, KeyMetadata>) -> HashMap<[u8; 32], PreparedKeyMetadata> {
    raw.into_iter()
        .map(|(k, v)| (sha256_of(&k), PreparedKeyMetadata::new(v)))
        .collect()
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
    keys: HashMap<[u8; 32], PreparedKeyMetadata>,
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
    fn extract_key<'s>(&self, session: &'s dyn PluginSession) -> Option<(Cow<'s, str>, &KeyGet)> {
        for source in &self.key_sources {
            match source {
                KeyGet::Header { .. } | KeyGet::Query { .. } | KeyGet::Cookie { .. } | KeyGet::Ctx { .. } => {
                    if let Some(value) = key_value(session, source) {
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

    fn validate_key(&self, key: &str) -> Option<&PreparedKeyMetadata> {
        self.keys.get(&sha256_of(key))
    }

    /// Set upstream headers from key metadata (whitelist controlled)
    fn set_upstream_headers(&self, session: &mut dyn PluginSession, metadata: &PreparedKeyMetadata) {
        for (name, value) in &metadata.headers {
            // Keep the live setter's replacement and deferred-mutation semantics.
            let _ = session.set_request_header_value(name, value.clone());
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
        let Some((key, used_source)) = self.extract_key(session) else {
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
        };

        // 2. Validate key
        let Some(metadata) = self.validate_key(&key) else {
            plugin_log.push("Deny bad key");
            apply_auth_failure_delay(self.auth_failure_delay_ms).await;
            return self.unauthorized(session, INVALID_KEY_BODY).await;
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
        if let Some(username) = &metadata.username {
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
#[path = "../../../../tests/unit/plugins/http/key_auth/plugin_test.rs"]
mod tests;
