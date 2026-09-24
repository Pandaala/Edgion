use std::fmt;

use http::HeaderValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};
use crate::resources::common::{is_core_api_group, SecretObjectReference};
use crate::resources::edgion_plugins::PluginValidation;

pub const CREDENTIAL_INJECTOR_VALUE_PREFIX_MAX_LEN: usize = 256;
pub const CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN: usize = 16 * 1024;

const RESERVED_TARGET_HEADER_NAMES: &[&str] = &[
    "host",
    ":authority",
    "content-length",
    "transfer-encoding",
    "trailer",
    "te",
    "expect",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "upgrade",
    "forwarded",
    "x-forwarded-for",
    "x-real-ip",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "via",
    "grpc-timeout",
    "cookie",
];

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialInjectorConfig {
    pub credential_ref: SecretObjectReference,

    #[serde(default)]
    pub target: CredentialInjectionTarget,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_credential: Option<ResolvedCredentialInjector>,
}

impl CredentialInjectorConfig {
    /// Whether this injector is backed by a controller-managed LinkSys.
    pub fn is_link_sys_ref(&self) -> bool {
        self.credential_ref.group.as_deref() == Some("edgion.io")
            && self.credential_ref.kind.as_deref() == Some("LinkSys")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum CredentialInjectionTarget {
    #[default]
    Bearer,
    Header {
        name: String,
        #[serde(default)]
        value_prefix: String,
    },
}

impl CredentialInjectionTarget {
    pub fn value_prefix(&self) -> &str {
        match self {
            Self::Bearer => "Bearer ",
            Self::Header { value_prefix, .. } => value_prefix,
        }
    }

    pub fn final_value_len(&self, credential_len: usize) -> Option<usize> {
        self.value_prefix().len().checked_add(credential_len)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedCredentialInjector {
    pub value: String,

    /// Absolute RFC3339 validity deadline for a dynamically acquired
    /// credential. Static Secret credentials have no deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
}

impl fmt::Debug for ResolvedCredentialInjector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCredentialInjector")
            .field("value", &"[redacted]")
            .finish()
    }
}

impl PluginValidation for CredentialInjectorConfig {
    fn validation_error(&self) -> Option<String> {
        if self.credential_ref.name.is_empty() {
            return Some("credentialRef.name must not be empty".to_string());
        }
        let group = self.credential_ref.group.as_deref().unwrap_or("");
        let kind = self.credential_ref.kind.as_deref().unwrap_or("Secret");
        let static_secret = is_core_api_group(group) && kind == "Secret";
        let managed_link_sys = group == "edgion.io" && kind == "LinkSys";
        if !static_secret && !managed_link_sys {
            return Some("credentialRef group/kind must identify either core/Secret or edgion.io/LinkSys".to_string());
        }

        let CredentialInjectionTarget::Header { name, value_prefix } = &self.target else {
            return None;
        };
        if name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name) {
            return Some("target.name must be a valid HTTP header name of at most 256 bytes".to_string());
        }
        if RESERVED_TARGET_HEADER_NAMES
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            return Some(format!("target.name '{name}' is reserved"));
        }
        if value_prefix.len() > CREDENTIAL_INJECTOR_VALUE_PREFIX_MAX_LEN {
            return Some("target.valuePrefix must be at most 256 bytes".to_string());
        }
        if HeaderValue::from_str(value_prefix).is_err() {
            return Some("target.valuePrefix must be a valid HTTP header value prefix".to_string());
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/credential_injector_test.rs"]
mod tests;
