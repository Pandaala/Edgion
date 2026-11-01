//! Cluster and local-region settings for reference-bound RegionRoute consumers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resources::edgion_plugins::plugin_configs::RegionDef;

/// Complete regional identity and routing table, published as one generation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterRegionSetting {
    pub cluster_name: String,
    pub region_name: String,
    pub regions: Vec<RegionDef>,
}

impl ClusterRegionSetting {
    /// Validate identity and the entire table before publishing its proof.
    pub fn validation_proof(&self) -> Result<RegionTableValidation, String> {
        validate_setting_identity(&self.cluster_name, &self.region_name)?;
        let proof = RegionTableValidation::new(&self.regions)?;
        if !self.regions.iter().any(|region| region.name == self.region_name) {
            return Err("regionName must identify a region in regions".to_string());
        }
        Ok(proof)
    }

    pub fn validate_and_init(&self) -> Result<(), String> {
        self.validation_proof().map(|_| ())
    }
}

/// Validate directory identity independently of enable state and routing-table health.
pub fn validate_setting_identity(cluster_name: &str, region_name: &str) -> Result<(), String> {
    for (field, value) in [("clusterName", cluster_name), ("regionName", region_name)] {
        if value.trim().is_empty() || value.len() > 63 || value.chars().any(char::is_control) {
            return Err(format!("{field} must be nonblank, control-free and at most 63 bytes"));
        }
    }
    Ok(())
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
#[path = "../../../tests/unit/resources/edgion_config_data/region_route_test.rs"]
mod tests;
