use super::*;

fn original_decode(source: &str, remaining: &mut usize) -> Result<String, GraphqlError> {
    let source = source.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if *remaining == 0 {
            return Err(GraphqlError::TooLarge);
        }
        let byte = match source[i] {
            b'+' => b' ',
            b'%' => {
                fn hex(b: u8) -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                }
                let a = source.get(i + 1).and_then(|b| hex(*b)).ok_or(GraphqlError::Invalid)?;
                let b = source.get(i + 2).and_then(|b| hex(*b)).ok_or(GraphqlError::Invalid)?;
                i += 2;
                a * 16 + b
            }
            b => b,
        };
        result.push(byte);
        *remaining -= 1;
        i += 1;
    }
    String::from_utf8(result).map_err(|_| GraphqlError::Invalid)
}

#[test]
fn decoding_preserves_bytes_errors_and_remaining_budget() {
    let long = "a".repeat(4096);
    for source in [
        "",
        "plain",
        "café",
        "€",
        "a+b",
        "%20",
        "%2B",
        "%C3%A9",
        "%ff",
        "%C3",
        "%",
        "%0",
        "%GG",
        "a%GG",
        "aa%GG",
        "aaa%GG",
        "é%GG",
        "abc+def%20ghi",
        "a%00b",
        "query Q{x}",
        r#"{"n":2}"#,
        long.as_str(),
    ] {
        for limit in (0..=source.len() + 1).chain(std::iter::once(usize::MAX)) {
            let mut old_remaining = limit;
            let mut new_remaining = limit;
            let old = original_decode(source, &mut old_remaining);
            let new = decode(source, &mut new_remaining);
            assert_eq!(new.as_deref(), old.as_deref(), "{source:?}, {limit}");
            assert_eq!(new_remaining, old_remaining, "{source:?}, {limit}");
        }
    }
}

#[test]
fn plain_components_borrow_input_and_escaped_components_own_output() {
    for source in ["", "plain", "café", "€", r#"{"n":2}"#] {
        let mut remaining = source.len() + 2;
        let decoded = decode(source, &mut remaining).unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded.as_ptr(), source.as_ptr());
        assert_eq!(remaining, 2);
    }
    for (source, expected) in [("a+b", "a b"), ("%2B", "+"), ("%C3%A9", "é")] {
        let mut remaining = 100;
        let decoded = decode(source, &mut remaining).unwrap();
        assert!(matches!(decoded, Cow::Owned(_)));
        assert_eq!(decoded, expected);
        assert_eq!(remaining, 100 - expected.len());
    }
}
