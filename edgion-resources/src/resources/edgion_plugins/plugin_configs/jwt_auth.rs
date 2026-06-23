//! JWT Auth plugin configuration
//!
//! Supports verification of JWT in header, query, or cookie.
//! Routing: peek iss claim → route to matching issuer_group → verify with
//! that group's keys/algorithm/aud (closes issuer-key decoupling vulnerability).

use crate::resources::common::group_name::default_group_name;
use crate::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Resolved JWT credential data (populated by controller from Secret)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJwtCredential {
    /// Key identifier (kid) used to look up the credential within a group
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Symmetric secret (for HS* algorithms), base64 encoded
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
    /// (`^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`, <=63 chars).
    /// Surfaces in PluginLog as `group=<name>`.
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
/// `algorithm` and the per-kid credential map for this group.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Credentials for this group, keyed by `kid` (Secret's `key` field).
    pub credentials: HashMap<String, ResolvedJwtCredential>,
}

/// JWT Auth plugin configuration (route/service level)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Anonymous consumer name; if set, requests without valid JWT are allowed and this name is set
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

    /// If true, the secret is base64 encoded and needs to be decoded before use
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

    /// Map JWT claims to request headers for upstream.
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

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for JwtAuthConfig {
    fn validation_error(&self) -> Option<String> {
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

        // 3. Final gate: anonymous or groups required
        let has_groups = self.issuer_groups.as_ref().is_some_and(|v| !v.is_empty());
        let has_resolved = self.resolved_groups_by_iss.as_ref().is_some_and(|m| !m.is_empty());
        if self.anonymous.is_none() && !has_groups && !has_resolved {
            return Some("issuerGroups (or resolvedGroupsByIss) is required when anonymous is not set".to_string());
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;
    use crate::resources::gateway::SecretObjectReference;

    fn make_secret_ref() -> SecretObjectReference {
        SecretObjectReference {
            group: None,
            kind: None,
            name: "my-secret".to_string(),
            namespace: Some("default".to_string()),
        }
    }

    fn make_issuer_group(name: &str, issuer: &str) -> JwtIssuerGroup {
        JwtIssuerGroup {
            name: name.to_string(),
            description: None,
            issuers: vec![issuer.to_string()],
            secret_refs: vec![make_secret_ref()],
            algorithm: None,
            audiences: None,
        }
    }

    #[test]
    fn test_no_credential_no_anonymous_returns_error() {
        let cfg = JwtAuthConfig::default();
        let err = cfg.validation_error();
        assert!(err.is_some(), "expected a validation error");
        let msg = err.unwrap();
        assert!(
            msg.contains("issuerGroups"),
            "error message should mention issuerGroups, got: {msg}"
        );
    }

    #[test]
    fn test_anonymous_set_no_credential_is_valid() {
        let cfg = JwtAuthConfig {
            anonymous: Some("guest".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.validation_error(), None);
    }

    #[test]
    fn test_empty_header_returns_error() {
        let cfg = JwtAuthConfig {
            issuer_groups: Some(vec![make_issuer_group("g", "https://a.example")]),
            header: "   ".to_string(),
            ..Default::default()
        };
        let err = cfg.validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("header"));
    }

    #[test]
    fn test_valid_config_with_issuer_group_returns_none() {
        let cfg = JwtAuthConfig {
            issuer_groups: Some(vec![make_issuer_group("g", "https://a.example")]),
            ..Default::default()
        };
        assert_eq!(cfg.validation_error(), None);
    }

    #[test]
    fn test_validate_issuer_groups_empty() {
        let config = JwtAuthConfig {
            issuer_groups: Some(vec![]),
            anonymous: None,
            ..Default::default()
        };
        let err = config.validation_error();
        assert_eq!(err.unwrap(), "issuerGroups cannot be empty when present");
    }

    #[test]
    fn test_validate_issuer_groups_duplicate_name() {
        let config = JwtAuthConfig {
            issuer_groups: Some(vec![
                JwtIssuerGroup {
                    name: "a".into(),
                    description: None,
                    issuers: vec!["https://a.example".into()],
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s1".into(),
                        namespace: None,
                    }],
                    algorithm: None,
                    audiences: None,
                },
                JwtIssuerGroup {
                    name: "a".into(),
                    description: None,
                    issuers: vec!["https://b.example".into()],
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s2".into(),
                        namespace: None,
                    }],
                    algorithm: None,
                    audiences: None,
                },
            ]),
            anonymous: None,
            ..Default::default()
        };
        assert!(config.validation_error().unwrap().contains("duplicate group name 'a'"));
    }

    #[test]
    fn test_validate_issuer_groups_invalid_name() {
        let config = JwtAuthConfig {
            issuer_groups: Some(vec![JwtIssuerGroup {
                name: "-bad".into(),
                description: None,
                issuers: vec!["https://a.example".into()],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            }]),
            anonymous: None,
            ..Default::default()
        };
        assert!(config
            .validation_error()
            .unwrap()
            .contains("must start with alphanumeric"));
    }

    #[test]
    fn test_validate_issuer_groups_empty_issuers() {
        let config = JwtAuthConfig {
            issuer_groups: Some(vec![JwtIssuerGroup {
                name: "a".into(),
                description: None,
                issuers: vec![],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            }]),
            anonymous: None,
            ..Default::default()
        };
        assert!(config.validation_error().unwrap().contains("group 'a' has no issuers"));
    }

    #[test]
    fn test_validate_cross_group_iss_collision() {
        let dup_iss = "https://shared.example";
        let config = JwtAuthConfig {
            issuer_groups: Some(vec![
                JwtIssuerGroup {
                    name: "a".into(),
                    description: None,
                    issuers: vec![dup_iss.into()],
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s1".into(),
                        namespace: None,
                    }],
                    algorithm: None,
                    audiences: None,
                },
                JwtIssuerGroup {
                    name: "b".into(),
                    description: None,
                    issuers: vec![dup_iss.into()],
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s2".into(),
                        namespace: None,
                    }],
                    algorithm: None,
                    audiences: None,
                },
            ]),
            anonymous: None,
            ..Default::default()
        };
        let err = config.validation_error().unwrap();
        assert!(err.contains("iss must be unique across groups"), "got: {}", err);
        assert!(err.contains(dup_iss), "got: {}", err);
    }
}
