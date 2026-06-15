//! KeyList group structure validation.
//!
//! Parallel to `ip_cidr_helpers::validate_groups_structure` but for
//! `EdgionData::KeyList`'s `Vec<KeyGroup>`. Validates structural
//! invariants only; does NOT build any matcher.

use std::collections::HashSet;

use crate::resources::common::group_name::validate_group_name;
use crate::resources::data::{KeyGroup, KeyMatchMode};

/// Validate a `Vec<KeyGroup>` for:
/// - groups list non-empty
/// - per-group `name` valid (K8s label-value charset, <=63 chars)
/// - cross-group `name` uniqueness
/// - per-group `items` non-empty
/// - per-item `key` non-empty
/// - cross- and intra-group `key` literal uniqueness (Regex mode does NOT
///   expand to pattern equivalence — operator is responsible for
///   non-overlapping patterns at the literal level)
/// - Regex mode: each `key` compiles via `regex::Regex::new` (error message
///   carries `group '<g>' item '<key>'` prefix)
pub fn validate_key_groups_structure(groups: &[KeyGroup], match_mode: &KeyMatchMode) -> Result<(), String> {
    if groups.is_empty() {
        return Err("group list is empty".to_string());
    }
    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut seen_keys: HashSet<&str> = HashSet::new();
    for g in groups {
        validate_group_name(&g.name)?;
        if !seen_names.insert(g.name.as_str()) {
            return Err(format!("duplicate group name '{}'", g.name));
        }
        if g.items.is_empty() {
            return Err(format!("group '{}' has no items", g.name));
        }
        for item in &g.items {
            if item.key.is_empty() {
                return Err(format!("group '{}' item has empty key", g.name));
            }
            if !seen_keys.insert(item.key.as_str()) {
                // Catches both intra-group AND cross-group duplicates.
                return Err(format!("duplicate key '{}'", item.key));
            }
            if matches!(match_mode, KeyMatchMode::Regex) {
                regex::Regex::new(&item.key)
                    .map_err(|e| format!("group '{}' item '{}': invalid regex: {}", g.name, item.key, e))?;
            }
        }
    }
    Ok(())
}
