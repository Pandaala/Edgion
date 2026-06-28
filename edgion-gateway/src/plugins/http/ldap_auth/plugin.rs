//! LDAP Authentication plugin implementation

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use ldap3::{LdapConnAsync, LdapConnSettings};
use sha2::{Digest, Sha256};

use crate::cache::ShardedLruCache;
use crate::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use crate::observe::metrics::registry::record_ldap_auth_error;
use crate::plugins::http::common::auth_common::{
    apply_auth_failure_delay, send_gateway_auth_error_response, send_gateway_plain_error_response,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::LdapAuthConfig;

/// LDAP Authentication plugin
pub struct LdapAuth {
    name: String,
    config: LdapAuthConfig,
    ldap_url: String,
    auth_cache: Arc<ShardedLruCache<String, String>>,
    /// Effective TLS verification flag (resolved from config.verify + global default).
    tls_verify: bool,
}

#[derive(Debug)]
enum LdapAuthError {
    InvalidCredentials,
    ConnectionFailed,
    Timeout,
    BindFailed,
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
        let auth_cache = Arc::new(ShardedLruCache::new(capacity, shards));

        // Resolve the effective verify flag. CA/mTLS are out of scope for LDAP
        // (deferred); only the verify tri-state is wired here. provider_can_disable=true
        // because ldap3 supports set_no_tls_verify. Operator-set verify=false is honored;
        // the shared resolver already emits a WARN at config-load time.
        use edgion_resources::resources::common::OutboundTlsLocal;
        let tls_verify = match edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls(
            &OutboundTlsLocal {
                verify: config.verify,
                ..Default::default()
            },
            None,
            None,
            true,
            "LdapAuth",
        ) {
            Ok(r) => r.verify,
            Err(e) => {
                // Only reachable when provider_can_disable=false, which is not the case
                // here (ldap3 supports set_no_tls_verify). Fail-closed for safety.
                tracing::warn!(
                    component = "ldap_auth",
                    error = ?e,
                    "outbound TLS resolve error; forcing certificate verification"
                );
                true
            }
        };

        Self {
            name: "LdapAuth".to_string(),
            config: config.clone(),
            ldap_url,
            auth_cache,
            tls_verify,
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
        self.auth_cache.get(&key)
    }

    fn store_cache(&self, username: &str, password: &str) {
        if self.config.cache_ttl == 0 {
            return;
        }
        let key = Self::cache_key(username, password);
        self.auth_cache.insert(
            key,
            username.to_string(),
            Some(Duration::from_secs(self.config.cache_ttl)),
        );
    }

    fn build_bind_dn(&self, username: &str) -> Result<String, &'static str> {
        // Escape per RFC 4514 (DN attribute value): handles injection-relevant
        // metacharacters while accepting legitimate enterprise LDAP usernames
        // (spaces in AD sAMAccountName, unicode CN values, aliases with '+', etc.).
        if username.is_empty() {
            return Err("empty username");
        }
        let escaped = ldap3::dn_escape(username);

        if let Some(template) = &self.config.bind_dn_template {
            return Ok(template.replace("{username}", escaped.as_ref()));
        }

        Ok(format!("{}={},{}", self.config.attribute, escaped, self.config.base_dn))
    }

    async fn ldap_bind(&self, bind_dn: &str, password: &str) -> Result<(), LdapAuthError> {
        let timeout = Duration::from_millis(self.config.timeout);

        let settings = LdapConnSettings::new()
            .set_conn_timeout(timeout)
            .set_starttls(self.config.start_tls)
            .set_no_tls_verify(!self.tls_verify);
        let (conn, mut ldap) = tokio::time::timeout(timeout, LdapConnAsync::with_settings(settings, &self.ldap_url))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|_| LdapAuthError::ConnectionFailed)?;

        ldap3::drive!(conn);

        let bind_result = tokio::time::timeout(timeout, ldap.simple_bind(bind_dn, password))
            .await
            .map_err(|_| LdapAuthError::Timeout)?
            .map_err(|_| LdapAuthError::BindFailed)?;

        let _ = ldap.unbind().await;

        if bind_result.rc == 0 {
            Ok(())
        } else if bind_result.rc == 49 {
            Err(LdapAuthError::InvalidCredentials)
        } else {
            Err(LdapAuthError::BindFailed)
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
        let _ = send_gateway_auth_error_response(
            session,
            401,
            &self.config.header_type,
            &self.config.realm,
            None,
            "Unauthorized",
        )
        .await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn service_unavailable(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = send_gateway_plain_error_response(
            session,
            503,
            "text/plain",
            Bytes::from_static(b"503 An unexpected error occurred"),
        )
        .await;
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
                    plugin_log.push("Allow anonymous");
                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push("Deny no credentials");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
            ParseAuthResult::Invalid => {
                plugin_log.push("Deny invalid header");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                return self.unauthorized(session).await;
            }
        };

        if let Some(cached_username) = self.check_cache(&username, &password) {
            self.apply_authenticated_headers(session, &cached_username);
            self.hide_credentials_if_needed(session);
            plugin_log.push("cache hit");
            plugin_log.push_fmt("OK user {}", la![sensitive(cached_username.as_str())]);
            return PluginRunningResult::GoodNext;
        }

        plugin_log.push("cache miss");

        let bind_dn = match self.build_bind_dn(&username) {
            Ok(dn) => dn,
            Err(_) => {
                plugin_log.push("Deny bad username");
                return self.unauthorized(session).await;
            }
        };

        match self.ldap_bind(&bind_dn, &password).await {
            Ok(()) => {
                self.store_cache(&username, &password);
                self.apply_authenticated_headers(session, &username);
                self.hide_credentials_if_needed(session);
                plugin_log.push_fmt("OK user {}", la![sensitive(&username)]);
                PluginRunningResult::GoodNext
            }
            Err(LdapAuthError::InvalidCredentials) => {
                // Uniform credential-rejection tag, matching basic/key/hmac/jwt_auth.
                // LDAP rc 49 (invalidCredentials) is returned for both a nonexistent DN
                // and a wrong password, so this branch cannot — and must not appear to —
                // discriminate username existence.
                plugin_log.push("Deny bad credential");
                apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
                self.unauthorized(session).await
            }
            Err(LdapAuthError::ConnectionFailed) => {
                plugin_log.push_err("ldap unreachable");
                record_ldap_auth_error("unreachable");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::Timeout) => {
                plugin_log.push_err("ldap timeout");
                record_ldap_auth_error("timeout");
                self.service_unavailable(session).await
            }
            Err(LdapAuthError::BindFailed) => {
                plugin_log.push_err("ldap bind fail");
                record_ldap_auth_error("bind_fail");
                self.service_unavailable(session).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::LruCacheConfig;

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

    #[test]
    fn test_parse_authorization_value_invalid() {
        let auth = ldap_auth_default();
        assert_eq!(auth.parse_authorization_value("Basic abc"), None);
        assert_eq!(auth.parse_authorization_value("ldap !!!"), None);
        assert_eq!(auth.parse_authorization_value("ldap"), None);
    }

    #[test]
    fn test_build_bind_dn_injection_escaped() {
        let auth = ldap_auth_default();

        // RFC 4514 metacharacters must be escaped so injection becomes a
        // literal value inside the RDN. ldap3::dn_escape uses the hex form
        // (\HH); both forms are RFC-equivalent for any compliant server.
        let dn = auth.build_bind_dn("alice,admin").unwrap();
        assert_eq!(dn, r"uid=alice\2cadmin,dc=example,dc=com");

        let dn = auth.build_bind_dn("alice+admin").unwrap();
        assert_eq!(dn, r"uid=alice\2badmin,dc=example,dc=com");

        // Leading '#' and trailing ' ' are also escaped per RFC 4514.
        let dn = auth.build_bind_dn("#bob ").unwrap();
        assert_eq!(dn, r"uid=\23bob\20,dc=example,dc=com");

        // Sanity check: no bare injection-relevant metachar remains inside the
        // attribute value. (`\` is the escape leader produced by dn_escape and
        // is expected to appear; the metachars below would terminate the RDN
        // or change its semantics if they leaked through unescaped.)
        let dn = auth.build_bind_dn(r#"x"y\z<a>b;c=d"#).unwrap();
        let value = dn
            .strip_prefix("uid=")
            .and_then(|s| s.strip_suffix(",dc=example,dc=com"))
            .unwrap();
        for ch in ['"', '<', '>', ';', '=', ',', '+'] {
            assert!(
                !value.contains(ch),
                "bare metachar {:?} leaked into RDN value {:?}",
                ch,
                value
            );
        }
    }

    #[test]
    fn test_build_bind_dn_accepts_spaces() {
        // AD sAMAccountName may contain spaces. Interior spaces are not RFC 4514
        // metacharacters and must pass through unescaped.
        let auth = ldap_auth_default();
        let dn = auth.build_bind_dn("John Smith").unwrap();
        assert_eq!(dn, "uid=John Smith,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_accepts_unicode() {
        // Multi-byte UTF-8 in CN/uid values (e.g. accented Latin) is valid in
        // RFC 4514 attribute values and must not be rejected or escaped.
        let auth = ldap_auth_default();
        let dn = auth.build_bind_dn("José").unwrap();
        assert_eq!(dn, "uid=José,dc=example,dc=com");
    }

    #[test]
    fn test_build_bind_dn_rejects_empty() {
        let auth = ldap_auth_default();
        assert!(auth.build_bind_dn("").is_err());
    }

    #[test]
    fn test_build_bind_dn_template_escapes_username() {
        // The {username} placeholder must be escaped before substitution so a
        // template-based deployment is equally injection-safe.
        let config = LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            bind_dn_template: Some("uid={username},ou=people,dc=example,dc=com".to_string()),
            ..Default::default()
        };
        let auth = LdapAuth::new(&config);
        let dn = auth.build_bind_dn("alice,admin").unwrap();
        assert_eq!(dn, r"uid=alice\2cadmin,ou=people,dc=example,dc=com");
    }

    #[test]
    fn test_ldap_auth_cache_is_lru() {
        let config = LdapAuthConfig {
            cache_ttl: 60,
            lru_cache: LruCacheConfig {
                capacity: 5,
                ..Default::default()
            },
            ldap_host: "localhost".to_string(),
            ldap_port: 389,
            base_dn: "dc=test,dc=com".to_string(),
            ..Default::default()
        };
        let auth = LdapAuth::new(&config);
        assert_eq!(auth.auth_cache.len(), 0);
    }

    /// Guard: verify=false is honored — the operator's explicit choice is respected.
    #[test]
    fn ldap_verify_false_is_honored() {
        let config = LdapAuthConfig {
            verify: Some(false),
            ..test_config()
        };
        let auth = LdapAuth::new(&config);
        assert!(!auth.tls_verify, "verify=false must be honoured when operator sets it");
    }

    /// Guard: verify=true (default) is respected.
    #[test]
    fn ldap_verify_true_by_default() {
        let config = LdapAuthConfig {
            verify: None,
            ..test_config()
        };
        let auth = LdapAuth::new(&config);
        assert!(auth.tls_verify, "verify must default to true when not explicitly set");
    }
}
