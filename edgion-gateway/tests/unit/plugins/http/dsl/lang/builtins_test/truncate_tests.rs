use super::truncate_to_max_string_len;

#[test]
fn under_limit_is_unchanged() {
    assert_eq!(truncate_to_max_string_len("abc".to_string(), 8), "abc");
    assert_eq!(truncate_to_max_string_len("abcdefgh".to_string(), 8), "abcdefgh");
}

#[test]
fn over_limit_truncated_to_byte_len() {
    assert_eq!(truncate_to_max_string_len("abcdefghij".to_string(), 4), "abcd");
    // limit 0 yields the empty string.
    assert_eq!(truncate_to_max_string_len("abc".to_string(), 0), "");
}

#[test]
fn truncation_walks_back_to_char_boundary() {
    // "é" is 2 UTF-8 bytes (0xC3 0xA9). Byte layout of "aéé": a@0, é@1..3, é@3..5.
    // Char boundaries are at {0, 1, 3, 5}.
    let s = "aéé".to_string();
    // limit 2 splits the first "é" mid-codepoint; walk back to boundary 1 ("a").
    assert_eq!(truncate_to_max_string_len(s.clone(), 2), "a");
    // limit 3 already lands on a boundary; no walk-back, keep "aé".
    let out = truncate_to_max_string_len(s, 3);
    assert_eq!(out, "aé");
    assert_eq!(out.len(), 3);
}
