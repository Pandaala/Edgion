//! Basic Authentication plugin implementation

use std::collections::HashMap;
use std::sync::Arc;

use crate::plugins::http::common::auth_common::{apply_auth_failure_delay, send_gateway_auth_error_response};
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
    name: String,
    // The Arc pins one immutable user-map generation across spawn_blocking.
    user_passwords: Arc<PreparedUserMap>,
    config: BasicAuthConfig,
}

impl BasicAuth {
    /// Create a new BasicAuth plugin from configuration
    pub fn new(config: &BasicAuthConfig) -> Self {
        let mut runtime_config = config.clone();
        let users = runtime_config.resolved_users.take().unwrap_or_default();
        let user_passwords = Arc::new(Self::load_users(users));

        BasicAuth {
            name: "BasicAuth".to_string(),
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
        let users = Arc::clone(&self.user_passwords);
        users.contains_key(username).then_some(users)
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
        let auth_header_value = match session.header_str("authorization") {
            Some(v) => v,
            None => return Err(AuthFailure::MissingHeader),
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
            None,
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use crate::plugins::runtime::PluginLog;
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
            resolved_users: Some(make_user_map("testuser", "testpass", "default")),
            ..Default::default()
        };
        BasicAuth::new(&config)
    }

    fn basic_header(decoded: &[u8]) -> String {
        format!("Basic {}", general_purpose::STANDARD.encode(decoded))
    }

    fn prepared_users(username: &str, password: &str, group: &str) -> Arc<PreparedUserMap> {
        Arc::new(BasicAuth::prepare_users(make_user_map(username, password, group)))
    }

    #[test]
    fn test_extract_credentials_case_insensitive_scheme() {
        let header = format!(
            "basic {}",
            general_purpose::STANDARD.encode("testuser:testpass".as_bytes())
        );
        let (username, password) = BasicAuth::extract_credentials(&header).expect("should parse");
        assert_eq!(username, "testuser");
        assert_eq!(password, "testpass");
    }

    #[test]
    fn test_extract_credentials_preserves_spaces_and_password_colons() {
        let header = basic_header(b" user : pass :tail");
        let (username, password) = BasicAuth::extract_credentials(&header).expect("should parse");
        assert_eq!(username, " user ");
        assert_eq!(password, " pass :tail");
    }

    #[test]
    fn test_extract_credentials_error_matrix() {
        let cases = [
            ("Basic".to_string(), "Invalid authorization header format"),
            ("Bearer abc".to_string(), "Invalid authorization header format"),
            ("Basic   ".to_string(), "Invalid authorization header format"),
            ("Basic !!!".to_string(), "Failed to decode authentication header"),
            (basic_header(&[0xff]), "Invalid UTF-8 in decoded authorization"),
            (basic_header(b"username"), "Invalid decoded data format"),
            (basic_header(b":password"), "Empty username or password"),
            (basic_header(b"username:"), "Empty username or password"),
        ];

        for (header, expected) in cases {
            let error = BasicAuth::extract_credentials(&header).expect_err("input must be rejected");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn password_prefix_classification_is_exact_and_case_sensitive() {
        for password in ["{SHA}hash", "$apr1$salt$hash", "$1$salt$hash"] {
            assert!(matches!(
                BasicAuth::classify_password(password),
                PasswordEncoding::SupportedHash
            ));
        }
        for password in [
            "$2$hash",
            "$2a$hash",
            "$2b$hash",
            "$2x$hash",
            "$2y$hash",
            "$5$salt$hash",
            "$6$salt$hash",
        ] {
            assert!(matches!(
                BasicAuth::classify_password(password),
                PasswordEncoding::RecognizedUnsupportedHash
            ));
        }
        for password in ["{sha}hash", "$APR1$salt$hash", "$2A$hash", "plain"] {
            assert!(matches!(
                BasicAuth::classify_password(password),
                PasswordEncoding::Plaintext
            ));
        }
    }

    #[test]
    fn prepare_users_moves_supported_and_unsupported_buffers() {
        let supported_password = "{SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g=".to_string();
        let supported_password_ptr = supported_password.as_ptr();
        let supported_group = "supported-group".to_string();
        let supported_group_ptr = supported_group.as_ptr();
        let unsupported_password = "$2b$12$recognized-but-unsupported".to_string();
        let unsupported_password_ptr = unsupported_password.as_ptr();
        let unsupported_group = "unsupported-group".to_string();
        let unsupported_group_ptr = unsupported_group.as_ptr();
        let users = HashMap::from([
            (
                "supported".to_string(),
                ResolvedBasicAuthUser {
                    password: supported_password,
                    group: supported_group,
                },
            ),
            (
                "unsupported".to_string(),
                ResolvedBasicAuthUser {
                    password: unsupported_password,
                    group: unsupported_group,
                },
            ),
        ]);

        let prepared = BasicAuth::prepare_users(users);
        let supported = prepared.get("supported").unwrap();
        assert_eq!(supported.password_hash.as_ptr(), supported_password_ptr);
        assert_eq!(supported.group.as_ptr(), supported_group_ptr);
        let unsupported = prepared.get("unsupported").unwrap();
        assert_eq!(unsupported.password_hash.as_ptr(), unsupported_password_ptr);
        assert_eq!(unsupported.group.as_ptr(), unsupported_group_ptr);
    }

    #[test]
    fn recognized_unsupported_hashes_remain_unchanged_and_reject_hash_literal() {
        for password_hash in [
            "$2$hash",
            "$2a$hash",
            "$2b$hash",
            "$2x$hash",
            "$2y$hash",
            "$5$salt$hash",
            "$6$salt$hash",
        ] {
            let users = prepared_users("alice", password_hash, "partner-a");
            assert_eq!(users.get("alice").unwrap().password_hash, password_hash);
            assert!(BasicAuth::verify_pinned_user(users, "alice".to_string(), password_hash.to_string()).is_none());
        }
    }

    #[test]
    fn plaintext_is_prepared_as_sha_and_verifies() {
        let users = prepared_users("alice", "password", "partner-a");
        assert!(users.get("alice").unwrap().password_hash.starts_with("{SHA}"));
        assert_eq!(
            BasicAuth::verify_pinned_user(Arc::clone(&users), "alice".to_string(), "password".to_string()),
            Some(("alice".to_string(), "partner-a".to_string()))
        );
        assert!(BasicAuth::verify_pinned_user(users, "alice".to_string(), "wrong".to_string()).is_none());
    }

    #[test]
    fn constructor_removes_resolved_users_from_retained_config() {
        let config = BasicAuthConfig {
            resolved_users: Some(make_user_map("alice", "password", "partner-a")),
            ..Default::default()
        };

        let plugin = BasicAuth::new(&config);

        assert!(plugin.config.resolved_users.is_none());
        assert!(config.resolved_users.as_ref().unwrap().contains_key("alice"));
        assert!(plugin.user_passwords.contains_key("alice"));
    }

    #[test]
    fn pinned_generation_keeps_one_user_version() {
        let old = prepared_users("alice", "old-password", "old-group");
        let new = prepared_users("alice", "new-password", "new-group");

        assert_eq!(
            BasicAuth::verify_pinned_user(Arc::clone(&old), "alice".to_string(), "old-password".to_string()),
            Some(("alice".to_string(), "old-group".to_string()))
        );
        assert!(BasicAuth::verify_pinned_user(old, "alice".to_string(), "new-password".to_string()).is_none());
        assert_eq!(
            BasicAuth::verify_pinned_user(Arc::clone(&new), "alice".to_string(), "new-password".to_string()),
            Some(("alice".to_string(), "new-group".to_string()))
        );
        assert!(BasicAuth::verify_pinned_user(new, "alice".to_string(), "old-password".to_string()).is_none());
    }

    #[tokio::test]
    async fn known_and_unknown_users_keep_distinct_dispatch_control() {
        let plugin = create_basic_auth_with_users();
        assert!(plugin.pin_known_user_generation("testuser").is_some());
        assert!(plugin.pin_known_user_generation("missing").is_none());

        let mut known = MockPluginSession::new();
        setup_header_mocks(
            &mut known,
            &[("authorization", basic_header(b"testuser:testpass").as_str())],
        );
        assert_eq!(
            plugin.authenticate_request(&mut known).await.unwrap(),
            ("testuser".to_string(), "default".to_string())
        );

        let mut unknown = MockPluginSession::new();
        setup_header_mocks(
            &mut unknown,
            &[("authorization", basic_header(b"missing:testpass").as_str())],
        );
        assert!(matches!(
            plugin.authenticate_request(&mut unknown).await,
            Err(AuthFailure::BadCredentials)
        ));
    }

    fn render_log(log: &PluginLog) -> Vec<String> {
        let v: Value = serde_json::to_value(log).unwrap();
        v.get("log")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn basic_hit_log_has_exact_group_and_user_shape() {
        let mut log = PluginLog::new("BasicAuth");
        let group = "partner-a";
        let username = "alice";
        log.push_fmt("OK group={} user {}", la![group, sensitive(username)]);

        let entries = render_log(&log);
        assert_eq!(entries, vec!["OK group=partner-a user alice"]);
    }

    #[test]
    fn basic_failure_logs_have_exact_non_attributing_shape() {
        for expected in ["Allow anonymous", "Deny no credential", "Deny bad credential"] {
            let mut log = PluginLog::new("BasicAuth");
            log.push(expected);
            let entries = render_log(&log);
            assert_eq!(entries, vec![expected]);
            assert!(!entries[0].contains("group="));
            assert!(!entries[0].contains("user "));
        }
    }
}
