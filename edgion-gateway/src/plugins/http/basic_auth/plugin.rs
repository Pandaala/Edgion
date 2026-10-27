//! Basic Authentication plugin implementation

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use std::collections::HashMap;
use std::sync::Arc;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, PreparedAuthChallenge,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BasicAuthConfig, ResolvedBasicAuthUser};

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

type PreparedUserMap = HashMap<String, PreparedUser>;

struct PreparedUser {
    password_hash: String,
    group: String,
}

enum PasswordEncoding {
    Plaintext,
    SupportedHash,
    RecognizedUnsupportedHash,
}

/// Basic Authentication plugin
pub struct BasicAuth {
    // The Arc pins one immutable user-map generation across spawn_blocking.
    user_passwords: Arc<PreparedUserMap>,
    config: BasicAuthConfig,
    auth_challenge: PreparedAuthChallenge,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        let mut runtime_config = config.clone();
        let users = runtime_config.resolved_users.take().unwrap_or_default();
        let user_passwords = Arc::new(Self::load_users(users));

        BasicAuth {
            auth_challenge: PreparedAuthChallenge::new("Basic", &runtime_config.realm),
            user_passwords,
            config: runtime_config,
        }
    }

    /// Cold construction boundary retained for the gateway tracing guard.
    fn load_users(users: HashMap<String, ResolvedBasicAuthUser>) -> PreparedUserMap {
        let prepared = Self::prepare_users(users);
        tracing::info!(
            "BasicAuth: Loaded {} users (mixed hashed/plaintext source)",
            prepared.len()
        );
        prepared
    }

    fn prepare_users(users: HashMap<String, ResolvedBasicAuthUser>) -> PreparedUserMap {
        let mut new_map = HashMap::new();
        for (username, user) in users {
            let password_hash = match Self::classify_password(&user.password) {
                PasswordEncoding::Plaintext => {
                    let digest = sha1::Sha1::digest(user.password.as_bytes());
                    format!("{{SHA}}{}", general_purpose::STANDARD.encode(digest))
                }
                PasswordEncoding::SupportedHash | PasswordEncoding::RecognizedUnsupportedHash => user.password,
            };
            new_map.insert(
                username,
                PreparedUser {
                    password_hash,
                    group: user.group,
                },
            );
        }
        new_map
    }

    fn classify_password(password: &str) -> PasswordEncoding {
        if password.starts_with("{SHA}") || password.starts_with("$apr1$") || password.starts_with("$1$") {
            PasswordEncoding::SupportedHash
        } else if ["$2$", "$2a$", "$2b$", "$2x$", "$2y$", "$5$", "$6$"]
            .iter()
            .any(|prefix| password.starts_with(prefix))
        {
            PasswordEncoding::RecognizedUnsupportedHash
        } else {
            PasswordEncoding::Plaintext
        }
    }

    fn pin_known_user_generation(&self, username: &str) -> Option<Arc<PreparedUserMap>> {
        self.user_passwords
            .contains_key(username)
            .then(|| Arc::clone(&self.user_passwords))
    }

    fn verify_pinned_user(users: Arc<PreparedUserMap>, username: String, password: String) -> Option<(String, String)> {
        let user = users.get(&username)?;
        if !super::htpasswd::verify(&password, &user.password_hash) {
            return None;
        }
        let group = user.group.clone();
        Some((username, group))
    }

    async fn authenticate_request(&self, session: &mut dyn PluginSession) -> Result<(String, String), AuthFailure> {
        let Some(auth_header_value) = session.header_str("authorization") else {
            return Err(AuthFailure::MissingHeader);
        };

        let (username, password) =
            Self::extract_credentials(auth_header_value).map_err(|_| AuthFailure::BadCredentials)?;

        // Preserve the fast miss for unknown users. Known users carry this same
        // immutable generation into the blocking verifier and re-look up there.
        let users = self
            .pin_known_user_generation(&username)
            .ok_or(AuthFailure::BadCredentials)?;

        // All supported hashes (SHA-1 / MD5-crypt / APR-MD5) are cheap to verify.
        tokio::task::spawn_blocking(move || Self::verify_pinned_user(users, username, password))
            .await
            .map_err(|_| AuthFailure::BadCredentials)?
            .ok_or(AuthFailure::BadCredentials)
    }

    fn extract_credentials(authorization: &str) -> BasicAuthResult<(String, String)> {
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

        let decoded = String::from_utf8(decoded).map_err(|_| "Invalid UTF-8 in decoded authorization")?;
        Self::split_decoded_credentials(decoded)
    }

    fn split_decoded_credentials(mut decoded: String) -> BasicAuthResult<(String, String)> {
        let separator = decoded.find(':').ok_or("Invalid decoded data format")?;
        if separator == 0 || separator + 1 == decoded.len() {
            return Err("Empty username or password".into());
        }

        let username = decoded[..separator].to_string();
        drop(decoded.drain(..=separator));
        Ok((username, decoded))
    }

    fn set_consumer_headers(&self, session: &mut dyn PluginSession, username: &str) -> BasicAuthResult<()> {
        // Set X-Consumer-Username header
        set_sensitive_request_header(session, "X-Consumer-Username", username)?;
        Ok(())
    }

    fn handle_anonymous_access(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> bool {
        if let Some(ref anonymous) = self.config.anonymous {
            plugin_log.push("Allow anonymous");
            let _ = set_sensitive_request_header(session, "X-Consumer-Username", anonymous);
            let _ = set_sensitive_request_header(session, "X-Anonymous-Consumer", "true");
            return true;
        }
        false
    }

    async fn auth_failed_return(&self, session: &mut dyn PluginSession) -> BasicAuthResult<()> {
        send_gateway_auth_error_response(
            session,
            401,
            &self.auth_challenge,
            None,
            "Unauthorized - Authentication required",
        )
        .await
    }
}

#[async_trait]
impl RequestFilter for BasicAuth {
    fn name(&self) -> &str {
        "BasicAuth"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let (username, group) = match self.authenticate_request(session).await {
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

        plugin_log.push_fmt("OK group={} user {}", la![group.as_str(), sensitive(&username)]);

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
#[path = "../../../../tests/unit/plugins/http/basic_auth/plugin_test.rs"]
mod tests;
