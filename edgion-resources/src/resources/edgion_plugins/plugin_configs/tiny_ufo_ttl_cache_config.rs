//! Configuration for the bounded TinyUFO TTL cache.

use crate::resources::common::parse_byte_size;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TINY_UFO_WEIGHT_UNIT_BYTES: usize = 64;
pub const MAX_TINY_UFO_CACHE_ENTRIES: usize = 65_536;
pub const MAX_TINY_UFO_LOGICAL_BYTES: usize = 256 * 1024 * 1024;

pub fn validate_tiny_ufo_effective_limits(max_entries: usize, max_logical_bytes: usize) -> Result<(), &'static str> {
    if max_entries == 0 {
        return Err("TinyUFO maxEntries must be greater than 0");
    }
    if max_entries > MAX_TINY_UFO_CACHE_ENTRIES {
        return Err("TinyUFO maxEntries exceeds the safety ceiling");
    }
    if max_logical_bytes < TINY_UFO_WEIGHT_UNIT_BYTES {
        return Err("TinyUFO maxLogicalBytes is smaller than one weight unit");
    }
    if max_logical_bytes > MAX_TINY_UFO_LOGICAL_BYTES {
        return Err("TinyUFO maxLogicalBytes exceeds the safety ceiling");
    }
    let total_units = max_logical_bytes / TINY_UFO_WEIGHT_UNIT_BYTES;
    let min_entry_units = total_units
        .checked_add(max_entries - 1)
        .ok_or("TinyUFO limit arithmetic overflowed")?
        / max_entries;
    if min_entry_units > u16::MAX as usize {
        return Err("TinyUFO logical budget per entry exceeds the backend weight limit");
    }
    Ok(())
}

/// Plugin-local limits for the Gateway's `TinyUfoTtlCache`.
///
/// Omit both values to inherit the Gateway's `tiny_ufo_ttl_cache` defaults.
/// Plugin-local overrides must set both values together; an explicitly zero
/// total logical budget is invalid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct TinyUfoTtlCacheConfig {
    /// Maximum retained entries. Zero inherits the Gateway process default.
    pub max_entries: usize,

    /// Total logical retention budget, expressed as a human-readable byte size.
    /// Omission inherits the Gateway process default. This bounds cache payload
    /// and record weight; it is not an exact allocator/RSS limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_logical_bytes: Option<String>,
}

impl TinyUfoTtlCacheConfig {
    /// Parse and validate the optional total logical budget.
    pub fn parsed_max_logical_bytes(&self) -> Result<Option<u64>, &'static str> {
        let Some(value) = self.max_logical_bytes.as_deref() else {
            return Ok(None);
        };
        let bytes = parse_byte_size(value).ok_or("maxLogicalBytes must be a valid byte size")?;
        if bytes == 0 {
            return Err("maxLogicalBytes must be greater than 0");
        }
        Ok(Some(bytes))
    }

    /// Return the resource-layer validation error, if any.
    pub fn validation_error(&self) -> Option<&'static str> {
        let logical_bytes = match self.parsed_max_logical_bytes() {
            Ok(bytes) => bytes,
            Err(error) => return Some(error),
        };
        if self.max_entries > MAX_TINY_UFO_CACHE_ENTRIES {
            return Some("TinyUFO maxEntries exceeds the safety ceiling");
        }
        if (self.max_entries == 0) != logical_bytes.is_none() {
            return Some("TinyUFO maxEntries and maxLogicalBytes must be configured together");
        }
        if let Some(logical_bytes) = logical_bytes {
            let Ok(logical_bytes) = usize::try_from(logical_bytes) else {
                return Some("TinyUFO maxLogicalBytes does not fit this platform");
            };
            if logical_bytes > MAX_TINY_UFO_LOGICAL_BYTES {
                return Some("TinyUFO maxLogicalBytes exceeds the safety ceiling");
            }
            if logical_bytes < TINY_UFO_WEIGHT_UNIT_BYTES {
                return Some("TinyUFO maxLogicalBytes is smaller than one weight unit");
            }
            if self.max_entries > 0 {
                return validate_tiny_ufo_effective_limits(self.max_entries, logical_bytes).err();
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/tiny_ufo_ttl_cache_config_test.rs"]
mod tests;
