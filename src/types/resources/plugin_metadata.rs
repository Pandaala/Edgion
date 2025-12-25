//! PluginMetaData resource definition
//!
//! PluginMetaData defines metadata for edgion_plugins, supporting three data types:
//! - StringList: List of strings with metadata
//! - IpList: List of IP addresses/CIDR ranges
//! - RegexList: List of regex patterns with metadata

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for PluginMetaData
pub const PLUGIN_METADATA_GROUP: &str = "edgion.io";

/// Kind for PluginMetaData
pub const PLUGIN_METADATA_KIND: &str = "PluginMetaData";

/// PluginMetaData defines metadata for edgion_plugins
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "PluginMetaData",
    plural = "pluginmetadata",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetaDataSpec {
    /// StringList contains a list of strings with metadata
    /// Only one of stringList, ipList, or regexList should be set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string_list: Option<StringListData>,
    
    /// IpList contains a list of IP addresses/CIDR ranges
    /// Only one of stringList, ipList, or regexList should be set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_list: Option<IpListData>,
    
    /// RegexList contains a list of regex patterns with metadata
    /// Only one of stringList, ipList, or regexList should be set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex_list: Option<RegexListData>,
}

/// StringListData contains a list of string items with metadata
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StringListData {
    /// Items in the string list
    pub items: Vec<StringItem>,
}

/// StringItem represents a string entry with metadata
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StringItem {
    /// Key is the string value (required)
    pub key: String,
    
    /// Code is a numeric code associated with this item
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    
    /// Priority determines the order of evaluation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    
    /// ID is a unique identifier for this item
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    
    /// Behavior defines how this item should be processed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
}

/// IpListData contains a list of IP addresses or CIDR ranges
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpListData {
    /// Items in the IP list (CIDR format: "192.168.1.0/24", "10.0.0.0/8", etc.)
    pub items: Vec<String>,
}

/// RegexListData contains a list of regex patterns with metadata
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegexListData {
    /// Items in the regex list
    pub items: Vec<RegexItem>,
}

/// RegexItem represents a regex pattern entry with metadata
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegexItem {
    /// Key is the regex pattern (required)
    pub key: String,
    
    /// Code is a numeric code associated with this pattern
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    
    /// Priority determines the order of pattern evaluation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    
    /// ID is a unique identifier for this pattern
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    
    /// Behavior defines how this pattern should be processed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
}

impl PluginMetaData {
    /// Validate that only one data type is set
    pub fn validate(&self) -> Result<(), String> {
        let mut count = 0;
        if self.spec.string_list.is_some() {
            count += 1;
        }
        if self.spec.ip_list.is_some() {
            count += 1;
        }
        if self.spec.regex_list.is_some() {
            count += 1;
        }
        
        match count {
            0 => Err("PluginMetaData must have one of stringList, ipList, or regexList set".to_string()),
            1 => Ok(()),
            _ => Err("PluginMetaData can only have one of stringList, ipList, or regexList set".to_string()),
        }
    }
    
    /// Get the data type of this metadata
    pub fn data_type(&self) -> Option<&str> {
        if self.spec.string_list.is_some() {
            Some("StringList")
        } else if self.spec.ip_list.is_some() {
            Some("IpList")
        } else if self.spec.regex_list.is_some() {
            Some("RegexList")
        } else {
            None
        }
    }
}

