//! Basic Authentication plugin implementation

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::gateway::cache::ShardedLruCache;
use crate::core::gateway::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use crate::core::gateway::plugins::http::common::auth_common::{apply_auth_failure_delay, send_auth_error_response};
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::BasicAuthConfig;

use async_singleflight::DefaultGroup;
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use sha2::{Digest, Sha256};

type BasicAuthError = Box<dyn std::error::Error + Send + Sync>;
type BasicAuthResult<T> = Result<T, BasicAuthError>;

/// Distinguishes "no credentials" from "bad credentials" for anonymous access logic.
#[derive(Debug)]
enum AuthFailure {
    MissingHeader,
    BadCredentials,
}

/// Basic Authentication plugin
pub struct BasicAuth {
    name: String,
    // Simple username -> password_hash mapping
    // We store whatever is verified: either a bcrypt hash (generated locally)
    // or a pre-supplied hash (from htpasswd).
    user_passwords: Arc<HashMap<String, String>>,
    /// LRU cache: auth header value -> username (TTL-based, managed by LocalLruCache)
    auth_cache: Arc<ShardedLruCache<String, String>>,
    sf_group: Arc<DefaultGroup<bool>>,
    plugin_namespace: String,
    config: BasicAuthConfig,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig, plugin_namespace: String) -> Self {
        let capacity = if config.lru_cache.capacity == 0 {
            get_default_lru_cache_capacity()
        } else {
            config.lru_cache.capacity
        };
        let shards = if config.lru_cache.shards > 0 {
            config.lru_cache.shards
        } else {
            get_default_lru_cache_shards()
        };
        let mut plugin = BasicAuth {
            name: "BasicAuth".to_string(),
            user_passwords: Arc::new(HashMap::new()),
            auth_cache: Arc::new(ShardedLruCache::new(capacity, shards)),
            sf_group: Arc::new(DefaultGroup::new()),
            plugin_namespace,
            config: config.clone(),
        };

        let users = config
            .resolved_users
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| plugin.load_users_from_secret_refs());
        if !users.is_empty() {
            if let Err(err) = plugin.load_users(users) {
                tracing::warn!("BasicAuth: failed to load resolved users: {}", err);
            }
        }
        plugin
    }

    fn load_users_from_secret_refs(&self) -> HashMap<String, String> {
        let mut users = HashMap::new();
        let Some(secret_refs) = self.config.secret_refs.as_ref() else {
            return users;
        };
        for secret_ref in secret_refs {
            let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
            let Some(secret) = get_secret(Some(ns), &secret_ref.name) else {
                continue;
            };

            let Some(username) = Self::read_secret_utf8(&secret, "username") else {
                continue;
            };
            let Some(password) = Self::read_secret_utf8(&secret, "password") else {
                continue;
            };
            if users.contains_key(&username) {
                tracing::warn!("BasicAuth: duplicate username '{}', skipping", username);
                continue;
            }
            users.insert(username, password);
        }
        users
    }

    fn read_secret_utf8(secret: &k8s_openapi::api::core::v1::Secret, key: &str) -> Option<String> {
        if let Some(data) = &secret.data {
            if let Some(bytes) = data.get(key) {
                if let Ok(v) = String::from_utf8(bytes.0.clone()) {
                    if !v.trim().is_empty() {
                        return Some(v);
                    }
                }
            }
        }
        if let Some(string_data) = &secret.string_data {
            if let Some(v) = string_data.get(key) {
                if !v.trim().is_empty() {
                    return Some(v.clone());
                }
            }
        }
        None
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
                // Warn (once per load) when a weak htpasswd hash format is encountered.
                // SHA-1 ({SHA}) is unsalted and APR1 ($apr1$) / MD5-crypt ($1$) only run 1000
                // rounds. Operators should migrate to bcrypt.
                let weak_fmt = if password.starts_with("{SHA}") {
                    Some("{SHA}")
                } else if password.starts_with("$apr1$") {
                    Some("$apr1$")
                } else if password.starts_with("$1$") {
                    Some("$1$")
                } else {
                    None
                };
                if let Some(fmt) = weak_fmt {
                    tracing::warn!(
                        component = "basic_auth",
                        "htpasswd entry uses weak hash format ({}), consider upgrading to bcrypt",
                        fmt
                    );
                }
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

    async fn authenticate_request(&self, session: &mut dyn PluginSession) -> Result<String, AuthFailure> {
        let auth_header_value = match session.header_value("authorization") {
            Some(v) => v,
            None => return Err(AuthFailure::MissingHeader),
        };

        // Fast path: cache lookup (skipped entirely when cache_ttl == 0)
        if self.config.cache_ttl > 0 {
            if let Some(username) = self.auth_cache.get(&auth_header_value) {
                return Ok(username);
            }
        }

        // Decode credentials
        let (username, password) = self
            .extract_credentials(&auth_header_value)
            .map_err(|_| AuthFailure::BadCredentials)?;

        // Look up stored hash
        let stored_hash = self
            .user_passwords
            .get(&username)
            .ok_or(AuthFailure::BadCredentials)?
            .clone();

        // Bcrypt hashes start with "$2" — only these are expensive enough to warrant singleflight
        let is_bcrypt = stored_hash.starts_with("$2");
        let is_valid: bool = if is_bcrypt && self.config.singleflight {
            // singleflight key: SHA256(password_bytes || stored_hash_bytes)
            // Never stores plaintext password in the map key.
            let sf_key = {
                let mut h = Sha256::new();
                h.update(password.as_bytes());
                h.update(stored_hash.as_bytes());
                format!("{:x}", h.finalize())
            };
            let ph = stored_hash.clone();
            let pw = password.clone();
            // work() returns Result<bool, Option<()>> — unwrap_or(false) on error
            let result = self
                .sf_group
                .work(&sf_key, async move {
                    let verified = tokio::task::spawn_blocking(move || super::htpasswd::verify(&pw, &ph))
                        .await
                        .unwrap_or(false);
                    Ok::<bool, ()>(verified)
                })
                .await;
            result.unwrap_or(false)
        } else {
            // Non-bcrypt (SHA1/MD5/APR-MD5) or singleflight disabled
            let ph = stored_hash.clone();
            let pw = password.clone();
            tokio::task::spawn_blocking(move || super::htpasswd::verify(&pw, &ph))
                .await
                .map_err(|_| AuthFailure::BadCredentials)?
        };

        if !is_valid {
            return Err(AuthFailure::BadCredentials);
        }

        // Cache success only — failures are never stored
        if self.config.cache_ttl > 0 {
            self.auth_cache.insert(
                auth_header_value.to_string(),
                username.clone(),
                Some(Duration::from_secs(self.config.cache_ttl)),
            );
        }

        Ok(username)
    }

    fn extract_credentials(&self, authorization: &str) -> BasicAuthResult<(String, String)> {
        let (scheme, encoded) = authorization
            .split_once(' ')
            .ok_or("Invalid authorization header format")?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return Err("Invalid authorization header format".into());
        }
        let encoded = encoded.trim();
        if encoded.is_empty() {
            return Err("Invalid authorization header format".into());
        }

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

        let username = parts[0].to_string();
        let password = parts[1].to_string();

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
            plugin_log.push("Allow anonymous");
            let _ = session.set_request_header("X-Consumer-Username", anonymous);
            let _ = session.set_request_header("X-Anonymous-Consumer", "true");
            return true;
        }
        false
    }

    async fn auth_failed_return(&self, session: &mut dyn PluginSession) -> BasicAuthResult<()> {
        send_auth_error_response(
            session,
            401,
            "Basic",
            &self.config.realm,
            "Unauthorized - Authentication required",
        )
        .await
    }
}

