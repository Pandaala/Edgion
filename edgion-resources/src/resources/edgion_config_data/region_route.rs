//! RegionRoute overlay EdgionConfigData type
//!
//! `RegionRouteOverride` — all-Optional typed overlay referenced via `overrideRef`
//! from a `RegionRouteConfig` in EdgionPlugins.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// Reserved and currently inert. Use the envelope `spec.enable` to disable
    /// the overlay and fall back to the base table.
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
    /// Called by the controller at EdgionConfigData ingest time (via `validate_pre_parse`).
    /// Delegates to [`validate_regions`] when `regions: Some` so controller and gateway
    /// share endpoint, name, range, overlap and failover-graph rules. Modulo
    /// compatibility is checked separately for each referring plugin.
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
/// Standalone checks are independent of any consumer's modulo. Use
/// `RegionRouteConfig::validate_effective_regions` when the consumer is known.
pub fn validate_regions(regions: &[RegionDef]) -> Result<(), String> {
    RegionTableValidation::new(regions).map(|_| ())
}

/// Proof summary for one immutable, standalone-valid region table.
///
/// Construct at configuration time. Only the maximum hash endpoint is needed
/// to check a consumer's modulo, so request-time validation is constant work.
/// Keep this summary with the exact table generation it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionTableValidation {
    max_hash: u16,
}

impl RegionTableValidation {
    /// Validate names, endpoints, disjoint closed ranges and an acyclic failover
    /// graph in O(n log n) time and O(n) memory, without recursion or I/O.
    pub fn new(regions: &[RegionDef]) -> Result<Self, String> {
        if regions.is_empty() {
            return Err("effective region table is empty".to_string());
        }
        let mut names = HashMap::with_capacity(regions.len());
        let mut ranges = Vec::with_capacity(regions.len());
        for (index, r) in regions.iter().enumerate() {
            if r.name.trim().is_empty() {
                return Err("region has empty name".to_string());
            }
            if names.insert(r.name.as_str(), index).is_some() {
                return Err(format!("duplicate region name: '{}'", r.name));
            }
            r.parse_endpoint()
                .map_err(|e| format!("region '{}': backendEndpoint: {e}", r.name))?;
            if r.hash_range[0] > r.hash_range[1] {
                return Err(format!(
                    "region '{}': hashRange [{}, {}] is invalid (low > high)",
                    r.name, r.hash_range[0], r.hash_range[1]
                ));
            }
            // u16::MAX cannot belong to any supported u16 modulo domain.
            if r.hash_range[1] == u16::MAX {
                return Err(format!(
                    "region '{}': hashRange upper bound must be less than 65535",
                    r.name
                ));
            }
            ranges.push(r.hash_range);
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[1][0] <= pair[0][1]) {
            return Err("region hashRange intervals must not overlap (endpoints are inclusive)".to_string());
        }

        let mut targets = Vec::with_capacity(regions.len());
        for r in regions {
            let target = match r.failover_to.as_deref() {
                None => None,
                Some(name) if name == r.name => {
                    return Err(format!("region '{}': failoverTo cannot reference itself", r.name))
                }
                Some(name) => Some(
                    *names
                        .get(name)
                        .ok_or_else(|| format!("region '{}': failoverTo target '{}' does not exist", r.name, name))?,
                ),
            };
            targets.push(target);
        }
        // 0 = unseen, 1 = this walk, 2 = complete. Iterative walks keep a long
        // operator-authored chain from consuming the call stack.
        let mut state = vec![0u8; regions.len()];
        for start in 0..regions.len() {
            let mut current = Some(start);
            while let Some(index) = current {
                match state[index] {
                    1 => return Err("region failoverTo graph must not contain a cycle".to_string()),
                    2 => break,
                    _ => {
                        state[index] = 1;
                        current = targets[index];
                    }
                }
            }
            current = Some(start);
            while let Some(index) = current {
                if state[index] != 1 {
                    break;
                }
                state[index] = 2;
                current = targets[index];
            }
        }
        Ok(Self {
            max_hash: ranges.last().expect("a non-empty region table has a range")[1],
        })
    }

