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
    /// Delegates to [`validate_regions`] when `regions: Some` so controller and gateway
    /// share the exact same rule set (empty list, empty name, empty endpoint, reversed
    /// hash range, duplicate name).
    pub fn validate_and_init(&self) -> Result<(), String> {
        if let Some(regions) = &self.regions {
            validate_regions(regions).map_err(|e| format!("RegionRouteOverride: {}", e))?;
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

/// Validate a slice of [`RegionDef`] for use as an effective region table.
///
/// Checks applied:
/// - At least one region (non-empty).
/// - Each region has a non-empty `name`.
/// - Each region has a non-empty `backendEndpoint` (validated via `parse_endpoint`).
/// - Each region's `hashRange` is valid: `low <= high`.
/// - No duplicate `name` values across regions.
///
/// Called by the gateway after applying an overlay to guard against malformed overlays
/// that slipped past controller validation (race / fed_sync / disabled controller
/// validation). Shareable with controller-side effective-config validation.
pub fn validate_regions(regions: &[RegionDef]) -> Result<(), String> {
    if regions.is_empty() {
        return Err("effective region table is empty".to_string());
    }
    let mut seen_names = std::collections::HashSet::new();
    for r in regions {
        if r.name.is_empty() {
            return Err("region has empty name".to_string());
        }
        if !seen_names.insert(r.name.as_str()) {
            return Err(format!("duplicate region name: '{}'", r.name));
        }
        let (host, _) = r.parse_endpoint();
        if host.is_empty() {
            return Err(format!("region '{}': backendEndpoint is empty", r.name));
        }
        if r.hash_range[0] > r.hash_range[1] {
            return Err(format!(
                "region '{}': hashRange [{}, {}] is invalid (low > high)",
                r.name, r.hash_range[0], r.hash_range[1]
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_region(name: &str, low: u16, high: u16, endpoint: &str) -> RegionDef {
        RegionDef {
            name: name.to_string(),
            hash_range: [low, high],
            backend_endpoint: endpoint.to_string(),
            tls: false,
            failover_to: None,
        }
    }

    #[test]
    fn validate_regions_empty_is_err() {
        assert!(validate_regions(&[]).is_err());
    }

    #[test]
    fn validate_regions_valid_passes() {
        let regions = vec![
            make_region("cn", 0, 499, "cn-backend:443"),
            make_region("us", 500, 999, "us-backend:443"),
        ];
        assert!(validate_regions(&regions).is_ok());
    }

    #[test]
    fn validate_regions_duplicate_name_is_err() {
        let regions = vec![
            make_region("cn", 0, 499, "ep-a:443"),
            make_region("cn", 500, 999, "ep-b:443"), // duplicate
        ];
        let err = validate_regions(&regions).unwrap_err();
        assert!(err.contains("duplicate") && err.contains("cn"), "{}", err);
    }

    #[test]
    fn validate_regions_reversed_hash_range_is_err() {
        let regions = vec![make_region("bad", 500, 0, "ep:443")]; // low > high
        let err = validate_regions(&regions).unwrap_err();
        assert!(err.contains("invalid") && err.contains("bad"), "{}", err);
    }

    #[test]
    fn validate_regions_empty_endpoint_is_err() {
        let regions = vec![make_region("r1", 0, 999, "")]; // empty backendEndpoint
        let err = validate_regions(&regions).unwrap_err();
        assert!(err.contains("backendEndpoint") && err.contains("r1"), "{}", err);
    }

    #[test]
    fn validate_regions_empty_name_is_err() {
        let regions = vec![make_region("", 0, 999, "ep:443")]; // empty name
        let err = validate_regions(&regions).unwrap_err();
        assert!(err.contains("empty name"), "{}", err);
    }

    #[test]
    fn override_validate_and_init_delegates_to_validate_regions() {
        // Regression: RegionRouteOverride::validate_and_init must catch the SAME
        // rules as validate_regions (controller/gateway symmetry).
        let ov = RegionRouteOverride {
            enable: None,
            active: None,
            regions: Some(vec![
                make_region("cn", 0, 499, "ep-a:443"),
                make_region("cn", 500, 999, "ep-b:443"), // duplicate name — was missed pre-fix
            ]),
        };
        let err = ov.validate_and_init().unwrap_err();
        assert!(err.starts_with("RegionRouteOverride: "), "{}", err);
        assert!(err.contains("duplicate") && err.contains("cn"), "{}", err);
    }
}
