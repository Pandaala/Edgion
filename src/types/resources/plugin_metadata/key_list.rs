//! Key list metadata — unified StringList + RegexList
//!
//! A list of key-value metadata items with configurable match mode.
//! `Exact` mode matches keys as literal strings (previously StringList).
//! `Regex` mode matches keys as regular expressions (previously RegexList).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Match mode for key list items.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum KeyMatchMode {
    /// Exact string matching (equivalent to the legacy StringList).
    #[default]
    Exact,
    /// Regular expression matching (equivalent to the legacy RegexList).
    Regex,
}

/// A list of key-value metadata items.
///
/// Used by plugins like IpRestriction, KeyAuth, etc. to reference external
/// metadata by PluginMetaData name. The `match_mode` determines whether
/// keys are matched as exact strings or regex patterns.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyListData {
    /// How to match the key values: exact string or regex pattern.
    #[serde(default)]
    pub match_mode: KeyMatchMode,

    /// Metadata items in this list.
    pub items: Vec<MetaDataItem>,
}

/// A metadata item with a key and optional code, priority, id, and behavior.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetaDataItem {
    /// Key value (string literal or regex pattern, depending on match_mode)
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
