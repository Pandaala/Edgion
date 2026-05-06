//! JWT Auth plugin configuration
//!
//! Supports verification of JWT in header, query, or cookie.
//! Credentials: single secret/public_key (secret_ref) or multiple keys (secret_refs).

use crate::types::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Resolved JWT credential data (populated by controller from Secret)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJwtCredential {
    /// Key identifier (for multi-key mode)
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

/// JWT Auth plugin configuration (route/service level)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JwtAuthConfig {
    /// Single Secret ref: Secret must contain "secret" (for HS*) or "publicKey" (for RS*/ES*).
    /// Used for single-issuer verification (no key lookup by claim).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretObjectReference>,

    /// Multiple credentials: list of Secret refs. Each Secret must contain "key" and "secret" (HS*)
    /// or "key" and "publicKey" (RS*/ES*). The JWT payload claim named by key_claim_name is used
    /// to select the credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<Vec<SecretObjectReference>>,

    /// Algorithm (default: HS256)
    #[serde(default)]
    pub algorithm: JwtAlgorithm,

    /// Header name to read token from (default: "authorization"; expect "Bearer <token>" or raw token)
    #[serde(default = "default_header")]
    pub header: String,

    /// Query parameter name (default: "jwt")
    #[serde(default = "default_query")]
    pub query: String,

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

    /// List of valid issuers (iss claim). If empty, issuer is not validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuers: Option<Vec<String>>,

    /// List of valid audiences (aud claim). If empty, audience is not validated.
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

    // === Runtime fields (populated by controller, not user-configurable) ===
    /// Resolved single credential (from secret_ref, populated by controller)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_credential: Option<ResolvedJwtCredential>,

    /// Resolved multiple credentials (from secret_refs, populated by controller)
    /// Key is the credential identifier from Secret's "key" field
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_credentials: Option<HashMap<String, ResolvedJwtCredential>>,
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
            secret_ref: None,
            secret_refs: None,
            algorithm: JwtAlgorithm::default(),
            header: default_header(),
            query: default_query(),
            cookie: default_cookie(),
            hide_credentials: false,
            anonymous: None,
            key_claim_name: default_key_claim_name(),
            lifetime_grace_period: 0,
            issuers: None,
            audiences: None,
            realm: default_realm(),
            base64_secret: false,
            maximum_expiration: 0,
            auth_failure_delay_ms: 0,
            store_claims_in_ctx: false,
            claims_to_headers: None,
            resolved_credential: None,
            resolved_credentials: None,
        }
    }
}

use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for JwtAuthConfig {
    fn validation_error(&self) -> Option<String> {
        // When not anonymous, at least one credential source must be present.
        // resolved_credential/resolved_credentials are set by the controller
        // (after secret lookup); at preparse time they may be None even if
        // secret_ref/secret_refs are set, so we only check the user-facing refs.
        if self.anonymous.is_none()
            && self.secret_ref.is_none()
            && self.secret_refs.is_none()
            && self.resolved_credential.is_none()
            && self.resolved_credentials.is_none()
        {
            return Some(
                "At least one of secretRef, secretRefs, or resolvedCredential must be provided \
                 when anonymous is not set"
                    .to_string(),
            );
        }

        // Token source fields must not be empty
        if self.header.trim().is_empty() {
            return Some("header must not be empty".to_string());
        }
        if self.query.trim().is_empty() {
            return Some("query must not be empty".to_string());
        }
        if self.cookie.trim().is_empty() {
            return Some("cookie must not be empty".to_string());
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_plugins::PluginValidation;
    use crate::types::resources::gateway::SecretObjectReference;

    fn make_secret_ref() -> SecretObjectReference {
        SecretObjectReference {
            group: None,
            kind: None,
            name: "my-secret".to_string(),
            namespace: Some("default".to_string()),
        }
    }

    #[test]
    fn test_no_credential_no_anonymous_returns_error() {
        let cfg = JwtAuthConfig::default();
        let err = cfg.validation_error();
        assert!(err.is_some(), "expected a validation error");
        let msg = err.unwrap();
        assert!(
            msg.contains("secretRef"),
            "error message should mention secretRef, got: {msg}"
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
            secret_ref: Some(make_secret_ref()),
            header: "   ".to_string(),
            ..Default::default()
        };
        let err = cfg.validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("header"));
    }

    #[test]
    fn test_valid_config_with_secret_ref_returns_none() {
        let cfg = JwtAuthConfig {
            secret_ref: Some(make_secret_ref()),
            ..Default::default()
        };
        assert_eq!(cfg.validation_error(), None);
    }
}
