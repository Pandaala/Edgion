//! Shared HTTP final-response status validation.

/// Return whether `status` may represent an ordinary final HTTP response.
///
/// Informational responses are never ordinary final responses. A real
/// `101 Switching Protocols` is terminal only when the transport reports that
/// it was actually written; callers must not use this validator to preselect
/// or claim a protocol switch.
#[inline]
pub const fn is_valid_final_http_status(status: u16) -> bool {
    status >= 200 && status <= 599
}

#[cfg(test)]
mod tests {
    use super::is_valid_final_http_status;

    #[test]
    fn ordinary_final_status_range_excludes_informational_and_invalid_values() {
        for status in [100, 101, 102, 103, 199, 600, 700] {
            assert!(!is_valid_final_http_status(status), "{status}");
        }
        for status in [200, 204, 399, 499, 599] {
            assert!(is_valid_final_http_status(status), "{status}");
        }
    }
}
