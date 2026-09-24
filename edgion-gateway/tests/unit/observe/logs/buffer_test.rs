use super::*;

#[test]
fn log_buffer_accepts_until_128_bytes() {
    let mut b = LogBuffer::new();
    // 16-byte entries: 8 entries == 128 bytes exactly fit, the 9th is rejected.
    for i in 0..8 {
        assert!(b.push("0123456789abcdef"), "entry {i} should fit the 128-byte budget");
    }
    assert!(
        !b.push("0123456789abcdef"),
        "9th entry must be rejected (byte budget full)"
    );
}

#[test]
fn log_buffer_accepts_one_entry_per_byte() {
    let mut b = LogBuffer::new();
    // 1-byte entries: all 128 fit within the 128-byte budget.
    for i in 0..128 {
        assert!(b.push("x"), "entry {i} should fit under the byte budget");
    }
    assert!(!b.push("x"), "129th byte must be rejected");
}

#[test]
fn log_buffer_bounds_zero_length_entries() {
    let mut b = LogBuffer::new();
    for i in 0..BUFFER_CAPACITY {
        assert!(b.push(""), "empty entry {i} should fit the entry budget");
    }
    assert!(!b.push(""), "entry index must remain bounded when entries use no bytes");
    assert_eq!(b.entries().count(), BUFFER_CAPACITY);
}

#[test]
fn log_buffer_rejects_oversize_entry_without_partial_write() {
    let mut b = LogBuffer::new();
    let big = "y".repeat(200);
    assert!(!b.push(&big), "an over-budget entry is rejected");
    // The buffer was not partially mutated: a small entry still fits.
    assert!(b.push("ok"), "buffer untouched after a rejected oversize push");
}

#[test]
fn log_buffer_push_err_preserves_prefix_and_entry_boundaries() {
    let mut b = LogBuffer::new();
    assert!(b.push("OK"));
    assert!(b.push_err("dependency fail"));

    assert_eq!(b.entries().collect::<Vec<_>>(), ["OK", "err: dependency fail"]);
    assert_eq!(serde_json::to_string(&b).unwrap(), r#"["OK","err: dependency fail"]"#);
}

#[test]
fn ulog_buffer_enforces_byte_cap_without_partial_write() {
    let mut b = ULogBuffer::new();
    assert!(b.push(&"x".repeat(ULOG_MAX_BUFFER)));
    assert!(!b.push("y"), "an entry beyond the byte cap must be rejected");
    assert_eq!(b.entries().collect::<Vec<_>>(), ["x".repeat(ULOG_MAX_BUFFER)]);
}

#[test]
fn ulog_buffer_bounds_zero_length_entries() {
    let mut b = ULogBuffer::new();
    for i in 0..ULOG_MAX_BUFFER {
        assert!(b.push(""), "empty entry {i} should fit the entry budget");
    }
    assert!(!b.push(""), "entry index must remain bounded when entries use no bytes");
    assert_eq!(b.entries().count(), ULOG_MAX_BUFFER);
}

#[test]
fn ulog_buffer_push_err_preserves_prefix_and_entry_boundaries() {
    let mut b = ULogBuffer::new();
    assert!(b.push("OK"));
    assert!(b.push_err("dependency fail"));

    assert_eq!(b.entries().collect::<Vec<_>>(), ["OK", "err: dependency fail"]);
    assert_eq!(serde_json::to_string(&b).unwrap(), r#"["OK","err: dependency fail"]"#);
}
