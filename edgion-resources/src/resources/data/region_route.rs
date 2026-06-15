//! RegionRoute overlay EdgionData type
//!
//! `RegionRouteOverride` — all-Optional typed overlay referenced via `overrideRef`
//! from a `RegionRouteConfig` in EdgionPlugins.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::plugin_configs::RegionDef;

/// All-Optional typed overlay for RegionRoute.
///
/// Referenced via `overrideRef` from a `RegionRouteConfig` in `EdgionPlugins`.
/// Only `Some` fields replace the corresponding base field; `None` keeps the base value.
/// The `regions` collection (if `Some`) is replaced **whole** — no element merge.
///
/// Logic fields (`route_rules`, `hash_calc`, `key_get`, etc.) are **not** present here —
/// logic is exclusively in the base `RegionRouteConfig` and cannot be overridden.
///
/// # Security invariant
/// `RegionRouteConfig.regions` in `EdgionPlugins` MUST be authored as the safe default.
/// On any overlay error, the base `regions` are used (fail-to-base).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteOverride {
    /// Override the kill-switch. When `Some(false)`, the plugin passes through (no routing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,

    /// Override the active profile selector (future use; reserved for profile-based routing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,

    /// Whole-replace the region table. When `Some`, completely substitutes the base regions.
    /// No element-wise merge — the entire table is replaced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regions: Option<Vec<RegionDef>>,
}

impl RegionRouteOverride {
    /// Standalone validation — checks only the fields that are present.
    ///
    /// Called by the controller at EdgionData ingest time (via `validate_pre_parse`).
    pub fn validate_and_init(&self) -> Result<(), String> {
        if let Some(regions) = &self.regions {
            if regions.is_empty() {
                return Err("RegionRouteOverride: regions must not be empty when specified".to_string());
            }
            // Validate each region's hash_range bounds [low, high] with low <= high
            for r in regions {
                if r.hash_range[0] > r.hash_range[1] {
                    return Err(format!(
                        "RegionRouteOverride: region '{}' hashRange[{}..{}] is invalid (low > high)",
                        r.name, r.hash_range[0], r.hash_range[1]
                    ));
                }
                let (host, _) = r.parse_endpoint();
                if host.is_empty() {
                    return Err(format!(
                        "RegionRouteOverride: region '{}' backendEndpoint is empty",
                        r.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Merge a `RegionRouteOverride` onto base `regions`, returning the effective `regions`.
///
/// - `Some(regions)` in overlay → whole-replace base regions (no element merge).
/// - `None` → base regions unchanged.
pub fn merge_region_route_override(base_regions: Vec<RegionDef>, overlay: &RegionRouteOverride) -> Vec<RegionDef> {
    if let Some(override_regions) = &overlay.regions {
        override_regions.clone()
    } else {
        base_regions
    }
}
