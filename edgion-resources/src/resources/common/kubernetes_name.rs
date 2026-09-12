//! Shared Kubernetes DNS name validation.

pub(crate) fn is_dns_label(value: &str, max_len: usize) -> bool {
    if value.is_empty() || value.len() > max_len {
        return false;
    }
    let bytes = value.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub(crate) fn is_dns_subdomain(value: &str, max_len: usize) -> bool {
    !value.is_empty() && value.len() <= max_len && value.split('.').all(|label| is_dns_label(label, 63))
}
