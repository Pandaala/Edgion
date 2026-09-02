//! W3C Trace Context (`traceparent`) parsing and generation.
//!
//! Propagation-only: a valid inbound `traceparent` is forwarded byte-for-byte by
//! the caller, so we only need to (a) decide whether an inbound value is
//! well-formed and extract its trace_id (for `x-trace-id` alignment), and
//! (b) generate a fresh, valid root `traceparent` when none is present/trusted.

use uuid::Uuid;

const TRACEPARENT_LEN: usize = 55;
const TRACE_ID_RANGE: std::ops::Range<usize> = 3..35;
const PARENT_ID_RANGE: std::ops::Range<usize> = 36..52;
const FLAGS_RANGE: std::ops::Range<usize> = 53..55;
const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

/// A freshly generated root trace context.
pub struct GeneratedRoot {
    /// Full `traceparent` header value (`00-<trace_id>-<span_id>-<flags>`).
    traceparent: String,
}

impl GeneratedRoot {
    pub fn traceparent(&self) -> &str {
        &self.traceparent
    }

    /// Borrow the 32-hex trace ID used for access-log correlation.
    pub fn trace_id(&self) -> &str {
        &self.traceparent[TRACE_ID_RANGE]
    }
}

/// Return the borrowed 32-hex lowercase trace_id when `value` is a well-formed W3C
/// `traceparent`, else `None` (caller treats `None` as "regenerate or skip").
///
/// Strict, W3C-conformant: lowercase hex only; trace-id 32 hex non-zero;
/// parent-id 16 hex non-zero; trace-flags 2 hex; version `00` => exactly 4
/// dash-fields (no trailing data); version `> 00` (but not `ff`) =>
/// forward-compatible (validate first 4 fields, allow extra trailing fields).
pub fn parse_trace_id(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    if bytes.len() < TRACEPARENT_LEN || bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
        return None;
    }

    let version = &bytes[..2];
    if !is_lower_hex(version) || version == b"ff" {
        return None;
    }
    if version == b"00" {
        if bytes.len() != TRACEPARENT_LEN {
            return None;
        }
    } else if bytes.len() > TRACEPARENT_LEN && bytes[TRACEPARENT_LEN] != b'-' {
        return None;
    }

    let trace_id = &bytes[TRACE_ID_RANGE];
    if !is_lower_hex(trace_id) || is_all_zero(trace_id) {
        return None;
    }
    let parent_id = &bytes[PARENT_ID_RANGE];
    if !is_lower_hex(parent_id) || is_all_zero(parent_id) {
        return None;
    }
    if !is_lower_hex(&bytes[FLAGS_RANGE]) {
        return None;
    }

    std::str::from_utf8(trace_id).ok()
}

/// Generate a fresh, valid W3C root `traceparent`. `sampled` selects the
/// trace-flags byte: `true` => `01`, `false` => `00`.
pub fn generate_root(sampled: bool) -> GeneratedRoot {
    let trace_uuid = Uuid::new_v4();
    let span_uuid = Uuid::new_v4();
    let mut traceparent = String::with_capacity(TRACEPARENT_LEN);
    traceparent.push_str("00-");
    push_lower_hex(&mut traceparent, trace_uuid.as_bytes());
    traceparent.push('-');
    push_lower_hex(&mut traceparent, &span_uuid.as_bytes()[..8]);
    traceparent.push_str(if sampled { "-01" } else { "-00" });
    GeneratedRoot { traceparent }
}

fn push_lower_hex(output: &mut String, bytes: &[u8]) {
    for byte in bytes {
        output.push(LOWER_HEX[(byte >> 4) as usize] as char);
        output.push(LOWER_HEX[(byte & 0x0f) as usize] as char);
    }
}

fn is_lower_hex(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_all_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| *b == b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn valid_v00_returns_borrowed_trace_id() {
        let trace_id = parse_trace_id(VALID).unwrap();
        assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(trace_id.as_ptr(), VALID.as_ptr().wrapping_add(3));
    }

    #[test]
    fn surrounding_whitespace_is_rejected() {
        assert!(parse_trace_id("  ").is_none());
        assert!(parse_trace_id("").is_none());
        assert!(parse_trace_id(&format!(" {VALID}")).is_none());
        assert!(parse_trace_id(&format!("{VALID} ")).is_none());
    }

    #[test]
    fn uppercase_hex_is_rejected() {
        assert!(parse_trace_id("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn wrong_lengths_rejected() {
        assert!(parse_trace_id("00-4bf9-00f067aa0ba902b7-01").is_none());
        assert!(parse_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-00f0-01").is_none());
        assert!(parse_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0100").is_none());
        assert!(parse_trace_id("00x4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn all_zero_ids_rejected() {
        assert!(parse_trace_id("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none());
        assert!(parse_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01").is_none());
    }

    #[test]
    fn trailing_data_on_v00_rejected() {
        assert!(parse_trace_id(&format!("{VALID}-extra")).is_none());
    }

    #[test]
    fn non_hex_rejected() {
        assert!(parse_trace_id("00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-00f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn version_ff_rejected() {
        assert!(parse_trace_id("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn higher_version_forward_compatible() {
        assert_eq!(
            parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(
            parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-future"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(
            parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-π"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert!(parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01future").is_none());
    }

    #[test]
    fn non_ascii_in_fixed_fields_is_rejected_without_panicking() {
        assert!(parse_trace_id("00-ébf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
        assert!(parse_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-é0f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn generate_root_unsampled_format() {
        let r = generate_root(false);
        assert_eq!(r.traceparent().len(), TRACEPARENT_LEN);
        assert!(r.traceparent().ends_with("-00"));
        assert!(r.traceparent().starts_with("00-"));
        assert_eq!(parse_trace_id(r.traceparent()), Some(r.trace_id()));
        assert_eq!(r.trace_id().len(), 32);
        assert_eq!(r.trace_id().as_ptr(), r.traceparent().as_ptr().wrapping_add(3));
        assert!(!is_all_zero(r.trace_id().as_bytes()));
        assert!(!is_all_zero(r.traceparent()[PARENT_ID_RANGE].as_bytes()));
    }

    #[test]
    fn generate_root_sampled_flag() {
        assert!(generate_root(true).traceparent().ends_with("-01"));
    }
}
