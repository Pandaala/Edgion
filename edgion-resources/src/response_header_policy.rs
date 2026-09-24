//! Policy for ordinary configured and extension-produced response mutations.
//!
//! Body and transport owners adjust framing through their private response path.
//! This policy does not restrict request headers or validate header syntax.

use crate::gwapi_types::is_http_framing_or_hop_by_hop_header_name;

/// Whether an ordinary response mutation would change a protocol-owned field.
pub fn is_protected_response_header(name: &str) -> bool {
    is_http_framing_or_hop_by_hop_header_name(name)
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.starts_with(':')
}

/// Apply the same policy to writes, removals, and both ends of a rename.
pub fn validate_response_header_mutation(name: &str) -> Result<(), &'static str> {
    if is_protected_response_header(name) {
        Err("response header mutation targets a protected protocol field")
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/response_header_policy_test.rs"]
mod tests;
