//! Bounded correlation identifier configuration shared by Controller and Gateway.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{is_blocked, LogExtSource};
use crate::{common::KeyGet, gwapi_types::is_valid_header_token, resources::edgion_plugins::PluginValidation};

pub const REQUEST_ID_MAX_COUNT: usize = 16;
pub const REQUEST_ID_MAX_VALUE_LEN: usize = 256;
pub const REQUEST_ID_MAX_HEADER_LEN: usize = 128;
pub const REQUEST_ID_MAX_PREFIX_LEN: usize = 64;
pub const REQUEST_ID_MAX_TOTAL_VALUE_LEN: usize = 4096;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestIdConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_id: Option<DefaultIdConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<RequestIdRule>,
}

/// Omitted fields leave the policy established by earlier attachments unchanged.
/// Controls the reserved W3C `x-trace-id` header; its access-log identity is always retained.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefaultIdConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_upstream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo_response: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestIdRule {
    /// Custom correlation header; `x-trace-id` is reserved for the W3C trace ID.
    pub header: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<KeyGet>,
    #[serde(default = "default_true")]
    pub generate_when_missing: bool,
    #[serde(default)]
    pub generator: RequestIdGenerator,
    #[serde(default = "default_true")]
    pub forward_upstream: bool,
    #[serde(default = "default_true")]
    pub echo_response: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestIdGenerator {
    #[serde(rename = "type", default)]
    pub kind: RequestIdGeneratorKind,
    #[serde(default)]
    pub prefix: String,
    /// RandomHex entropy bytes; defaults to 16, allowed range 8-64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// NanoId output length; defaults to 21, allowed range 8-128.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RequestIdGeneratorKind {
    #[default]
    #[serde(rename = "uuidV4")]
    UuidV4,
    #[serde(rename = "uuidV7")]
    UuidV7,
    #[serde(rename = "randomHex")]
    RandomHex,
    #[serde(rename = "nanoId")]
    NanoId,
    #[serde(rename = "md5")]
    Md5,
}

/// Correlation IDs cannot contain whitespace, list separators or control bytes.
pub fn valid_request_id_value(value: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= REQUEST_ID_MAX_VALUE_LEN
        && value.iter().all(|b| b.is_ascii_graphic() && *b != b',')
}

/// Destinations cannot change HTTP framing, routing, credentials or W3C context.
pub fn valid_request_id_header(name: &str) -> bool {
    if name.len() > REQUEST_ID_MAX_HEADER_LEN || !is_valid_header_token(name) || is_blocked(LogExtSource::Header, name)
    {
        return false;
    }
    let name = name.to_ascii_lowercase();
    !matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
            | "openai-organization"
            | "openai-project"
            | "anthropic-workspace-id"
            | "anthropic-user-profile-id"
            | "anthropic-version"
            | "anthropic-beta"
            | "traceparent"
            | "tracestate"
            | "x-trace-id"
            | "forwarded"
            | "via"
            | "expect"
            | "content-type"
            | "content-encoding"
            | "accept-encoding"
            | "range"
            | "location"
            | "refresh"
    ) && !name.starts_with("x-forwarded-")
        && !name.starts_with("sec-")
        && !name.starts_with("proxy-")
        && !name.starts_with("grpc-")
}

impl RequestIdGenerator {
    pub fn validation_error(&self) -> Option<String> {
        if self.prefix.len() > REQUEST_ID_MAX_PREFIX_LEN
            || (!self.prefix.is_empty() && !valid_request_id_value(self.prefix.as_bytes()))
        {
            return Some(
                "generator prefix must contain at most 64 printable non-whitespace, non-comma ASCII bytes".into(),
            );
        }
        if self.bytes.is_some() && self.kind != RequestIdGeneratorKind::RandomHex {
            return Some("generator bytes is only supported for randomHex".into());
        }
        if self.length.is_some() && self.kind != RequestIdGeneratorKind::NanoId {
            return Some("generator length is only supported for nanoId".into());
        }
        if self.bytes.is_some_and(|n| !(8..=64).contains(&n)) {
            return Some("randomHex bytes must be between 8 and 64".into());
        }
        if self.length.is_some_and(|n| !(8..=128).contains(&n)) {
            return Some("nanoId length must be between 8 and 128".into());
        }
        None
    }
}

impl PluginValidation for RequestIdConfig {
    fn validation_error(&self) -> Option<String> {
        if self.ids.len() > REQUEST_ID_MAX_COUNT {
            return Some("ids must contain at most 16 rules".into());
        }
        let mut names = std::collections::HashSet::new();
        for (index, rule) in self.ids.iter().enumerate() {
            if !valid_request_id_header(&rule.header) {
                return Some(format!("ids[{index}]: invalid or reserved destination header"));
            }
            if !names.insert(rule.header.to_ascii_lowercase()) {
                return Some(format!("ids[{index}]: duplicate destination header"));
            }
            let valid_source = match &rule.source {
                None | Some(KeyGet::TraceId) => true,
                Some(KeyGet::Header { name }) => {
                    name.len() <= REQUEST_ID_MAX_HEADER_LEN
                        && is_valid_header_token(name)
                        && !is_blocked(LogExtSource::Header, name)
                }
                Some(KeyGet::Ctx { name }) => {
                    !name.is_empty() && name.len() <= REQUEST_ID_MAX_HEADER_LEN && !is_blocked(LogExtSource::Ctx, name)
                }
                _ => false,
            };
            if !valid_source {
                return Some(format!("ids[{index}]: unsupported or sensitive source"));
            }
            if let Some(error) = rule.generator.validation_error() {
                return Some(format!("ids[{index}]: {error}"));
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/request_id_test.rs"]
mod tests;
