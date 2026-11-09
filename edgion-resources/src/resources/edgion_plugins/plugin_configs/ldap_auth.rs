//! LDAP Auth plugin configuration

use crate::resources::common::OutboundTlsLocal;
use crate::resources::gateway::SecretObjectReference;

crate::redact::impl_redacted_debug!(LdapAuthConfig);
use crate::resources::edgion_plugins::auth_realm::{validate_anonymous_header_text, validate_auth_realm};
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

    // === Search mode ===
    /// Service-account entry DN. Setting it selects search mode: bind as the
    /// service account, search for the user entry, then bind as that entry.
    /// Must be an entry DN (not a UPN or down-level name); requires
    /// `bindPasswordRef` and `ldaps` or `startTls` with certificate verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_dn: Option<String>,

    /// Secret holding the service-account password under the key `password`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_password_ref: Option<SecretObjectReference>,

    /// RFC 4515 search filter containing `{username}` (default:
    /// `(<attribute>={username})`). The username is inserted as raw assertion
    /// bytes into the compiled filter, so it cannot change the filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_filter: Option<String>,

    /// Search base DN (default: `baseDn`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_base: Option<String>,

    // === Group requirements (search mode) ===
    /// Group DNs of which the user must belong to at least one (1..=32).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_group_list"
    )]
    pub required_groups_any_of: Option<Vec<String>>,

    /// Group DNs of which the user must belong to every one (1..=32). With
    /// `requiredGroupsAnyOf` both requirements must hold.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_group_list"
    )]
    pub required_groups_all_of: Option<Vec<String>>,

    /// Attribute on the user entry that lists group DNs (default: `memberOf`).
    /// The directory must maintain it and users must not be able to write it.
    #[serde(
        default = "default_group_attribute",
        deserialize_with = "deserialize_group_attribute"
    )]
    pub group_attribute: String,

    /// Match nested groups with the Active Directory in-chain rule
    /// `1.2.840.113556.1.4.1941` (not implemented by OpenLDAP).
    #[serde(default)]
    pub nested_groups: bool,

    /// Resolved service-account password (from `bindPasswordRef`; populated by the Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_bind_password: Option<String>,

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

/// A present `null` (what an empty templated value renders to) reads as an
/// empty list and a `null` item as an empty entry; validation rejects both
/// with a per-resource status. They must neither mean "no requirement" nor
/// fail decoding: a decode error would drop the whole EdgionPlugins object
/// (and break the Kubernetes list).
fn deserialize_group_list<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<String>>, D::Error> {
    let groups = Option::<Vec<Option<String>>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(Some(groups.into_iter().map(Option::unwrap_or_default).collect()))
}

/// A `null` `groupAttribute` (an empty templated value) means the default.
fn deserialize_group_attribute<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_group_attribute))
}

fn default_group_attribute() -> String {
    "memberOf".to_string()
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
            bind_dn: None,
            bind_password_ref: None,
            search_filter: None,
            search_base: None,
            required_groups_any_of: None,
            required_groups_all_of: None,
            group_attribute: default_group_attribute(),
            nested_groups: false,
            resolved_bind_password: None,
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

/// Most groups in each of `requiredGroupsAnyOf` and `requiredGroupsAllOf`.
pub const MAX_REQUIRED_GROUPS: usize = 32;
// The validation message spells the bound out.
const _: () = assert!(MAX_REQUIRED_GROUPS == 32);
/// Longest group DN (bytes, after trimming).
pub const MAX_GROUP_DN_LEN: usize = 1024;
/// Longest `groupAttribute` (bytes, after trimming).
pub const MAX_GROUP_ATTRIBUTE_LEN: usize = 256;

impl LdapAuthConfig {
    /// Secret data keys accepted for `bindPasswordRef` (shared by the
    /// Controller resolver and its status check).
    pub const BIND_PASSWORD_KEYS: &'static [&'static str] = &["password"];

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
        if http::HeaderName::from_bytes(self.credential_identifier_header.as_bytes()).is_err() {
            return Some("credentialIdentifierHeader is not a valid header name");
        }
        if http::HeaderName::from_bytes(self.anonymous_header.as_bytes()).is_err() {
            return Some("anonymousHeader is not a valid header name");
        }
        if let Err(error) = validate_anonymous_header_text(self.anonymous.as_deref()) {
            return Some(error);
        }
        if let Some(error) = self.search_mode_error() {
            return Some(error);
        }

        None
    }
}

