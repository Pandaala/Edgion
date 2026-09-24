use super::*;

#[test]
fn test_parse_query_string_basic() {
    let query = "name=john&age=30";
    let params = parse_query_string(query);

    assert_eq!(params.len(), 2);
    assert_eq!(params.get("name"), Some(&"john".to_string()));
    assert_eq!(params.get("age"), Some(&"30".to_string()));
}

#[test]
fn test_parse_query_string_with_encoding() {
    let query = "q=hello+world&filter=%20test";
    let params = parse_query_string(query);

    assert_eq!(params.get("q"), Some(&"hello world".to_string()));
    assert_eq!(params.get("filter"), Some(&" test".to_string()));
}

#[test]
fn test_parse_query_string_no_value() {
    let query = "flag";
    let params = parse_query_string(query);

    assert_eq!(params.len(), 1);
    assert_eq!(params.get("flag"), Some(&String::new()));
}

#[test]
fn test_parse_query_string_empty() {
    let query = "";
    let params = parse_query_string(query);

    assert_eq!(params.len(), 0);
}

#[test]
fn test_url_decode_basic() {
    assert_eq!(url_decode("hello"), "hello");
    assert_eq!(url_decode("hello+world"), "hello world");
    assert_eq!(url_decode("hello%20world"), "hello world");
}

#[test]
fn test_url_decode_special_chars() {
    assert_eq!(url_decode("a%2Bb%3Dc"), "a+b=c");
    assert_eq!(url_decode("100%25"), "100%");
}

#[test]
fn test_url_decode_multibyte_utf8() {
    // 2-byte UTF-8: e-acute (U+00E9 = C3 A9), the typical loanword case.
    assert_eq!(url_decode("%C3%A9"), "é");
    // 3-byte UTF-8: euro sign (U+20AC = E2 82 AC), a currency symbol.
    assert_eq!(url_decode("%E2%82%AC"), "€");
    // Mixed: ASCII + multi-byte + ASCII (`price=€100`).
    assert_eq!(url_decode("price%3D%E2%82%AC100"), "price=€100");
    // Two adjacent multi-byte runs back-to-back: 2-byte then 3-byte
    // (`é€` = C3 A9 E2 82 AC). Guards against decoder state bugs at
    // multi-byte boundaries.
    assert_eq!(url_decode("%C3%A9%E2%82%AC"), "é€");
}

#[test]
fn test_url_decode_invalid_encoding() {
    // Invalid hex sequences should be kept as-is
    assert_eq!(url_decode("test%"), "test%");
    assert_eq!(url_decode("test%GG"), "test%GG");
}
