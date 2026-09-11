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
mod tests {
    use super::*;

    #[test]
    fn compilation_canonicalizes_destinations_and_is_stable_across_insertion_order() {
        let first = HashMap::from([
            ("subject".to_string(), "X-Subject".to_string()),
            ("tenant".to_string(), "X-Tenant".to_string()),
        ]);
        let second = HashMap::from([
            ("tenant".to_string(), "X-Tenant".to_string()),
            ("subject".to_string(), "X-Subject".to_string()),
        ]);

        let first = compile_claim_header_projection(Some(&first)).unwrap();
        let second = compile_claim_header_projection(Some(&second)).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.entries()[0].claim_path(), "subject");
        assert_eq!(first.entries()[0].header_name(), "x-subject");
        assert_eq!(first.entries()[1].claim_path(), "tenant");
        assert_eq!(first.entries()[1].header_name(), "x-tenant");
    }

    #[test]
    fn compilation_rejects_case_insensitive_duplicate_destinations() {
        let mapping = HashMap::from([
            ("subject".to_string(), "X-Identity".to_string()),
            ("email".to_string(), "x-identity".to_string()),
        ]);

        assert_eq!(
            compile_claim_header_projection(Some(&mapping)),
            Err(DUPLICATE_HEADER_DESTINATION)
        );
    }

    #[test]
    fn compilation_rejects_destinations_already_owned_by_the_plugin() {
        let mapping = HashMap::from([("subject".to_string(), "X-Identity".to_string())]);

        assert_eq!(
            compile_claim_header_projection_with_owned_destinations(Some(&mapping), &["x-identity"]),
            Err(DUPLICATE_HEADER_DESTINATION)
        );
    }

    #[test]
    fn compilation_rejects_invalid_destination_names() {
        for destination in ["", "bad header", "x-bad\r\nname"] {
            let mapping = HashMap::from([("subject".to_string(), destination.to_string())]);
            assert_eq!(
                compile_claim_header_projection(Some(&mapping)),
                Err(INVALID_HEADER_DESTINATION)
            );
        }
    }
}
