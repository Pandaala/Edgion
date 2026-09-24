//! Key list data — unified StringList + RegexList
//!
//! A grouped list of key-value data items with configurable match mode.
//! Mirrors `IpListData` / `IpGroup` (sibling EdgionConfigData type).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Match mode for key list items.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum KeyMatchMode {
    #[default]
    Exact,
    Regex,
}

/// A named group of metadata items.
///
/// Used by `KeyListData` to organize items by management origin
/// (partner-a / partner-b / internal). Cross-group `key` uniqueness is
/// enforced at preparse; the group name surfaces in audit/log when a
/// consumer plugin (future) routes a match back to its origin.
///
/// Mirrors `IpGroup` (`ip_group.rs`).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyGroup {
    /// Group name; charset per K8s label-value (validated via shared
    /// `validate_group_name`).
    pub name: String,

    /// Human-readable description (not used by runtime).
    /// Mirrors IpGroup: `skip_serializing_if` only, NO `default` attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Metadata items in this group. Must be non-empty at preparse.
    pub items: Vec<MetaDataItem>,
}

/// A list of key-value data items, organized into named groups.
///
/// Used by plugins (KeyAuth, IpRestriction, etc., future use) to reference
/// external data by EdgionConfigData name. The `match_mode` is plugin-level
/// only (not overridable per group; YAGNI).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyListData {
    /// How to match the key values: exact string or regex pattern.
    #[serde(default)]
    pub match_mode: KeyMatchMode,

    /// Named groups of metadata items.
    pub items: Vec<KeyGroup>,
}

/// A metadata item with a key and optional code, priority, id, and behavior.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetaDataItem {
    /// The value to match against (string literal or regex pattern, depending
    /// on `match_mode`). This is a MATCH TARGET, not a credential: it is NOT
    /// redacted on the admin API. Do not store secrets here — real credentials
    /// (e.g. KeyAuth API keys) belong in a Secret referenced via the consuming
    /// plugin's `secret_refs`.
    pub key: String,

    /// Numeric code associated with this item
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,

    /// Priority determines the order of evaluation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,

    /// Unique identifier for this item
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Behavior defines how this item should be processed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/key_list_test.rs"]
mod tests;
