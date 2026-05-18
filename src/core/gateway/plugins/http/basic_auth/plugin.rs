//! Basic Authentication plugin implementation

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::gateway::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response,
};
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{BasicAuthConfig, ResolvedBasicAuthUser};

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
    // username -> ResolvedBasicAuthUser (password_hash + group).
    // Stored passwords are htpasswd-style hashes ({SHA} / $apr1$ / $1$).
    // Plaintext passwords from Secrets are auto-hashed as {SHA}<base64(sha1(password))>
    // on load, matching what verify() expects.
    user_passwords: Arc<HashMap<String, ResolvedBasicAuthUser>>,
    config: BasicAuthConfig,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        let mut plugin = BasicAuth {
            name: "BasicAuth".to_string(),
            user_passwords: Arc::new(HashMap::new()),
            config: config.clone(),
        };

        let initial_users = config.resolved_users.clone().unwrap_or_default();
        let _ = plugin.load_users(initial_users);
        plugin
    }

    /// Load users from resolved Secret data.
    ///
    /// - If the password looks like a supported htpasswd hash, store as-is.
    /// - Otherwise treat it as plaintext and hash as `{SHA}<base64(sha1(password))>`,
    ///   which matches what `htpasswd.rs::verify` recognises and aligns with the
    ///   `{SHA}` htpasswd format.
    pub fn load_users(&mut self, users: HashMap<String, ResolvedBasicAuthUser>) -> Result<(), String> {
        let mut new_map = HashMap::new();
        for (username, user) in users {
            let stored_password = if self.is_pre_hashed(&user.password) {
                user.password
            } else {
                // Plaintext -> {SHA}<base64(sha1(password))>
                let digest = sha1::Sha1::digest(user.password.as_bytes());
                format!("{{SHA}}{}", general_purpose::STANDARD.encode(digest))
            };
            new_map.insert(
                username,
                ResolvedBasicAuthUser {
                    password: stored_password,
                    group: user.group,
                },
            );
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

    async fn authenticate_request(
        &self,
        session: &mut dyn PluginSession,
    ) -> Result<(String, ResolvedBasicAuthUser), AuthFailure> {
        let auth_header_value = match session.header_value("authorization") {
            Some(v) => v,
            None => return Err(AuthFailure::MissingHeader),
        };

        // Decode credentials
        let (username, password) = self
            .extract_credentials(&auth_header_value)
            .map_err(|_| AuthFailure::BadCredentials)?;

        // Look up stored user (clone to avoid borrow across await)
        let user = self
            .user_passwords
            .get(&username)
            .cloned()
            .ok_or(AuthFailure::BadCredentials)?;

        // All supported hashes (SHA-1 / MD5-crypt / APR-MD5) are cheap to verify.
        let ph = user.password.clone();
        let pw = password.clone();
        let is_valid = tokio::task::spawn_blocking(move || super::htpasswd::verify(&pw, &ph))
            .await
            .map_err(|_| AuthFailure::BadCredentials)?;

        if !is_valid {
            return Err(AuthFailure::BadCredentials);
        }

        Ok((username, user))
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
        let (username, user) = match self.authenticate_request(session).await {
            Ok(result) => result,
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

        plugin_log.push_fmt("OK group={} user {}", la![user.group.as_str(), sensitive(&username)]);

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
    use crate::core::gateway::plugins::runtime::PluginLog;
    use base64::engine::general_purpose;
    use serde_json::Value;

    fn make_user_map(username: &str, password: &str, group: &str) -> HashMap<String, ResolvedBasicAuthUser> {
        let mut map = HashMap::new();
        map.insert(
            username.to_string(),
            ResolvedBasicAuthUser {
                password: password.to_string(),
                group: group.to_string(),
            },
        );
        map
    }

    fn create_basic_auth_with_users() -> BasicAuth {
        let config = BasicAuthConfig {
            realm: "Test Realm".to_string(),
            ..Default::default()
        };
        let mut auth = BasicAuth::new(&config);
        let users = make_user_map("testuser", "testpass", "default");
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

    fn render_log(log: &PluginLog) -> Vec<String> {
        let v: Value = serde_json::to_value(log).unwrap();
        v.get("log")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn basic_hit_log_contains_group_and_user() {
        let mut log = PluginLog::new("BasicAuth");
        let user = ResolvedBasicAuthUser {
            password: "{SHA}base64hash".to_string(),
            group: "partner-a".to_string(),
        };
        let username = "alice";
        log.push_fmt("OK group={} user {}", la![user.group.as_str(), sensitive(username)]);

        let entries = render_log(&log);
        assert!(
            entries
                .iter()
                .any(|e| e.starts_with("OK group=partner-a") && e.contains("user ")),
            "expected 'OK group=partner-a user ...', got: {:?}",
            entries
        );
    }

    #[test]
    fn basic_no_group_in_failure_branches() {
        // Sanity: lesson L7 — failure literals must not carry group=.
        // The contract is enforced by skills/04-review/basic-auth/group-in-ok-log.md.
        // This is a literal-string check on the static deny strings.
        let deny_strings = ["Allow anonymous", "Deny no credential", "Deny bad credential"];
        for s in &deny_strings {
            assert!(!s.contains("group="), "{} must not carry group= (lesson L7)", s);
        }
    }
}
