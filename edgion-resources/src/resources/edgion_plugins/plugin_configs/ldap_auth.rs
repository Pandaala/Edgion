//! LDAP Auth plugin configuration

use crate::resources::common::OutboundTlsLocal;

crate::redact::impl_redacted_debug!(LdapAuthConfig);
use crate::resources::edgion_plugins::auth_realm::validate_auth_realm;
use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// LDAP Authentication plugin configuration
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Outbound TLS settings (verify, CA trust anchors, client mTLS).
    /// All fields are optional; absent fields inherit from the global outboundTls default.
    #[serde(default)]
    pub tls: OutboundTlsLocal,

    /// Resolved CA Secrets (from `tls.validation.caCertificateRefs`; populated by the Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,

    /// Resolved mTLS client-identity Secret (from `tls.clientCertificateRef`; populated by the Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,

    // === LDAP Search / Bind ===
    /// Base DN for LDAP bind DN construction
    pub base_dn: String,

    /// LDAP attribute for username binding (for example: uid, cn)
    pub attribute: String,

    /// Custom bind DN template, must contain `{username}`.
    /// The `{username}` placeholder is escaped per RFC 4514 (DN context)
    /// before substitution, so the template itself supplies the DN structure
    /// and the user-supplied value cannot inject DN metacharacters.
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

    // === Connection Behavior ===
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
}

fn default_ldap_port() -> u16 {
    389
}

fn default_header_type() -> String {
    "ldap".to_string()
}

fn default_realm() -> String {
    "API Gateway".to_string()
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
            tls: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            base_dn: String::new(),
            attribute: String::new(),
            bind_dn_template: None,
            header_type: default_header_type(),
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            anonymous: None,
            realm: default_realm(),
            timeout: default_timeout(),
            keepalive: default_keepalive(),
            credential_identifier_header: default_credential_identifier_header(),
            anonymous_header: default_anonymous_header(),
        }
    }
}

impl LdapAuthConfig {
    /// Validate configuration and return error message when invalid.
    pub fn get_validation_error(&self) -> Option<&str> {
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error);
        }
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

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for LdapAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.get_validation_error().map(str::to_owned)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ldap_auth_test.rs"]
mod tests;
