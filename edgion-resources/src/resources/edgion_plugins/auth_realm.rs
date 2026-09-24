//! Shared validation for authentication challenge realms.

/// Validate the unescaped realm before configuration is used or a challenge is built.
///
/// Quotes and backslashes are valid data and are escaped by the gateway's canonical
/// challenge writer. ASCII controls (including HTAB) and DEL are rejected rather
/// than stripped. Other UTF-8 bytes are preserved as HTTP quoted-string obs-text.
/// Existing plugin-specific empty-realm checks remain the caller's responsibility.
pub fn validate_auth_realm(realm: &str) -> Result<(), &'static str> {
    if realm.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("realm must not contain ASCII control characters (including tabs and DEL)");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/auth_realm_test.rs"]
mod tests;
