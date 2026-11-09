//! JWT Auth plugin configuration
//!
//! Supports verification of JWT in header, query, or cookie.
//! Routing: peek iss claim → route to matching issuer_group → verify with
//! that group's keys/algorithm/aud (closes issuer-key decoupling vulnerability).

use crate::resources::common::group_name::default_group_name;

crate::redact::impl_redacted_debug!(JwtAuthConfig, ResolvedJwtCredential, ResolvedJwtIssuerGroup);
use crate::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Resolved JWT credential data (populated by controller from Secret)
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJwtCredential {
    /// Payload credential-key value used to look up the credential within a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Symmetric secret (for HS* algorithms), read as UTF-8 from the Secret.
    /// After Kubernetes Secret decoding, when `base64_secret` is enabled, this
    /// UTF-8 string itself wraps the actual key bytes in standard Base64.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Public key PEM (for RS*/ES* algorithms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// JWT signature algorithm
///
/// Symmetric: HS256, HS384, HS512 (HMAC with SHA-256/384/512)
/// Asymmetric RSA: RS256, RS384, RS512 (RSASSA-PKCS1-v1_5 with SHA-256/384/512)
/// Asymmetric ECDSA: ES256, ES384 (ECDSA with P-256/P-384)
///
/// Note: ES512 (P-521) is not supported by the underlying jsonwebtoken library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub enum JwtAlgorithm {
    /// HMAC using SHA-256
    #[default]
    HS256,
    /// HMAC using SHA-384
    HS384,
    /// HMAC using SHA-512
    HS512,
    /// RSASSA-PKCS1-v1_5 using SHA-256
    RS256,
    /// RSASSA-PKCS1-v1_5 using SHA-384
    RS384,
    /// RSASSA-PKCS1-v1_5 using SHA-512
    RS512,
    /// ECDSA using P-256 and SHA-256
    ES256,
    /// ECDSA using P-384 and SHA-384
    ES384,
}

/// A named group of issuers + Secret refs whose credentials all carry this
/// group's name. Token routing is by `iss` claim; verification is scoped
/// strictly to this group's keys and (snapshot-merged) algorithm + aud.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JwtIssuerGroup {
    /// Group name; charset per K8s label-value
    /// ([`crate::gwapi_types::GROUP_NAME_PATTERN`], <=63 chars).
    /// Surfaces in PluginLog as `group=<name>`.
    #[schemars(
        length(max = crate::gwapi_types::GROUP_NAME_MAX_LEN),
        pattern(crate::gwapi_types::GROUP_NAME_PATTERN)
    )]
    pub name: String,

    /// Human-readable description (not used by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Issuers this group covers. At least one; cross-group uniqueness
    /// enforced at preparse (this is the routing key).
    pub issuers: Vec<String>,

    /// Secret references for this group. Must be non-empty at preparse.
    pub secret_refs: Vec<SecretObjectReference>,

    /// Override the plugin-level `algorithm`. `None` = use plugin-level.
    /// Snapshot-merged at controller resolve time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<JwtAlgorithm>,

    /// Override the plugin-level `audiences`. `None` = use plugin-level.
    /// `Some(vec![])` is treated as "no aud check for this group".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audiences: Option<Vec<String>>,
}

/// Snapshot-merged group (controller-populated). Always carries concrete
/// `algorithm` and the per-payload-credential-key map for this group.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJwtIssuerGroup {
    /// Group name (snapshot of `JwtIssuerGroup.name`). Always set by
    /// controller; `#[serde(default)]` provides a fallback for
    /// Gateway-new / Controller-old rolling upgrades.
    #[serde(default = "default_group_name")]
    pub group: String,

    /// Snapshot-merged: group's `algorithm` if set, else plugin-level default.
    /// Always a concrete value at runtime — gateway does NO fallback here.
    pub algorithm: JwtAlgorithm,

    /// Snapshot-merged: group's `audiences` if set, else plugin-level.
    /// `None` = no aud check; `Some(vec![])` = explicit "no check" override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audiences: Option<Vec<String>>,

    /// Credentials for this group, keyed by the payload credential-key value
    /// stored in the Secret's `key` field.
    pub credentials: HashMap<String, ResolvedJwtCredential>,
}

