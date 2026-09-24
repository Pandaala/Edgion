//! Group name validation and defaults shared across plugin grouping types
//! (IpGroup, KeyAuthSecretGroup, HmacAuthSecretGroup, JwtIssuerGroup,
//! BasicAuthSecretGroup).

/// Validate a group name per Kubernetes label-value charset.
///
/// Rules: 1..=63 chars, first and last byte alphanumeric, middle bytes
/// in `[a-zA-Z0-9._-]`. Single-character names are allowed.
pub fn validate_group_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("group name is empty".to_string());
    }
    if name.len() > 63 {
        return Err(format!("group name '{}' exceeds 63 chars", name));
    }
    let bytes = name.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_alphanumeric();
    let is_mid = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.';
    if !is_alnum(bytes[0]) {
        return Err(format!("group name '{}' must start with alphanumeric", name));
    }
    if !is_alnum(bytes[bytes.len() - 1]) {
        return Err(format!("group name '{}' must end with alphanumeric", name));
    }
    if bytes.len() > 2 {
        for &b in &bytes[1..bytes.len() - 1] {
            if !is_mid(b) {
                return Err(format!("group name '{}' contains invalid character", name));
            }
        }
    }
    Ok(())
}

/// Default group name used by `#[serde(default)]` on resolved-credential
/// `group: String` fields across plugin grouping types. Provides
/// rolling-upgrade safety: Gateway-new + Controller-old payloads without
/// the `group` field deserialize with this fallback instead of failing.
pub fn default_group_name() -> String {
    "default".to_string()
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/group_name_test.rs"]
mod tests;
