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
mod tests {
    use super::*;
    use crate::matcher::key_group_helpers::validate_key_groups_structure;

    fn make_item(key: &str) -> MetaDataItem {
        MetaDataItem {
            key: key.to_string(),
            code: None,
            priority: None,
            id: None,
            behavior: None,
        }
    }

    #[test]
    fn metadata_item_key_not_redacted_under_guard() {
        // `key` is a match target, not a secret: it must serialize plainly
        // even under the admin redaction guard.
        let item = MetaDataItem {
            key: "user-allow-list-entry".to_string(),
            code: None,
            priority: None,
            id: None,
            behavior: None,
        };
        let under_guard = {
            let _g = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&item).unwrap()
        };
        assert_eq!(under_guard["key"], "user-allow-list-entry");

        let plain = serde_json::to_value(&item).unwrap();
        assert_eq!(plain["key"], "user-allow-list-entry");
    }

    #[test]
    fn empty_groups_validation_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![],
        };
        assert!(validate_key_groups_structure(&data.items, &data.match_mode).is_err());
    }

    #[test]
    fn single_group_valid() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![KeyGroup {
                name: "default".into(),
                description: None,
                items: vec![make_item("alice")],
            }],
        };
        assert!(validate_key_groups_structure(&data.items, &data.match_mode).is_ok());
    }

    #[test]
    fn duplicate_group_name_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![
                KeyGroup {
                    name: "a".into(),
                    description: None,
                    items: vec![make_item("k1")],
                },
                KeyGroup {
                    name: "a".into(),
                    description: None,
                    items: vec![make_item("k2")],
                },
            ],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("duplicate group name 'a'"), "got: {}", err);
    }

    #[test]
    fn invalid_group_name_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![KeyGroup {
                name: "-bad".into(),
                description: None,
                items: vec![make_item("k1")],
            }],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("must start with alphanumeric"), "got: {}", err);
    }

    #[test]
    fn empty_items_in_group_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![KeyGroup {
                name: "a".into(),
                description: None,
                items: vec![],
            }],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("group 'a' has no items"), "got: {}", err);
    }

    #[test]
    fn cross_group_duplicate_key_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![
                KeyGroup {
                    name: "a".into(),
                    description: None,
                    items: vec![make_item("dup")],
                },
                KeyGroup {
                    name: "b".into(),
                    description: None,
                    items: vec![make_item("dup")],
                },
            ],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("duplicate key 'dup'"), "got: {}", err);
    }

    #[test]
    fn same_group_duplicate_key_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![KeyGroup {
                name: "a".into(),
                description: None,
                items: vec![make_item("dup"), make_item("dup")],
            }],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("duplicate key 'dup'"), "got: {}", err);
    }

    #[test]
    fn empty_key_in_item_errs() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Exact,
            items: vec![KeyGroup {
                name: "a".into(),
                description: None,
                items: vec![make_item("")],
            }],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("group 'a' item has empty key"), "got: {}", err);
    }

    #[test]
    fn regex_mode_invalid_pattern_errs_with_group_prefix() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Regex,
            items: vec![KeyGroup {
                name: "broken".into(),
                description: None,
                items: vec![make_item("(unclosed")],
            }],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains("group 'broken'"), "expected group prefix, got: {}", err);
        assert!(err.contains("(unclosed"), "expected item key in error, got: {}", err);
        assert!(err.contains("invalid regex"), "expected 'invalid regex', got: {}", err);
    }

    #[test]
    fn regex_mode_valid_patterns_pass() {
        let data = KeyListData {
            match_mode: KeyMatchMode::Regex,
            items: vec![KeyGroup {
                name: "default".into(),
                description: None,
                items: vec![make_item(r"^user-[a-z]+$"), make_item(r"^admin-\d+$")],
            }],
        };
        assert!(validate_key_groups_structure(&data.items, &data.match_mode).is_ok());
    }

    #[test]
    fn regex_mode_cross_group_literal_duplicate_errs() {
        // Locks "literal uniqueness, no regex equivalence expansion" contract.
        let data = KeyListData {
            match_mode: KeyMatchMode::Regex,
            items: vec![
                KeyGroup {
                    name: "a".into(),
                    description: None,
                    items: vec![make_item(r"^user-\d+$")],
                },
                KeyGroup {
                    name: "b".into(),
                    description: None,
                    items: vec![make_item(r"^user-\d+$")],
                },
            ],
        };
        let err = validate_key_groups_structure(&data.items, &data.match_mode).unwrap_err();
        assert!(err.contains(r"duplicate key '^user-\d+$'"), "got: {}", err);
    }

    #[test]
    fn match_mode_default_when_absent_in_yaml() {
        // Regression: when YAML omits matchMode, deserialization falls back
        // to Exact (KeyMatchMode::default()).
        let yaml = r#"
items:
  - name: default
    items:
      - key: alice
"#;
        let data: KeyListData = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(data.match_mode, KeyMatchMode::Exact));
    }

    #[test]
    fn serde_roundtrip() {
        let yaml = r#"
matchMode: exact
items:
  - name: partner-a
    description: "Partner A keys"
    items:
      - key: alice
        code: 100
      - key: bob
  - name: partner-b
    items:
      - key: carol
"#;
        let data: KeyListData = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(data.items.len(), 2);
        assert_eq!(data.items[0].name, "partner-a");
        assert_eq!(data.items[0].items.len(), 2);
        assert_eq!(data.items[1].name, "partner-b");
        // re-serialize and parse back
        let json = serde_json::to_string(&data).unwrap();
        let _data2: KeyListData = serde_json::from_str(&json).unwrap();
    }
}
