//! Key Authentication plugin implementation
//!
//! Validates requests using API keys from various sources (header, query, cookie, etc.).
//! Keys and metadata are loaded from Kubernetes Secrets.

use crate::plugins::http::common::auth_common::{
    prepare_sensitive_headers, remove_request_headers, set_sensitive_request_header,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, cookie_cleanup_failure, send_gateway_auth_error_response, strip_cookie,
    strip_query_param, PreparedAuthChallenge, CREDENTIAL_REMOVAL_FAILED, IDENTITY_HEADER_REJECTED,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::session_adapter::key_value;
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionResult, RequestFilter};
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

use crate::plugins::http::common::auth_common::HideCredentialError;

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
    auth_challenge: PreparedAuthChallenge,
    forward_anonymous_username: bool,
    /// Whitelisted metadata names (`upstreamHeaderFields`). The plugin owns
    /// them: every continuation clears them before writing, so a client copy
    /// never survives under a name the consumer lacks.
    owned_header_names: Vec<String>,
    /// SHA-256(key) → Metadata mapping. Keys are stored as hashes to avoid keeping
    /// plaintext API keys in process memory after initialization.
    keys: HashMap<[u8; 32], PreparedKeyMetadata>,
}

impl KeyAuth {
    fn from_config(config: &KeyAuthConfig) -> Self {
        let mut config = config.clone();
        let _ = config.validate();
        let keys = hash_keys(config.resolved_keys.take().unwrap_or_default());
        let forward_anonymous_username = config.forwards_anonymous_username();

        Self {
            key_sources: config.key_sources,
            hide_credentials: config.hide_credentials,
            auth_failure_delay_ms: config.auth_failure_delay_ms,
            anonymous: config.anonymous,
            auth_challenge: PreparedAuthChallenge::new("ApiKey", &config.realm),
            forward_anonymous_username,
            owned_header_names: config.upstream_header_fields,
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

    /// Set upstream headers from key metadata (whitelist controlled).
    /// The first failed write stops the projection; the caller denies.
    fn set_upstream_headers(
        &self,
        session: &mut dyn PluginSession,
        metadata: &PreparedKeyMetadata,
    ) -> PluginSessionResult<()> {
        remove_request_headers(session, &self.owned_header_names)?;
        for (name, value) in &metadata.headers {
            // Keep the live setter's replacement and deferred-mutation semantics.
            session.set_request_header_value(name, value.clone())?;
        }
        Ok(())
    }

    /// Clear owned names, then write the anonymous identity headers.
    fn set_anonymous_headers(&self, session: &mut dyn PluginSession, anon_user: &str) -> PluginSessionResult<()> {
        remove_request_headers(session, &self.owned_header_names)?;
        if self.forward_anonymous_username {
            set_sensitive_request_header(session, "X-Consumer-Username", anon_user)?;
        }
        set_sensitive_request_header(session, "X-Anonymous-Consumer", "true")
    }

    /// Hide credentials from upstream request based on the key source used
    fn hide_credentials(
        &self,
        session: &mut dyn PluginSession,
        used_source: &KeyGet,
    ) -> Result<(), HideCredentialError> {
        match used_source {
            KeyGet::Header { name } => {
                session
                    .remove_request_header(name)
                    .map_err(|_| HideCredentialError::Removal)?;
            }
            KeyGet::Ctx { name } => {
                // A ctx variable is never forwarded upstream as a request header.
                let _ = session.remove_ctx_var(name);
            }
            KeyGet::Query { name } => {
                strip_query_param(session, name).map_err(|_| HideCredentialError::Removal)?;
            }
            KeyGet::Cookie { name } => {
                strip_cookie(session, name).map_err(HideCredentialError::Cookie)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The single 401 writer: applies the shared failure delay, then sends
    /// the challenge with `body`.
    async fn unauthorized(&self, session: &mut dyn PluginSession, body: &'static str) -> PluginRunningResult {
        apply_auth_failure_delay(self.auth_failure_delay_ms).await;
        let _ = send_gateway_auth_error_response(session, 401, &self.auth_challenge, None, body).await;
        PluginRunningResult::ErrTerminateRequest
    }

    /// Deny a request whose identity-header mutation or credential removal
    /// failed, with the bad-credential response of that path.
    async fn deny(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        tag: &'static str,
        body: &'static str,
    ) -> PluginRunningResult {
        crate::plugins::http::common::auth_common::note_auth_header_mutation_failure(session, plugin_log, tag);
        self.unauthorized(session, body).await
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
                // A failed clear or write must deny: a client-supplied
                // identity header would otherwise reach the upstream.
                if self.set_anonymous_headers(session, anon_user).is_err() {
                    return self
                        .deny(session, plugin_log, IDENTITY_HEADER_REJECTED, MISSING_KEY_BODY)
                        .await;
                }
                plugin_log.push("Allow anon");
                return PluginRunningResult::GoodNext;
            }

            plugin_log.push("Deny no key");
            return self.unauthorized(session, MISSING_KEY_BODY).await;
        };

        // 2. Validate key
        let Some(metadata) = self.validate_key(&key) else {
            plugin_log.push("Deny bad key");
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

        // 3. Clear owned names, then set upstream headers (from whitelist)
        if self.set_upstream_headers(session, metadata).is_err() {
            return self
                .deny(session, plugin_log, IDENTITY_HEADER_REJECTED, INVALID_KEY_BODY)
                .await;
        }

        // 4. Hide credentials if configured
        if self.hide_credentials {
            match self.hide_credentials(session, used_source) {
                Ok(()) => {}
                Err(HideCredentialError::Removal) => {
                    return self
                        .deny(session, plugin_log, CREDENTIAL_REMOVAL_FAILED, INVALID_KEY_BODY)
                        .await;
                }
                Err(HideCredentialError::Cookie(error)) => return cookie_cleanup_failure(error),
            }
        }

        // 5. Log OK with group attribution + optional username, only after
        // every mutation and credential cleanup succeeded.
        if let Some(username) = &metadata.username {
            plugin_log.push_fmt("OK group={} user {}", la![metadata.group.as_ref(), sensitive(username)]);
        } else {
            plugin_log.push_fmt("OK group={}", la![metadata.group.as_ref()]);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/key_auth/plugin_test.rs"]
mod tests;