impl LdapAuthConfig {
    /// The effective search filter: the configured one or `(<attribute>={username})`.
    pub fn effective_search_filter(&self) -> String {
        self.search_filter
            .clone()
            .unwrap_or_else(|| format!("({}={{username}})", self.attribute.trim()))
    }

    fn search_mode_error(&self) -> Option<&'static str> {
        let Some(bind_dn) = &self.bind_dn else {
            if self.search_filter.is_some() || self.search_base.is_some() || self.bind_password_ref.is_some() {
                return Some("searchFilter, searchBase and bindPasswordRef require bindDn");
            }
            if self.has_group_requirements() {
                return Some("requiredGroupsAnyOf and requiredGroupsAllOf require bindDn");
            }
            return None;
        };
        if bind_dn.trim().is_empty() {
            return Some("bindDn cannot be empty");
        }
        if self.bind_dn_template.is_some() {
            return Some("bindDn and bindDnTemplate cannot both be set");
        }
        if self.bind_password_ref.is_none() {
            return Some("bindPasswordRef is required with bindDn");
        }
        if !self.ldaps && !self.start_tls {
            return Some("bindDn requires ldaps or startTls");
        }
        if self.search_base.as_deref().is_some_and(|base| base.trim().is_empty()) {
            return Some("searchBase cannot be empty");
        }
        // A non-DN base would answer every request with a 503 at run time.
        if !edgion_ldap::dn::is_valid(self.search_base.as_deref().unwrap_or(&self.base_dn)) {
            return Some("searchBase (or baseDn when unset) is not a valid DN");
        }
        // The string form keeps the service-account guard's comparison with
        // the directory's spelling of the DN reliable.
        if !edgion_ldap::dn::is_string_form(bind_dn) {
            return Some("bindDn must be a DN in string form (attribute names, no #hex values)");
        }
        if !edgion_ldap::filter::is_attribute_description(self.attribute.trim()) {
            return Some("attribute is not a valid attribute name");
        }
        let san_policy = self
            .tls
            .validation
            .as_ref()
            .and_then(|validation| validation.subject_alt_names.as_ref())
            .is_some_and(|sans| !sans.is_empty());
        if self.tls.verify == Some(false) && !san_policy {
            return Some("bindDn requires TLS certificate verification");
        }
        let filter = self.effective_search_filter();
        if !filter.contains("{username}") {
            return Some("searchFilter must contain {username}");
        }
        if edgion_ldap::filter::SearchFilter::compile(&filter).is_err() {
            return Some("searchFilter is invalid");
        }
        self.group_requirements_error()
    }

    /// Whether a group requirement is configured.
    pub fn has_group_requirements(&self) -> bool {
        self.required_groups_any_of.is_some() || self.required_groups_all_of.is_some()
    }

    fn group_requirements_error(&self) -> Option<&'static str> {
        if !self.has_group_requirements() {
            return None;
        }
        let lists = [&self.required_groups_any_of, &self.required_groups_all_of];
        for groups in lists.into_iter().flatten() {
            if groups.is_empty() || groups.len() > MAX_REQUIRED_GROUPS {
                return Some("requiredGroupsAnyOf and requiredGroupsAllOf need 1 to 32 groups each");
            }
            for group in groups {
                let group = group.trim();
                if group.len() > MAX_GROUP_DN_LEN || !edgion_ldap::dn::is_valid(group) {
                    return Some("requiredGroupsAnyOf and requiredGroupsAllOf entries must be valid DNs");
                }
            }
        }
        let attribute = self.group_attribute.trim();
        if attribute.len() > MAX_GROUP_ATTRIBUTE_LEN || !edgion_ldap::filter::is_attribute_description(attribute) {
            return Some("groupAttribute is not a valid attribute name");
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
