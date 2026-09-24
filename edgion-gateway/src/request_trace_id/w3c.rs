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
    #[cfg(test)]
    pub fn traceparent(&self) -> &str {
        &self.traceparent
    }

    pub(super) fn into_traceparent(self) -> String {
        self.traceparent
    }

    /// Borrow the 32-hex trace ID used for access-log correlation.
    #[cfg(test)]
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
#[path = "../../tests/unit/request_trace_id/w3c_test.rs"]
mod tests;
