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
