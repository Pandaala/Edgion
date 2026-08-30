//! mTLS field selector and SAN type filter for `Condition::MtlsMatch`.
//!
//! Lives as a sibling of `conditions.rs` so `Condition` can name `SanType`
//! without importing `plugin_configs`. `plugin_configs` re-exports `SanType`
//! so existing paths compile.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// mTLS client-certificate field selected by `Condition::MtlsMatch`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MtlsField {
    /// Full subject DN string.
    Subject,
    /// Subject Common Name.
    Cn,
    /// SAN list; any entry matching satisfies. Optional `sanType` scopes the kind.
    San,
}

impl MtlsField {
    /// Value-free accessor token for compiled descriptors (`subject` / `cn` / `san`).
    pub fn as_log_str(self) -> &'static str {
        match self {
            MtlsField::Subject => "subject",
            MtlsField::Cn => "cn",
            MtlsField::San => "san",
        }
    }
}

/// SAN type filter for `MtlsMatch` / `MtlsSan`. When set, matching is scoped to
/// SAN entries of this type, on the prefix-stripped value. `IP` is intentionally
/// not offered (no IP-based matching in this plugin).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum SanType {
    #[serde(rename = "DNS")]
    Dns,
    #[serde(rename = "URI")]
    Uri,
    #[serde(rename = "Email")]
    Email,
}
