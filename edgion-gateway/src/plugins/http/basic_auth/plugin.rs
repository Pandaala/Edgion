//! Basic Authentication plugin implementation

use crate::plugins::http::common::auth_common::set_sensitive_request_header;
use std::collections::HashMap;

use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, PreparedAuthChallenge, CREDENTIAL_REMOVAL_FAILED,
    IDENTITY_HEADER_REJECTED,
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
    user_passwords: PreparedUserMap,
    config: BasicAuthConfig,
    auth_challenge: PreparedAuthChallenge,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        let mut runtime_config = config.clone();
        let users = runtime_config.resolved_users.take().unwrap_or_default();
        let user_passwords = Self::load_users(users);

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

    /// Verify one user's password and return the user's group.
    ///
    /// Supported hashes are verified inline on the request worker, like other
    /// data-plane auth crypto: `{SHA}` costs well under a microsecond for
    /// typical passwords (linear in length, comparable to decoding the header)
    /// and APR1/MD5-crypt (1000 MD5 rounds, password capped at 256 bytes)
    /// roughly 0.1-0.7 ms. A blocking-pool hop would not reduce that CPU cost;
    /// it would add a thread handoff and admit up to 512 blocking threads per
    /// listener runtime, each with an unbounded queue, under credential-guessing
    /// load.
    fn verify_user<'a>(users: &'a PreparedUserMap, username: &str, password: &str) -> Option<&'a str> {
        let user = users.get(username)?;
        super::htpasswd::verify(password, &user.password_hash).then_some(user.group.as_str())
    }

    fn authenticate_request(&self, session: &mut dyn PluginSession) -> Result<(String, &str), AuthFailure> {
        let Some(auth_header_value) = session.header_str("authorization") else {
            return Err(AuthFailure::MissingHeader);
        };

        let (username, password) =
            Self::extract_credentials(auth_header_value).map_err(|_| AuthFailure::BadCredentials)?;

        let group = Self::verify_user(&self.user_passwords, &username, &password).ok_or(AuthFailure::BadCredentials)?;
        Ok((username, group))
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

    /// Write the anonymous identity headers.
    fn set_anonymous_headers(session: &mut dyn PluginSession, anonymous: &str) -> BasicAuthResult<()> {
        set_sensitive_request_header(session, "X-Consumer-Username", anonymous)?;
        set_sensitive_request_header(session, "X-Anonymous-Consumer", "true")?;
        Ok(())
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

    /// Every 401 of this plugin: the failure delay first (one place, so no
    /// branch can answer faster than another), then the challenge response.
    async fn deny(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
        let _ = self.auth_failed_return(session).await;
        PluginRunningResult::ErrTerminateRequest
    }

    /// A failed identity-header write or credential removal: the same 401 as a
    /// bad credential, plus the alertable diagnostic (usually operator
    /// misconfiguration).
    async fn deny_header(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        tag: &'static str,
    ) -> PluginRunningResult {
        crate::plugins::http::common::auth_common::note_auth_header_mutation_failure(session, plugin_log, tag);
        self.deny(session).await
    }
}

#[async_trait]
impl RequestFilter for BasicAuth {
    fn name(&self) -> &str {
        "BasicAuth"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let (username, group) = match self.authenticate_request(session) {
            Ok(result) => result,
            Err(AuthFailure::MissingHeader) => {
                if let Some(anonymous) = &self.config.anonymous {
                    // A failed write must deny: a client-supplied identity
                    // header would otherwise reach the upstream.
                    if Self::set_anonymous_headers(session, anonymous).is_err() {
                        return self.deny_header(session, plugin_log, IDENTITY_HEADER_REJECTED).await;
                    }
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }
                plugin_log.push("Deny no credential");
                return self.deny(session).await;
            }
            Err(AuthFailure::BadCredentials) => {
                // Unified PluginLog tag blocks log-grep-based enumeration (unknown user vs
                // wrong password look identical in access log). Timing-side enumeration is
                // a separate concern — verify only runs when the user exists, so a fast
                // path still reveals user existence. `auth_failure_delay_ms` is added after
                // verify, so it slows guessing but does not equalize timing; limit probing
                // with rate limiting at the deployment layer.
                plugin_log.push("Deny bad credential");
                // Credentials present but invalid — always reject, never anonymous
                return self.deny(session).await;
            }
        };

        // Write the identity header before the success line: a failed write
        // denies the request (a client-supplied value must never reach the
        // upstream) and a denied request must not carry `OK group=`. The
        // shared delay keeps this 401 as slow as a bad-credential 401, so
        // timing does not reveal the password check.
        if self.set_consumer_headers(session, &username).is_err() {
            return self.deny_header(session, plugin_log, IDENTITY_HEADER_REJECTED).await;
        }

        // A hidden credential that cannot be removed must not reach the
        // upstream: deny with the same 401 instead of forwarding it.
        if self.config.hide_credentials && session.remove_request_header("authorization").is_err() {
            return self.deny_header(session, plugin_log, CREDENTIAL_REMOVAL_FAILED).await;
        }
        plugin_log.push_fmt("OK group={} user {}", la![group, sensitive(&username)]);

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/basic_auth/plugin_test.rs"]
mod tests;
