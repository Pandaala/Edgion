//! LDAP Auth plugin configuration

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// LDAP Authentication plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LdapAuthConfig {
    // === LDAP Server Connection ===
    /// LDAP server hostname or IP address
    pub ldap_host: String,

    /// LDAP server port (default: 389, use 636 for LDAPS)
    #[serde(default = "default_ldap_port")]
    pub ldap_port: u16,

    /// Enable LDAPS (LDAP over TLS)
    #[serde(default)]
    pub ldaps: bool,

    /// Enable StartTLS on plain LDAP connection
    #[serde(default)]
    pub start_tls: bool,

    /// Verify LDAP server certificate hostname (default: true)
    #[serde(default = "default_verify_ldap_host")]
    pub verify_ldap_host: bool,

    // === LDAP Search / Bind ===
    /// Base DN for LDAP bind DN construction
    pub base_dn: String,

    /// LDAP attribute for username binding (for example: uid, cn)
    pub attribute: String,

    /// Custom bind DN template, must contain {username}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_dn_template: Option<String>,

    // === Authentication Behavior ===
    /// Authorization scheme type in header (default: ldap)
    #[serde(default = "default_header_type")]
    pub header_type: String,

    /// Remove authorization headers before forwarding to upstream
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// Username used for anonymous pass-through when no credentials
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Realm for WWW-Authenticate
    #[serde(default = "default_realm")]
    pub realm: String,

    // === Performance & Caching ===
    /// Authentication result cache TTL in seconds. 0 disables cache.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u64,

    /// LDAP connect/bind timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Reserved keepalive setting in milliseconds
    #[serde(default = "default_keepalive")]
    pub keepalive: u64,

    // === Upstream Headers ===
    /// Header for authenticated identity
    #[serde(default = "default_credential_identifier_header")]
    pub credential_identifier_header: String,

    /// Header set to true for anonymous requests
    #[serde(default = "default_anonymous_header")]
    pub anonymous_header: String,

    // === Validation ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_ldap_port() -> u16 {
    389
}

fn default_verify_ldap_host() -> bool {
    true
}

fn default_header_type() -> String {
    "ldap".to_string()
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

fn default_cache_ttl() -> u64 {
    60
}

fn default_timeout() -> u64 {
    10_000
}

fn default_keepalive() -> u64 {
    60_000
}

fn default_credential_identifier_header() -> String {
    "X-Credential-Identifier".to_string()
}

fn default_anonymous_header() -> String {
    "X-Anonymous-Consumer".to_string()
}

impl Default for LdapAuthConfig {
    fn default() -> Self {
        Self {
            ldap_host: String::new(),
            ldap_port: default_ldap_port(),
            ldaps: false,
            start_tls: false,
            verify_ldap_host: default_verify_ldap_host(),
            base_dn: String::new(),
            attribute: String::new(),
            bind_dn_template: None,
            header_type: default_header_type(),
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            anonymous: None,
            realm: default_realm(),
            cache_ttl: default_cache_ttl(),
            timeout: default_timeout(),
            keepalive: default_keepalive(),
            credential_identifier_header: default_credential_identifier_header(),
            anonymous_header: default_anonymous_header(),
            validation_error: None,
        }
    }
}

impl LdapAuthConfig {
    /// Validate configuration and return error message when invalid.
    pub fn get_validation_error(&self) -> Option<&str> {
        if self.ldap_host.trim().is_empty() {
            return Some("ldapHost is required");
        }
        if self.base_dn.trim().is_empty() {
            return Some("baseDn is required");
        }
        if self.attribute.trim().is_empty() {
            return Some("attribute is required");
        }
        if self.ldaps && self.start_tls {
            return Some("ldaps and startTls cannot both be true");
        }
        if self.header_type.trim().is_empty() {
            return Some("headerType cannot be empty");
        }
        if self.timeout == 0 {
            return Some("timeout must be greater than 0");
        }
        if let Some(template) = &self.bind_dn_template {
            if !template.contains("{username}") {
                return Some("bindDnTemplate must contain {username}");
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = LdapAuthConfig::default();
        assert_eq!(cfg.ldap_port, 389);
        assert!(!cfg.ldaps);
        assert!(!cfg.start_tls);
        assert!(cfg.verify_ldap_host);
        assert_eq!(cfg.header_type, "ldap");
        assert_eq!(cfg.realm, "API Gateway");
        assert_eq!(cfg.cache_ttl, 60);
        assert_eq!(cfg.timeout, 10_000);
        assert_eq!(cfg.credential_identifier_header, "X-Credential-Identifier");
        assert_eq!(cfg.anonymous_header, "X-Anonymous-Consumer");
    }

    #[test]
    fn test_validation_required_fields() {
        let cfg = LdapAuthConfig::default();
        assert_eq!(cfg.get_validation_error(), Some("ldapHost is required"));
    }

    #[test]
    fn test_validation_ldaps_starttls_conflict() {
        let cfg = LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            ldaps: true,
            start_tls: true,
            ..Default::default()
        };
        assert_eq!(
            cfg.get_validation_error(),
            Some("ldaps and startTls cannot both be true")
        );
    }

    #[test]
    fn test_validation_bind_dn_template() {
        let cfg = LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            bind_dn_template: Some("uid=user,dc=example,dc=com".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.get_validation_error(),
            Some("bindDnTemplate must contain {username}")
        );
    }

    #[test]
    fn test_validation_ok() {
        let cfg = LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            bind_dn_template: Some("uid={username},dc=example,dc=com".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.get_validation_error(), None);
    }
}