/// JWT Auth plugin configuration (route/service level)
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JwtAuthConfig {
    /// Algorithm (default: HS256). Applied at plugin level; each group may override.
    #[serde(default)]
    pub algorithm: JwtAlgorithm,

    /// Header name to read token from (default: "authorization"; expect "Bearer <token>" or raw token)
    #[serde(default = "default_header")]
    pub header: String,

    /// Query parameter name (default: "jwt")
    #[serde(default = "default_query")]
    pub query: String,

    /// Allow reading the token from the URL query parameter (default: false).
    /// RFC 6750 §2.3/§5.3 advise against bearer tokens in URLs (they leak into
    /// access logs, browser history, and Referer headers), so the query path is
    /// opt-in. When false, the `query` field is ignored during extraction.
    #[serde(default)]
    pub allow_token_in_query: bool,

    /// Cookie name (default: "jwt")
    #[serde(default = "default_cookie")]
    pub cookie: String,

    /// If true, do not forward the token (header/query/cookie) to upstream
    #[serde(default)]
    pub hide_credentials: bool,

    /// Anonymous consumer name; if set, requests without a JWT are allowed and this name is set.
    /// A present but invalid JWT is always rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Claim name in JWT payload that identifies the credential key (default: "key")
    #[serde(default = "default_key_claim_name")]
    pub key_claim_name: String,

    /// Grace period in seconds for exp/nbf (clock skew tolerance)
    #[serde(default)]
    pub lifetime_grace_period: u64,

    /// List of valid audiences (aud claim) at plugin level.
    /// Each group may override. If empty, audience is not validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audiences: Option<Vec<String>>,

    /// Realm for WWW-Authenticate header (default: "jwt")
    #[serde(default = "default_realm")]
    pub realm: String,

    /// If true, decode the Controller-provided UTF-8 secret string as standard
    /// Base64 after Kubernetes has already decoded the outer Secret `.data` layer.
    /// If false, use the UTF-8 bytes directly.
    #[serde(default)]
    pub base64_secret: bool,

    /// Maximum allowed expiration time in seconds from now.
    /// If set, tokens with exp claim further than this in the future will be rejected.
    /// 0 means no limit. (e.g., 86400 = 1 day max)
    #[serde(default)]
    pub maximum_expiration: u64,

    /// Delay in milliseconds before returning 401 on auth failure.
    /// Helps prevent timing attacks. 0 means no delay.
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// If true, store decoded JWT claims in context variable "jwt_claims" as JSON string.
    /// Downstream plugins can access via get_ctx_var("jwt_claims").
    #[serde(default)]
    pub store_claims_in_ctx: bool,

    /// Map JWT claims to request headers for upstream. Destination header names
    /// are case-insensitive and must be unique within this plugin instance.
    /// Key is the claim name (e.g., "sub", "email"), value is the header name (e.g., "X-User-ID").
    /// Example: {"sub": "X-User-ID", "email": "X-User-Email"}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims_to_headers: Option<HashMap<String, String>>,

    /// Issuer groups. Optional so anonymous-only deployments remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_groups: Option<Vec<JwtIssuerGroup>>,

    // === Runtime fields (populated by controller, not user-configurable) ===
    /// Snapshot-merged groups indexed by `iss` (one entry per iss claim).
    /// Built by controller from `issuer_groups` + plugin-level defaults.
    /// Multiple iss values from the same group each hold their own copy
    /// (clone, not Arc, to keep serde simple).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_groups_by_iss: Option<HashMap<String, ResolvedJwtIssuerGroup>>,
}

