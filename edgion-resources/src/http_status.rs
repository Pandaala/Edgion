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
#[path = "../tests/unit/http_status_test.rs"]
mod tests;
