//! Group name validation shared across plugin grouping types
//! (IpGroup, KeyAuthSecretGroup, future HmacSecretGroup / JwtIssuerGroup).

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_char() {
        assert!(validate_group_name("a").is_ok());
        assert!(validate_group_name("0").is_ok());
    }

    #[test]
    fn accepts_typical_label() {
        assert!(validate_group_name("partner-a").is_ok());
        assert!(validate_group_name("internal_keys").is_ok());
        assert!(validate_group_name("v1.0").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(validate_group_name("").unwrap_err(), "group name is empty");
    }

    #[test]
    fn rejects_too_long() {
        let name = "a".repeat(64);
        assert!(validate_group_name(&name).unwrap_err().contains("exceeds 63 chars"));
    }

    #[test]
    fn rejects_leading_hyphen() {
        assert!(validate_group_name("-foo")
            .unwrap_err()
            .contains("start with alphanumeric"));
    }

    #[test]
    fn rejects_trailing_dot() {
        assert!(validate_group_name("foo.")
            .unwrap_err()
            .contains("end with alphanumeric"));
    }

    #[test]
    fn rejects_invalid_char() {
        assert!(validate_group_name("foo bar")
            .unwrap_err()
            .contains("invalid character"));
        assert!(validate_group_name("foo/bar")
            .unwrap_err()
            .contains("invalid character"));
    }
}