fn default_header() -> String {
    "authorization".to_string()
}
fn default_query() -> String {
    "jwt".to_string()
}
fn default_cookie() -> String {
    "jwt".to_string()
}
fn default_key_claim_name() -> String {
    "key".to_string()
}
fn default_realm() -> String {
    "jwt".to_string()
}

impl Default for JwtAuthConfig {
    fn default() -> Self {
        Self {
            algorithm: JwtAlgorithm::default(),
            header: default_header(),
            query: default_query(),
            allow_token_in_query: false,
            cookie: default_cookie(),
            hide_credentials: false,
            anonymous: None,
            key_claim_name: default_key_claim_name(),
            lifetime_grace_period: 0,
            audiences: None,
            realm: default_realm(),
            base64_secret: false,
            maximum_expiration: 0,
            auth_failure_delay_ms: 0,
            store_claims_in_ctx: false,
            claims_to_headers: None,
            issuer_groups: None,
            resolved_groups_by_iss: None,
        }
    }
}

impl JwtAuthConfig {
    pub fn compile_claim_header_projection(&self) -> Result<super::ClaimHeaderProjectionPlan, &'static str> {
        super::auth_header_projection::compile_authenticated_claim_header_projection(
            self.claims_to_headers.as_ref(),
            &[],
        )
    }
}

use crate::resources::edgion_plugins::auth_realm::{validate_anonymous_header_text, validate_auth_realm};
use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for JwtAuthConfig {
    fn validation_error(&self) -> Option<String> {
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error.to_string());
        }
        // 1. issuer_groups structural validation
        if let Some(groups) = &self.issuer_groups {
            if groups.is_empty() {
                return Some("issuerGroups cannot be empty when present".to_string());
            }
            let mut seen_names: HashSet<&str> = HashSet::new();
            let mut seen_issuers: HashSet<&str> = HashSet::new();
            for g in groups {
                if let Err(e) = crate::resources::common::group_name::validate_group_name(&g.name) {
                    return Some(format!("issuerGroups: {}", e));
                }
                if !seen_names.insert(g.name.as_str()) {
                    return Some(format!("duplicate group name '{}'", g.name));
                }
                if g.issuers.is_empty() {
                    return Some(format!("group '{}' has no issuers", g.name));
                }
                if g.secret_refs.is_empty() {
                    return Some(format!("group '{}' has no secretRefs", g.name));
                }
                // Cross-group iss uniqueness — critical for routing correctness.
                // (audiences are NOT cross-group unique — legitimate sharing.)
                for iss in &g.issuers {
                    if iss.trim().is_empty() {
                        return Some(format!("group '{}' has empty issuer", g.name));
                    }
                    if !seen_issuers.insert(iss.as_str()) {
                        return Some(format!(
                            "issuer '{}' appears in multiple groups; iss must be unique across groups",
                            iss
                        ));
                    }
                }
            }
        }

        // 2. Token source fields must not be empty
        if self.header.trim().is_empty() {
            return Some("header must not be empty".to_string());
        }
        if self.query.trim().is_empty() {
            return Some("query must not be empty".to_string());
        }
        if self.cookie.trim().is_empty() {
            return Some("cookie must not be empty".to_string());
        }

        if let Err(error) = self.compile_claim_header_projection() {
            return Some(error.to_string());
        }

        // The anonymous name is written to X-Consumer-Username; a value that is
        // not header text could never be forwarded and would deny every
        // credential-less request.
        if let Err(error) = validate_anonymous_header_text(self.anonymous.as_deref()) {
            return Some(error.to_string());
        }

        // 3. Final gate: anonymous or groups required
        let has_groups = self.issuer_groups.as_ref().is_some_and(|v| !v.is_empty());
        if self.anonymous.is_none() && !has_groups {
            return Some("issuerGroups is required when anonymous is not set".to_string());
        }

        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/jwt_auth_test.rs"]
mod tests;