#[async_trait]
impl RequestFilter for BasicAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let username = match self.authenticate_request(session).await {
            Ok(user) => user,
            Err(AuthFailure::MissingHeader) => {
                if self.handle_anonymous_access(session, plugin_log) {
                    return PluginRunningResult::GoodNext;
                }
                plugin_log.push("Deny no credential");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                let _ = self.auth_failed_return(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
            Err(AuthFailure::BadCredentials) => {
                // Unified PluginLog tag blocks log-grep-based enumeration (unknown user vs
                // wrong password look identical in access log). Timing-side enumeration is
                // a separate concern — bcrypt verify only runs when the user exists, so a
                // sub-ms fast path still reveals user existence. Mitigate via
                // `auth_failure_delay_ms` at the deployment layer.
                plugin_log.push("Deny bad credential");
                // Credentials present but invalid — always reject, never anonymous
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                let _ = self.auth_failed_return(session).await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        plugin_log.push_fmt("OK user {}", la![sensitive(&username)]);

        // Set consumer headers for upstream
        let _ = self.set_consumer_headers(session, &username);

        // Hide credentials if configured
        if self.config.hide_credentials {
            let _ = session.remove_request_header("authorization");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_plugins::LruCacheConfig;
    use base64::engine::general_purpose;

    fn create_basic_auth_with_users() -> BasicAuth {
        let config = BasicAuthConfig {
            realm: "Test Realm".to_string(),
            cache_ttl: 300,
            lru_cache: LruCacheConfig {
                capacity: 10,
                ..Default::default()
            },
            singleflight: false, // disable for deterministic tests
            ..Default::default()
        };
        let mut auth = BasicAuth::new(&config, "default".to_string());
        let mut users = HashMap::new();
        users.insert("testuser".to_string(), "testpass".to_string());
        auth.load_users(users).unwrap();
        auth
    }

    #[test]
    fn test_extract_credentials_case_insensitive_scheme() {
        let auth = create_basic_auth_with_users();
        let header = format!(
            "basic {}",
            general_purpose::STANDARD.encode("testuser:testpass".as_bytes())
        );
        let (username, password) = auth.extract_credentials(&header).expect("should parse");
        assert_eq!(username, "testuser");
        assert_eq!(password, "testpass");
    }

    #[test]
    fn test_extract_credentials_preserves_spaces() {
        let auth = create_basic_auth_with_users();
        let header = format!("Basic {}", general_purpose::STANDARD.encode(" user : pass ".as_bytes()));
        let (username, password) = auth.extract_credentials(&header).expect("should parse");
        assert_eq!(username, " user ");
        assert_eq!(password, " pass ");
    }

    #[test]
    fn test_basic_auth_cache_is_lru() {
        let config = BasicAuthConfig {
            cache_ttl: 60,
            lru_cache: LruCacheConfig {
                capacity: 10,
                ..Default::default()
            },
            singleflight: false,
            resolved_users: Some(std::collections::HashMap::from([("u".to_string(), "p".to_string())])),
            ..Default::default()
        };
        let auth = BasicAuth::new(&config, "default".to_string());
        assert_eq!(auth.auth_cache.len(), 0);
    }
}
