pub mod backend_san;
pub mod cert;
pub mod mtls;

pub(crate) use cert::check_key_matches_cert;
pub use cert::{validate_cert, CertValidationError, CertValidationResult};
pub use mtls::{validate_cn_whitelist, validate_san_whitelist};

/// Match a value against a pattern, supporting exact match and single-level wildcard (`*.example.com`).
/// Case-insensitive per RFC 6125 §6.4.1.
///
/// Uses RFC 6125 §6.4.3 semantics: `*.example.com` matches `api.example.com` but NOT
/// `a.b.example.com` (single-label only). This is intentionally stricter than Gateway API
/// route listener matching, which accepts any subdomain depth per the spec. Used for TLS
/// certificate SAN validation at config load time; BoringSSL applies the same single-label
/// rule during the TLS handshake.
pub(crate) fn matches_host_pattern(value: &str, pattern: &str) -> bool {
    if value.eq_ignore_ascii_case(pattern) {
        return true;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        if suffix.is_empty() {
            return false;
        }

        // value must end with ".suffix" (case-insensitive) and have exactly one subdomain label
        if value.len() < suffix.len() + 2 {
            return false;
        }
        let split_pos = value.len() - suffix.len();
        if value.as_bytes()[split_pos - 1] != b'.' || !value[split_pos..].eq_ignore_ascii_case(suffix) {
            return false;
        }
        let subdomain = &value[..split_pos - 1];
        return !subdomain.is_empty() && !subdomain.contains('.');
    }

    false
}
