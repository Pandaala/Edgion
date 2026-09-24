//! mTLS client certificate validation at application layer
//!
//! Validates client certificate SAN/CN against whitelist after TLS handshake

use crate::ctx::ClientCertInfo;
use edgion_common::allowed_san::{client_sans_match_allowed, PresentedSan};
use edgion_resources::resources::edgion_tls::AllowedSan;

/// Validate presented client SANs against the typed allowlist.
pub fn validate_san_whitelist(presented: &[PresentedSan], allowed_sans: &[AllowedSan]) -> bool {
    client_sans_match_allowed(presented, allowed_sans)
}

/// Validate client certificate against CN whitelist
/// Returns true if certificate matches whitelist or no whitelist configured
pub fn validate_cn_whitelist(cert_info: &ClientCertInfo, allowed_cns: &[String]) -> bool {
    if allowed_cns.is_empty() {
        return true;
    }

    let Some(ref cn) = cert_info.cn else {
        return false;
    };

    allowed_cns.iter().any(|allowed| cn == allowed)
}

#[cfg(test)]
#[path = "../../../tests/unit/tls/validation/mtls_test.rs"]
mod tests;
