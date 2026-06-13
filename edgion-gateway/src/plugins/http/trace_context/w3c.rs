//! W3C Trace Context (`traceparent`) parsing and generation.
//!
//! Propagation-only: a valid inbound `traceparent` is forwarded byte-for-byte by
//! the caller, so we only need to (a) decide whether an inbound value is
//! well-formed and extract its trace_id (for `x-trace-id` alignment), and
//! (b) generate a fresh, valid root `traceparent` when none is present/trusted.

use uuid::Uuid;

/// A freshly generated root trace context.
pub struct GeneratedRoot {
    /// Full `traceparent` header value (`00-<trace_id>-<span_id>-<flags>`).
    pub traceparent: String,
    /// The 32-hex trace_id (for `x-trace-id` alignment).
    pub trace_id: String,
}

/// Return the 32-hex lowercase trace_id when `value` is a well-formed W3C
/// `traceparent`, else `None` (caller treats `None` as "regenerate or skip").
///
/// Strict, W3C-conformant: lowercase hex only; trace-id 32 hex non-zero;
/// parent-id 16 hex non-zero; trace-flags 2 hex; version `00` => exactly 4
/// dash-fields (no trailing data); version `> 00` (but not `ff`) =>
/// forward-compatible (validate first 4 fields, allow extra trailing fields).
pub fn parse_trace_id(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let parts: Vec<&str> = v.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let version = parts[0];
    if version.len() != 2 || !is_lower_hex(version) || version == "ff" {
        return None;
    }
    if version == "00" && parts.len() != 4 {
        return None;
    }
    let trace_id = parts[1];
    let parent_id = parts[2];
    let flags = parts[3];
    if trace_id.len() != 32 || !is_lower_hex(trace_id) || is_all_zero(trace_id) {
        return None;
    }
    if parent_id.len() != 16 || !is_lower_hex(parent_id) || is_all_zero(parent_id) {
        return None;
    }
    if flags.len() != 2 || !is_lower_hex(flags) {
        return None;
    }
    Some(trace_id.to_string())
}

/// Generate a fresh, valid W3C root `traceparent`. `sampled` selects the
/// trace-flags byte: `true` => `01`, `false` => `00`.
pub fn generate_root(sampled: bool) -> GeneratedRoot {
    let trace_id = Uuid::new_v4().simple().to_string();
    let span_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    let flags = if sampled { "01" } else { "00" };
    GeneratedRoot {
        traceparent: format!("00-{trace_id}-{span_id}-{flags}"),
        trace_id,
    }
}

fn is_lower_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_all_zero(s: &str) -> bool {
    s.bytes().all(|b| b == b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn valid_v00_returns_trace_id() {
        assert_eq!(
            parse_trace_id(VALID).as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert!(parse_trace_id("  ").is_none());
        assert!(parse_trace_id("").is_none());
        assert_eq!(
            parse_trace_id(&format!("  {VALID}  ")).as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
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
            parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(
            parse_trace_id("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-future").as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn generate_root_unsampled_format() {
        let r = generate_root(false);
        assert!(r.traceparent.ends_with("-00"));
        assert!(r.traceparent.starts_with("00-"));
        assert_eq!(parse_trace_id(&r.traceparent).as_deref(), Some(r.trace_id.as_str()));
        assert_eq!(r.trace_id.len(), 32);
    }

    #[test]
    fn generate_root_sampled_flag() {
        assert!(generate_root(true).traceparent.ends_with("-01"));
    }
}
