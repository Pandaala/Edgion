// SQL injection blacklist checker - matches C implementation exactly

use super::sqli_data;

/// Check if a fingerprint is blacklisted
/// This matches libinjection_sqli_blacklist from the C version
pub fn is_blacklisted(fingerprint: &str) -> bool {
    // Prefix the bounded v0 fingerprint for the v1 table lookup.
    // The canonical lookup_word normalizes case before searching the table.

    if fingerprint.is_empty() {
        return false;
    }

    let mut prefixed = [0_u8; 6];
    prefixed[0] = b'0';
    let mut len = 1;
    for byte in fingerprint.bytes() {
        if len == prefixed.len() {
            return false;
        }
        prefixed[len] = byte;
        len += 1;
    }
    let Ok(key) = std::str::from_utf8(&prefixed[..len]) else {
        return false;
    };
    sqli_data::lookup_word(key) == crate::sqli::TokenType::Fingerprint
}
