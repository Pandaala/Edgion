use super::*;

#[test]
fn unescaped_inputs_are_borrowed_and_escaped_inputs_remain_strict() {
    for preserve_slash in [false, true] {
        for value in ["", "plain", "café", "a+b", "count=3"] {
            assert!(matches!(decode_path(value, preserve_slash), Ok(Cow::Borrowed(actual)) if actual == value));
        }
        for (value, expected) in [("hello%20world", "hello world"), ("%C3%A9", "é"), ("a%26b+c", "a&b+c")] {
            assert!(matches!(decode_path(value, preserve_slash), Ok(Cow::Owned(actual)) if actual == expected));
        }
        for value in ["%", "%2", "%XY", "%FF", "%C3"] {
            assert!(
                matches!(decode_path(value, preserve_slash), Err(TranscodingError::Invalid)),
                "{value}"
            );
        }
    }
    assert_eq!(decode_path("a%2Fb%2fc", true).unwrap(), "a%2Fb%2fc");
    assert_eq!(decode_path("a%2Fb%2fc", false).unwrap(), "a/b/c");
}
