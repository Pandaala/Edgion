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
    for plain in ["", "hello", "é€π", "key=value&flag"] {
        assert_eq!(url_decode(plain), plain);
    }
    let long_plain = "abcé€π".repeat(1024);
    assert_eq!(url_decode(&long_plain), long_plain);
    assert_eq!(url_decode(&format!("{long_plain}+tail")), format!("{long_plain} tail"));
    assert_eq!(
        url_decode(&format!("{long_plain}%20tail")),
        format!("{long_plain} tail")
    );
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

#[test]
fn test_url_decode_radix_leading_plus() {
    assert_eq!(url_decode("%+A%+0+%2B"), "\n\0 +");
}

#[test]
fn test_url_decode_failed_tokens_preserve_consumed_plus() {
    assert_eq!(url_decode("%+G+%G++%+"), "%+G %G+ %+");
}

#[test]
fn test_url_decode_malformed_scalar_consumption() {
    for (input, expected) in [
        ("%", "%"),
        ("%A", "%A"),
        ("%G1+", "%G1 "),
        ("%%41", "%%41"),
        ("%é", "%é"),
        ("%éA%20", "%éA "),
        ("%Aé%20", "%Aé "),
        ("%€+%20", "%€+ "),
    ] {
        assert_eq!(url_decode(input), expected, "input: {input}");
    }
}

#[test]
fn test_url_decode_percent_run_flush_boundaries() {
    assert_eq!(url_decode("%C3%A9%GG%E2%82%AC"), "é%GG€");
    assert_eq!(url_decode("%C3%A9x%E2%82%AC+%41"), "éx€ A");
}

#[test]
fn test_url_decode_invalid_utf8_falls_back_for_entire_run() {
    // A valid UTF-8 prefix falls back with the invalid tail in the same run.
    assert_eq!(url_decode("%C3%A9%FF"), "\u{c3}©\u{ff}");
    assert_eq!(url_decode("%E2%82"), "â\u{82}");
    assert_eq!(url_decode("%C3%A9%FF+%C3%A9"), "\u{c3}©\u{ff} é");
}

#[test]
fn test_parse_query_string_decoded_duplicates_and_empty_pairs() {
    let params = parse_query_string("&&name=first&na%6De=last%2Bvalue&flag&&value=a=b&");

    assert_eq!(params.len(), 3);
    assert_eq!(params.get("name").map(String::as_str), Some("last+value"));
    assert_eq!(params.get("flag").map(String::as_str), Some(""));
    assert_eq!(params.get("value").map(String::as_str), Some("a=b"));
}
