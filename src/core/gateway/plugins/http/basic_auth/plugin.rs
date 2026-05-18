//! Basic Authentication plugin implementation

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret;
use crate::core::gateway::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response,
};
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::BasicAuthConfig;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use sha1::Digest;

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
    // Simple username -> password_hash mapping.
    // Stored values are htpasswd-style hashes ({SHA} / $apr1$ / $1$).
    // Plaintext passwords from Secrets are auto-hashed as {SHA}<base64(sha1(password))>
    // on load, matching what verify() expects.
    user_passwords: Arc<HashMap<String, String>>,
    plugin_namespace: String,
    config: BasicAuthConfig,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig, plugin_namespace: String) -> Self {
        let mut plugin = BasicAuth {
            name: "BasicAuth".to_string(),
            user_passwords: Arc::new(HashMap::new()),
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

    /// Load users from resolved Secret data.
    ///
    /// - If the password looks like a supported htpasswd hash, store as-is.
    /// - Otherwise treat it as plaintext and hash as `{SHA}<base64(sha1(password))>`,
    ///   which matches what `htpasswd.rs::verify` recognises and aligns with the
    ///   `{SHA}` htpasswd format.
    pub fn load_users(&mut self, users: HashMap<String, String>) -> Result<(), String> {
        let mut new_map = HashMap::new();
        for (username, password) in users {
            if self.is_pre_hashed(&password) {
                // Store pre-hashed password directly
                new_map.insert(username, password);
            } else {
                // Plaintext password -> store as {SHA}<base64(sha1(password))>.
                let digest = sha1::Sha1::digest(password.as_bytes());
                let stored = format!("{{SHA}}{}", general_purpose::STANDARD.encode(digest));
                new_map.insert(username, stored);
            }
        }
        self.user_passwords = Arc::new(new_map);

        tracing::info!(
            "BasicAuth: Loaded {} users (mixed hashed/plaintext source)",
            self.user_passwords.len()
        );
        Ok(())
    }

    /// Check if a password string looks like a supported htpasswd hash.
    fn is_pre_hashed(&self, s: &str) -> bool {
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

        // All supported hashes (SHA-1 / MD5-crypt / APR-MD5) are cheap to verify.
        let ph = stored_hash.clone();
        let pw = password.clone();
        let is_valid = tokio::task::spawn_blocking(move || super::htpasswd::verify(&pw, &ph))
            .await
            .map_err(|_| AuthFailure::BadCredentials)?;

        if !is_valid {
            return Err(AuthFailure::BadCredentials);
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

        let (username, password) = decoded_str.split_once(':').ok_or("Invalid decoded data format")?;

        if username.is_empty() || password.is_empty() {
            return Err("Empty username or password".into());
        }

        Ok((username.to_string(), password.to_string()))
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
        send_gateway_auth_error_response(
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
                // a separate concern — verify only runs when the user exists, so a fast
                // path still reveals user existence. Mitigate via `auth_failure_delay_ms`
                // at the deployment layer.
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
    use base64::engine::general_purpose;

    fn create_basic_auth_with_users() -> BasicAuth {
        let config = BasicAuthConfig {
            realm: "Test Realm".to_string(),
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
}
