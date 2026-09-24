use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};
use std::collections::{HashMap, HashSet};

pub const INVALID_HEADER_DESTINATION: &str = "claim-to-header destination must be a valid HTTP header name";
pub const DUPLICATE_HEADER_DESTINATION: &str = "claim-to-header destinations must be unique ignoring ASCII case";

/// One prevalidated claim/payload path to upstream-header projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimHeaderProjectionEntry {
    claim_path: String,
    canonical_header_name: String,
}

impl ClaimHeaderProjectionEntry {
    pub fn claim_path(&self) -> &str {
        &self.claim_path
    }

    pub fn header_name(&self) -> &str {
        &self.canonical_header_name
    }
}

/// Deterministic request-path plan compiled from an operator mapping.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClaimHeaderProjectionPlan {
    entries: Vec<ClaimHeaderProjectionEntry>,
}

impl ClaimHeaderProjectionPlan {
    pub fn entries(&self) -> &[ClaimHeaderProjectionEntry] {
        &self.entries
    }
}

/// Validate and compile a claim/payload-to-header mapping before request processing.
pub fn compile_claim_header_projection(
    mapping: Option<&HashMap<String, String>>,
) -> Result<ClaimHeaderProjectionPlan, &'static str> {
    compile_claim_header_projection_with_owned_destinations(mapping, &[])
}

/// Compile a mapping after reserving destinations already written by this plugin path.
pub fn compile_claim_header_projection_with_owned_destinations(
    mapping: Option<&HashMap<String, String>>,
    owned_destinations: &[&str],
) -> Result<ClaimHeaderProjectionPlan, &'static str> {
    let Some(mapping) = mapping else {
        return Ok(ClaimHeaderProjectionPlan::default());
    };

    let mut seen_destinations = HashSet::with_capacity(mapping.len() + owned_destinations.len());
    seen_destinations.extend(owned_destinations.iter().map(|name| name.to_ascii_lowercase()));
    let mut entries = Vec::with_capacity(mapping.len());

    for (claim_path, header_name) in mapping {
        if header_name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(header_name) {
            return Err(INVALID_HEADER_DESTINATION);
        }

        let canonical_header_name = header_name.to_ascii_lowercase();
        if !seen_destinations.insert(canonical_header_name.clone()) {
            return Err(DUPLICATE_HEADER_DESTINATION);
        }

        entries.push(ClaimHeaderProjectionEntry {
            claim_path: claim_path.clone(),
            canonical_header_name,
        });
    }

    entries.sort_unstable_by(|left, right| left.canonical_header_name.cmp(&right.canonical_header_name));
    Ok(ClaimHeaderProjectionPlan { entries })
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/auth_header_projection_test.rs"]
mod tests;
