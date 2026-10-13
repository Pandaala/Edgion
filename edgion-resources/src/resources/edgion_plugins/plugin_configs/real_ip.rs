//! Shared HTTP and global RealIp policy. Collections belong to referenced IpLists.

use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_reference_set::{IpReadResolution, IpReferenceSet};
use crate::resources::edgion_plugins::PluginValidation;
use schemars::JsonSchema;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealIpConfig {
    /// Ordered trusted proxy lists. Every list must be available before trusting headers.
    #[schemars(rename = "trustedIpRefs", with = "Vec<EdgionConfigDataRef>", length(min = 1))]
    pub trusted: IpReferenceSet,
    #[serde(default = "default_real_ip_header")]
    #[schemars(pattern(crate::gwapi_types::HEADER_NAME_PATTERN), length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN))]
    pub real_ip_header: String,
    #[serde(default = "default_recursive")]
    pub recursive: bool,
    /// None is unlimited. Recursive all-trusted chains return the leftmost parsed hop.
    /// Some(0) stops at the first trusted hop; nonrecursive mode ignores the cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_trusted_hops: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    #[serde(default)]
    trusted_ip_refs: Vec<EdgionConfigDataRef>,
    #[serde(default = "default_real_ip_header")]
    real_ip_header: String,
    #[serde(default = "default_recursive")]
    recursive: bool,
    #[serde(default)]
    max_trusted_hops: Option<u32>,
    #[serde(default)]
    invalid_input: bool,
    #[serde(default)]
    resolved_refs: Option<Box<IpReadResolution>>,
    #[serde(flatten)]
    rejected: HashMap<String, IgnoredAny>,
}

impl<'de> Deserialize<'de> for RealIpConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            trusted: IpReferenceSet {
                ip_refs: wire.trusted_ip_refs,
                invalid_input: wire.invalid_input || !wire.rejected.is_empty(),
                resolved_refs: wire.resolved_refs,
            },
            real_ip_header: wire.real_ip_header,
            recursive: wire.recursive,
            max_trusted_hops: wire.max_trusted_hops,
        })
    }
}

impl Serialize for RealIpConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct BorrowedWire<'a> {
            trusted_ip_refs: &'a [EdgionConfigDataRef],
            real_ip_header: &'a str,
            recursive: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            max_trusted_hops: Option<u32>,
            #[serde(skip_serializing_if = "is_false")]
            invalid_input: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            resolved_refs: Option<&'a IpReadResolution>,
        }
        BorrowedWire {
            trusted_ip_refs: &self.trusted.ip_refs,
            real_ip_header: &self.real_ip_header,
            recursive: self.recursive,
            max_trusted_hops: self.max_trusted_hops,
            invalid_input: self.trusted.invalid_input,
            resolved_refs: self.trusted.resolved_refs.as_deref(),
        }
        .serialize(serializer)
    }
}
fn is_false(value: &bool) -> bool {
    !value
}
fn default_real_ip_header() -> String {
    "X-Forwarded-For".into()
}
fn default_recursive() -> bool {
    true
}
impl Default for RealIpConfig {
    fn default() -> Self {
        Self {
            trusted: IpReferenceSet::default(),
            real_ip_header: default_real_ip_header(),
            recursive: true,
            max_trusted_hops: None,
        }
    }
}
impl RealIpConfig {
    pub fn new(mut config: RealIpConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        self.check()
    }
    fn check(&self) -> Result<(), String> {
        self.trusted
            .validate()
            .map_err(|_| "trustedIpRefs must contain valid references without inline or unknown fields".to_string())?;
        if self.real_ip_header.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
            || !crate::gwapi_types::is_valid_header_token(&self.real_ip_header)
        {
            return Err("'real_ip_header' must be a valid HTTP header name".into());
        }
        Ok(())
    }
}
impl PluginValidation for RealIpConfig {
    fn validation_error(&self) -> Option<String> {
        self.check().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/real_ip_test/validation_tests.rs"]
mod validation_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/real_ip_test.rs"]
mod tests;
