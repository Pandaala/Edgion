//! LDAP Authentication plugin implementation

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use dashmap::DashMap;
use ldap3::{LdapConnAsync, LdapConnSettings};
use pingora_http::ResponseHeader;
use sha2::{Digest, Sha256};

use crate::core::gateway::plugins::http::common::auth_common::{apply_auth_failure_delay, send_auth_error_response};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::LdapAuthConfig;

/// LDAP Authentication plugin
pub struct LdapAuth {
    name: String,
    config: LdapAuthConfig,
    ldap_url: String,
    /// Cache key: SHA-256(username:password), value: (username, expiry)
    auth_cache: Arc<DashMap<String, (String, Instant)>>,
}

#[derive(Debug)]
enum LdapAuthError {
    InvalidCredentials,
    ConnectionFailed(String),
    Timeout,
    BindFailed(String),
}

#[derive(Debug, PartialEq)]
enum ParseAuthResult {
    Missing,
    Invalid,
    Valid(String, String),
}

impl LdapAuth {
    /// Create a new LdapAuth plugin from configuration
    pub fn create(config: &LdapAuthConfig) -> Box<dyn RequestFilter> {
        Box::new(Self::new(config))
    }

    pub fn new(config: &LdapAuthConfig) -> Self {
        let scheme = if config.ldaps { "ldaps" } else { "ldap" };
        let ldap_url = format!("{}://{}:{}", scheme, config.ldap_host, config.ldap_port);

        Self {
            name: "LdapAuth".to_string(),
            config: config.clone(),
            ldap_url,
            auth_cache: Arc::new(DashMap::new()),
        }
    }

    fn parse_authorization_value(&self, auth_str: &str) -> Option<(String, String)> {
        let trimmed = auth_str.trim();
        let mut parts = trimmed.splitn(2, ' ');
        let scheme = parts.next()?;
        let encoded = parts.next()?.trim();

        if !scheme.eq_ignore_ascii_case(&self.config.header_type) || encoded.is_empty() {
            return None;
        }

        let decoded = general_purpose::STANDARD.decode(encoded).ok()?;
        let decoded_str = String::from_utf8(decoded).ok()?;
        let mut user_pass = decoded_str.splitn(2, ':');
        let username = user_pass.next()?.trim().to_string();
        let password = user_pass.next()?.to_string();

        if username.is_empty() || password.is_empty() {
            return None;
        }

        Some((username, password))
    }

    /// Parse credentials from request headers.
    /// Priority: Proxy-Authorization > Authorization.
    fn parse_credentials(&self, session: &dyn PluginSession) -> ParseAuthResult {
        let proxy_auth = session.header_value("proxy-authorization");
        let authorization = session.header_value("authorization");

        match (proxy_auth, authorization) {
            (Some(v), _) => self
                .parse_authorization_value(&v)
                .map_or(ParseAuthResult::Invalid, |(u, p)| ParseAuthResult::Valid(u, p)),
            (None, Some(v)) => self
                .parse_authorization_value(&v)
                .map_or(ParseAuthResult::Invalid, |(u, p)| ParseAuthResult::Valid(u, p)),
            (None, None) => ParseAuthResult::Missing,
        }
    }

    fn cache_key(username: &str, password: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(username.as_bytes());
        hasher.update(b":");
        hasher.update(password.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn check_cache(&self, username: &str, password: &str) -> Option<String> {
        if self.config.cache_ttl == 0 {
            return None;
        }

        let key = Self::cache_key(username, password);
        if let Some(entry) = self.auth_cache.get(&key) {
            let (cached_username, expiry) = entry.value();
            if Instant::now() < *expiry {
                return Some(cached_username.clone());
            }

            drop(entry);
            self.auth_cache.remove(&key);
        }

        None
    }

    fn store_cache(&self, username: &str, password: &str) {
        if self.config.cache_ttl == 0 {
            return;
        }

        let key = Self::cache_key(username, password);
        let expiry = Instant::now() + Duration::from_secs(self.config.cache_ttl);
        self.auth_cache.insert(key, (username.to_string(), expiry));
    }

    fn build_bind_dn(&self, username: &str) -> Result<String, &'static str> {
        // Reject dangerous DN special chars to reduce LDAP injection risk.
        if username
            .chars()
            .any(|c| matches!(c, '\\' | ',' | '+' | '"' | '<' | '>' | ';' | '\0'))
        {
            return Err("invalid username characters");
        }

        if let Some(template) = &self.config.bind_dn_template {
            return Ok(template.replace("{username}", username));
        }

        Ok(format!(
            "{}={},{}",
            self.config.attribute, username, self.config.base_dn
        ))
    }

    async fn ldap_bind(&self, bind_dn: &str, password: &str) -> Result<(), LdapAuthError> {
        let timeout = Duration::from_millis(self.config.timeout);

        let settings = LdapConnSettings::new()
            .set_conn_timeout(timeout)
            .set_starttls(self.config.start_tls)
            .set_no_tls_verify(!self.config.verify_ldap_host);
        let (conn, mut ldap) = tokio::time::timeout(timeout, LdapConnAsync::with_settings(settings, &self.ldap_url))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|e| LdapAuthError::ConnectionFailed(e.to_string()))?;

        ldap3::drive!(conn);

        let bind_result = tokio::time::timeout(timeout, ldap.simple_bind(bind_dn, password))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|e| LdapAuthError::BindFailed(e.to_string()))?;

