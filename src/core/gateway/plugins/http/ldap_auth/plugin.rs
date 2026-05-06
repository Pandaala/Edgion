//! LDAP Authentication plugin implementation

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use ldap3::{LdapConnAsync, LdapConnSettings};
use pingora_http::ResponseHeader;
use sha2::{Digest, Sha256};

use crate::core::gateway::cache::ShardedLruCache;
use crate::core::gateway::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use crate::core::gateway::observe::metrics::registry::{record_ldap_auth_error, set_ldap_cache_hit_rate};
use crate::core::gateway::plugins::http::common::auth_common::{apply_auth_failure_delay, send_auth_error_response};
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::LdapAuthConfig;

/// LDAP Authentication plugin
pub struct LdapAuth {
    name: String,
    config: LdapAuthConfig,
    ldap_url: String,
    auth_cache: Arc<ShardedLruCache<String, String>>,
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

        // Background hit-ratio monitor: runs every 30 s, logs only on transitions.
        // Uses a Weak reference so the task exits automatically when the plugin is dropped.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let cache_monitor = Arc::downgrade(&auth_cache);
            handle.spawn(async move {
                const LOW_RATIO: f64 = 0.5;
                const MIN_SAMPLES: u64 = 100;
                let mut was_low = false;
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let Some(cache) = cache_monitor.upgrade() else {
                        break;
                    };
                    let stats = cache.stats();
                    let total = stats.hits + stats.misses + stats.expirations;
                    if total < MIN_SAMPLES {
                        continue;
                    }
                    let ratio = stats.hit_ratio();
                    set_ldap_cache_hit_rate(ratio);
                    let is_low = ratio < LOW_RATIO;
                    if is_low && !was_low {
                        tracing::info!(hit_ratio = ratio, "LDAP cache hit ratio low; possible brute-force");
                        was_low = true;
                    } else if !is_low && was_low {
                        tracing::info!(hit_ratio = ratio, "LDAP cache hit ratio recovered");
                        was_low = false;
                    }
                }
            });
        }

        Self {
            name: "LdapAuth".to_string(),
            config: config.clone(),
            ldap_url,
            auth_cache,
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
        // Allowlist: only permit characters safe in LDAP DN attribute values.
        // This prevents LDAP injection via DN special chars (RFC 4514).
        if username.is_empty()
            || !username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '@' | '-' | '_'))
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
                plugin_log.push("Deny bad password");
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
    use crate::types::resources::edgion_plugins::LruCacheConfig;

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
    fn test_build_bind_dn_injection_rejected() {
        let auth = ldap_auth_default();
        assert!(auth.build_bind_dn("alice,admin").is_err());
        assert!(auth.build_bind_dn("alice+admin").is_err());
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
}
