use schemars::JsonSchema;

crate::redact::impl_redacted_debug!(CsrfConfig);
use serde::{Deserialize, Serialize};

use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};
use crate::resources::gateway::SecretObjectReference;

/// CSRF (Cross-Site Request Forgery) protection plugin configuration
///
/// This plugin protects against CSRF attacks by validating tokens in requests
/// and setting CSRF tokens in response cookies.
///
/// ## How it works:
/// 1. On ResponseHeader stage: generates and sets a CSRF token cookie
/// 2. On Request stage: validates that the token in header matches the cookie token
///
/// ## Usage Example:
/// ```yaml
/// csrf:
///   secretRef:
///     name: csrf-key
///   expires: 7200  # 2 hours
///   name: "apisix-csrf-token"
/// ```
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsrfConfig {
    /// Reference to a K8s Secret whose `key` data entry holds the CSRF signing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretObjectReference>,

    /// Positive token expiration time in seconds (default: 7200 = 2 hours)
    #[serde(default = "default_expires")]
    pub expires: i64,

    /// HTTP-token name used in both cookie and header (default: "apisix-csrf-token")
    #[serde(default = "default_name")]
    pub name: String,

    /// Whether to set the `Secure` attribute on the CSRF cookie (default: true)
    #[serde(default = "default_cookie_secure")]
    pub cookie_secure: bool,

    /// Resolved signing key (filled by controller from `secret_ref`; never from
    /// user YAML). Serialized to the gateway via conf_sync; hidden from CRD schema
    /// and redacted in admin output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_key: Option<String>,
}

fn default_expires() -> i64 {
    7200 // 2 hours
}

fn default_name() -> String {
    "apisix-csrf-token".to_string()
}

fn default_cookie_secure() -> bool {
    true
}

impl Default for CsrfConfig {
    fn default() -> Self {
        CsrfConfig {
            secret_ref: None,
            expires: default_expires(),
            name: default_name(),
            cookie_secure: default_cookie_secure(),
            resolved_key: None,
        }
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for CsrfConfig {
    fn validation_error(&self) -> Option<String> {
        // Require a Secret reference OR an already-resolved key. Mirrors jwe_decrypt:
        // does NOT error in the secret_ref-present / resolved-absent window, so the
        // controller never rejects a CSRF plugin whose Secret hasn't resolved yet.
        // The gateway-only check that the key is actually present before the plugin
        // goes live lives in plugin_runtime.rs::plugin_validation_error.
        if self.secret_ref.is_none() && self.resolved_key.is_none() {
            return Some("CSRF config: 'secretRef' is required".to_string());
        }
        if self.expires <= 0 {
            return Some("CSRF config: 'expires' must be positive".to_string());
        }
        if self.name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(&self.name) {
            return Some("CSRF config: 'name' must be a valid HTTP token of at most 256 bytes".to_string());
        }
        if self.name.eq_ignore_ascii_case("cookie") {
            return Some("CSRF config: 'name' must not be 'cookie'".to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_deserializes_secret_ref() {
        let json = r#"{"secretRef":{"name":"csrf-key"}}"#;
        let c: CsrfConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.secret_ref.as_ref().unwrap().name, "csrf-key");
        assert!(c.resolved_key.is_none());
        assert_eq!(c.expires, 7200);
        assert_eq!(c.name, "apisix-csrf-token");
    }

    #[test]
    fn csrf_rejects_inline_key() {
        let json = r#"{"key":"leaked"}"#;
        assert!(serde_json::from_str::<CsrfConfig>(json).is_err());
    }

    #[test]
    fn csrf_validation_requires_secret_ref_or_resolved() {
        use crate::resources::edgion_plugins::PluginValidation;
        let empty = CsrfConfig::default();
        assert!(empty.validation_error().is_some());
        let with_ref = CsrfConfig {
            secret_ref: Some(crate::resources::gateway::SecretObjectReference {
                group: None,
                kind: None,
                name: "k".into(),
                namespace: None,
            }),
            ..Default::default()
        };
        assert!(with_ref.validation_error().is_none());
        let with_resolved = CsrfConfig {
            resolved_key: Some("k".into()),
            ..Default::default()
        };
        assert!(with_resolved.validation_error().is_none());
    }

    #[test]
    fn csrf_resolved_key_redacted_under_guard() {
        let cfg = CsrfConfig {
            resolved_key: Some("SUPER-SECRET".to_string()),
            ..Default::default()
        };
        let redacted = {
            let _g = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(redacted["resolvedKey"], "[redacted]");
        let plain = serde_json::to_value(&cfg).unwrap();
        assert_eq!(plain["resolvedKey"], "SUPER-SECRET");
    }

    #[test]
    fn csrf_validation_requires_a_positive_expiration() {
        for expires in [0, -1, i64::MIN] {
            let cfg = CsrfConfig {
                resolved_key: Some("k".into()),
                expires,
                ..Default::default()
            };
            assert_eq!(
                cfg.validation_error().as_deref(),
                Some("CSRF config: 'expires' must be positive")
            );
        }

        let cfg = CsrfConfig {
            resolved_key: Some("k".into()),
            expires: 1,
            ..Default::default()
        };
        assert!(cfg.validation_error().is_none());
    }

    #[test]
    fn csrf_validation_bounds_the_shared_header_and_cookie_token_name() {
        for name in [
            default_name(),
            "x".repeat(HEADER_NAME_MAX_LEN as usize),
            "x-cookie".to_string(),
        ] {
            let cfg = CsrfConfig {
                resolved_key: Some("k".into()),
                name,
                ..Default::default()
            };
            assert!(cfg.validation_error().is_none());
        }

        for name in [
            String::new(),
            "bad name".to_string(),
            "bad:name".to_string(),
            "x".repeat(HEADER_NAME_MAX_LEN as usize + 1),
        ] {
            let cfg = CsrfConfig {
                resolved_key: Some("k".into()),
                name,
                ..Default::default()
            };
            assert_eq!(
                cfg.validation_error().as_deref(),
                Some("CSRF config: 'name' must be a valid HTTP token of at most 256 bytes")
            );
        }
    }

    #[test]
    fn csrf_validation_rejects_the_cookie_header_name_case_insensitively() {
        for name in ["cookie", "Cookie", "COOKIE"] {
            let cfg = CsrfConfig {
                resolved_key: Some("k".into()),
                name: name.to_string(),
                ..Default::default()
            };
            assert_eq!(
                cfg.validation_error().as_deref(),
                Some("CSRF config: 'name' must not be 'cookie'")
            );
        }
    }
}