        let _ = ldap.unbind().await;

        if bind_result.rc == 0 {
            Ok(())
        } else if bind_result.rc == 49 {
            Err(LdapAuthError::InvalidCredentials)
        } else {
            Err(LdapAuthError::BindFailed(format!(
                "server returned rc={}",
                bind_result.rc
            )))
        }
    }

    fn apply_authenticated_headers(&self, session: &mut dyn PluginSession, username: &str) {
        let _ = session.set_request_header(&self.config.credential_identifier_header, username);
        let _ = session.remove_request_header(&self.config.anonymous_header);
    }

    fn apply_anonymous_headers(&self, session: &mut dyn PluginSession, anonymous_user: &str) {
        let _ = session.set_request_header(&self.config.credential_identifier_header, anonymous_user);
        let _ = session.set_request_header(&self.config.anonymous_header, "true");
    }

    fn hide_credentials_if_needed(&self, session: &mut dyn PluginSession) {
        if self.config.hide_credentials {
            let _ = session.remove_request_header("authorization");
            let _ = session.remove_request_header("proxy-authorization");
        }
    }

    async fn unauthorized(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = send_auth_error_response(
            session,
            401,
            &self.config.header_type,
            &self.config.realm,
            "Unauthorized",
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn service_unavailable(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(503, None) {
            Ok(v) => v,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };
        let body = Bytes::from("503 An unexpected error occurred");
        let _ = resp.insert_header("Content-Type", "text/plain");

        if session.write_response_header(Box::new(resp), false).await.is_ok() {
            let _ = session.write_response_body(Some(body), true).await;
        }
        session.shutdown().await;
        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for LdapAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let (username, password) = match self.parse_credentials(session) {
            ParseAuthResult::Valid(u, p) => (u, p),
            ParseAuthResult::Missing => {
                if let Some(anon_user) = &self.config.anonymous {
                    self.apply_anonymous_headers(session, anon_user);
                    self.hide_credentials_if_needed(session);
                    plugin_log.push("Anonymous; ");
                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Auth failed; ");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
            ParseAuthResult::Invalid => {
                plugin_log.push("Auth failed; ");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
        };

        if let Some(cached_username) = self.check_cache(&username, &password) {
            self.apply_authenticated_headers(session, &cached_username);
            self.hide_credentials_if_needed(session);
            plugin_log.push(&format!("Auth cached user={}; ", cached_username));
            return PluginRunningResult::GoodNext;
        }

        let bind_dn = match self.build_bind_dn(&username) {
            Ok(dn) => dn,
            Err(_) => {
                plugin_log.push("Auth failed; ");
                return self.unauthorized(session).await;
            }
        };

        match self.ldap_bind(&bind_dn, &password).await {
            Ok(()) => {
                self.store_cache(&username, &password);
                self.apply_authenticated_headers(session, &username);
                self.hide_credentials_if_needed(session);
                plugin_log.push(&format!("Auth user={}; ", username));
                PluginRunningResult::GoodNext
            }
            Err(LdapAuthError::InvalidCredentials) => {
                plugin_log.push("Auth failed; ");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                self.unauthorized(session).await
            }
            Err(LdapAuthError::ConnectionFailed(e)) => {
                tracing::warn!(error = %e, ldap_url = %self.ldap_url, "LdapAuth connection failed");
                plugin_log.push("LDAP unavailable; ");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::Timeout) => {
                tracing::warn!(ldap_url = %self.ldap_url, "LdapAuth timeout");
                plugin_log.push("LDAP timeout; ");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::BindFailed(e)) => {
                tracing::warn!(error = %e, ldap_url = %self.ldap_url, "LdapAuth bind failed");
                plugin_log.push("LDAP unavailable; ");
                self.service_unavailable(session).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;

    fn test_config() -> LdapAuthConfig {
        LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            ..Default::default()
        }
    }

    fn ldap_auth_default() -> LdapAuth {
        LdapAuth::new(&test_config())
    }

    fn encode_creds(username: &str, password: &str) -> String {
        format!(
            "ldap {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", username, password))
        )
    }

    #[test]
    fn test_parse_authorization_value_ok() {
        let auth = ldap_auth_default();
        let header = encode_creds("alice", "secret");
        let parsed = auth.parse_authorization_value(&header);
        assert_eq!(parsed, Some(("alice".to_string(), "secret".to_string())));
    }

    #[test]
    fn test_parse_authorization_value_password_with_colon() {
        let auth = ldap_auth_default();
        let header = format!(
            "ldap {}",
            base64::engine::general_purpose::STANDARD.encode("alice:a:b:c")
        );
        let parsed = auth.parse_authorization_value(&header);
        assert_eq!(parsed, Some(("alice".to_string(), "a:b:c".to_string())));
    }

    #[test]
    fn test_parse_authorization_value_invalid() {
        let auth = ldap_auth_default();
        assert_eq!(auth.parse_authorization_value("Basic abc"), None);
        assert_eq!(auth.parse_authorization_value("ldap !!!"), None);
        assert_eq!(auth.parse_authorization_value("ldap"), None);
    }

    #[test]
    fn test_build_bind_dn_default() {
        let auth = ldap_auth_default();
        let dn = auth.build_bind_dn("alice").unwrap();
        assert_eq!(dn, "uid=alice,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_template() {
        let mut cfg = test_config();
        cfg.bind_dn_template = Some("cn={username},ou=users,dc=example,dc=com".to_string());
        let auth = LdapAuth::new(&cfg);
        let dn = auth.build_bind_dn("alice").unwrap();
        assert_eq!(dn, "cn=alice,ou=users,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_injection_rejected() {
        let auth = ldap_auth_default();
        assert!(auth.build_bind_dn("alice,admin").is_err());
        assert!(auth.build_bind_dn("alice+admin").is_err());
    }

    #[test]
    fn test_cache_hit_and_expired() {
        let auth = ldap_auth_default();

        auth.store_cache("alice", "secret");
        assert_eq!(auth.check_cache("alice", "secret"), Some("alice".to_string()));

        let key = LdapAuth::cache_key("bob", "secret");
        auth.auth_cache
            .insert(key, ("bob".to_string(), Instant::now() - Duration::from_secs(1)));
        assert_eq!(auth.check_cache("bob", "secret"), None);
    }

    #[tokio::test]
    async fn test_parse_credentials_proxy_priority() {
        let auth = ldap_auth_default();
        let mut session = MockPluginSession::new();

        let proxy = encode_creds("proxy-user", "proxy-pass");

        session
            .expect_header_value()
            .with(mockall::predicate::eq("proxy-authorization"))
            .return_once(move |_| Some(proxy));

        session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .return_once(|_| Some(encode_creds("auth-user", "auth-pass")));

        let result = auth.parse_credentials(&session);
        assert_eq!(
            result,
            ParseAuthResult::Valid("proxy-user".to_string(), "proxy-pass".to_string())
        );
    }

    #[tokio::test]
    async fn test_run_request_anonymous_missing_header() {
        let mut cfg = test_config();
        cfg.anonymous = Some("anonymous-user".to_string());
        cfg.hide_credentials = true;
        let auth = LdapAuth::new(&cfg);

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("LdapAuth");

        session
            .expect_header_value()
            .with(mockall::predicate::eq("proxy-authorization"))
            .return_once(|_| None);
        session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .return_once(|_| None);

        session
            .expect_set_request_header()
            .with(
                mockall::predicate::eq("X-Credential-Identifier"),
                mockall::predicate::eq("anonymous-user"),
            )
            .returning(|_, _| Ok(()));
        session
            .expect_set_request_header()
            .with(
                mockall::predicate::eq("X-Anonymous-Consumer"),
                mockall::predicate::eq("true"),
            )
            .returning(|_, _| Ok(()));

        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq("authorization"))
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq("proxy-authorization"))
            .returning(|_| Ok(()));

        let result = auth.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Anonymous"));
    }

    #[tokio::test]
    async fn test_run_request_invalid_header_without_anonymous() {
        let auth = ldap_auth_default();
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("LdapAuth");

        session
            .expect_header_value()
            .with(mockall::predicate::eq("proxy-authorization"))
            .return_once(|_| None);
        session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .return_once(|_| Some("ldap invalid-base64".to_string()));

        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = auth.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[tokio::test]
    async fn test_run_request_invalid_header_with_anonymous_still_401() {
        let mut cfg = test_config();
        cfg.anonymous = Some("guest".to_string());
        let auth = LdapAuth::new(&cfg);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("LdapAuth");

        session
            .expect_header_value()
            .with(mockall::predicate::eq("proxy-authorization"))
            .return_once(|_| None);
        session
            .expect_header_value()
            .with(mockall::predicate::eq("authorization"))
            .return_once(|_| Some("ldap invalid-base64".to_string()));

        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let result = auth.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }
}