    /// The only consumer-dependent table check. This does not traverse regions.
    pub fn accepts_modulo(self, modulo: u16) -> bool {
        modulo >= 2 && self.max_hash < modulo
    }

    /// Operator-facing error form of the constant-time consumer check.
    pub fn validate_modulo(self, modulo: u16) -> Result<(), String> {
        if modulo < 2 {
            return Err("hashCalc.modulo must be at least 2".to_string());
        }
        if self.max_hash >= modulo {
            return Err(format!(
                "region hashRange upper bound {} must be less than modulo {modulo}",
                self.max_hash
            ));
        }
        Ok(())
    }
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
    fn table_bounds_are_inclusive_disjoint_and_consumer_specific() {
        let first = make_region("first", 0, 10, "first:80");
        let overlap = make_region("second", 10, 20, "second:80");
        assert!(validate_regions(&[first.clone(), overlap])
            .unwrap_err()
            .contains("overlap"));
        let gap = make_region("second", 12, 20, "second:80");
        let proof = RegionTableValidation::new(&[gap, first]).unwrap();
        assert!(!proof.accepts_modulo(20));
        assert!(proof.accepts_modulo(21));
        assert!(proof.validate_modulo(20).unwrap_err().contains("modulo 20"));
        assert!(!proof.accepts_modulo(0));
        assert!(!proof.accepts_modulo(1));
        assert!(validate_regions(&[make_region("last", 65534, 65534, "last")]).is_ok());
        assert!(validate_regions(&[make_region("last", 65535, 65535, "last")]).is_err());
    }

    #[test]
    fn failover_graph_rejects_dangling_self_and_cycles_but_allows_chains() {
        let mut regions = vec![
            make_region("a", 0, 9, "a"),
            make_region("b", 10, 19, "b"),
            make_region("c", 20, 29, "c"),
        ];
        regions[0].failover_to = Some("a".to_string());
        assert!(validate_regions(&regions).unwrap_err().contains("itself"));
        regions[0].failover_to = Some("missing".to_string());
        assert!(validate_regions(&regions).unwrap_err().contains("does not exist"));
        regions[0].failover_to = Some("b".to_string());
        regions[1].failover_to = Some("c".to_string());
        assert!(validate_regions(&regions).is_ok());
        regions[2].failover_to = Some("a".to_string());
        assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
        regions[2].failover_to = Some("b".to_string());
        assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
        regions[2].failover_to = None;
        regions[0].failover_to = Some("c".to_string());
        assert!(validate_regions(&regions).is_ok(), "converging walks are not cycles");
    }

    #[test]
    fn long_failover_walk_is_iterative() {
        let mut regions: Vec<_> = (0..4000)
            .map(|slot| make_region(&format!("r{slot}"), slot, slot, "backend"))
            .collect();
        for (index, region) in regions.iter_mut().enumerate().take(3999) {
            region.failover_to = Some(format!("r{}", index + 1));
        }
        assert!(validate_regions(&regions).is_ok());
        regions[3999].failover_to = Some("r0".to_string());
        assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
    }

    #[test]
    fn absent_overlay_table_inherits_but_empty_replacement_is_invalid() {
        let mut overlay = RegionRouteOverride {
            enable: None,
            active: None,
            regions: None,
        };
        assert!(overlay.validate_and_init().is_ok());
        overlay.regions = Some(Vec::new());
        assert!(overlay.validate_and_init().is_err());
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
                make_region("cn", 500, 999, "ep-b:443"), // duplicate name
            ]),
        };
        let err = ov.validate_and_init().unwrap_err();
        assert!(err.starts_with("RegionRouteOverride: "), "{}", err);
        assert!(err.contains("duplicate") && err.contains("cn"), "{}", err);
    }
}
