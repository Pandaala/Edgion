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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/csrf_test.rs"]
mod tests;
