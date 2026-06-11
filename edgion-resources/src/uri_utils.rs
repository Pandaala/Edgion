//! Generic URI utilities shared between the routing layer and other
//! consumers (request context, backend selector). Kept in `types/` so that
//! lower layers don't have to reach into `core::gateway::routes::http`
//! internals just to decode a query string.
//!
//! Decoding semantics match `application/x-www-form-urlencoded`:
//! `%XX` percent-decoded, `+` translated to space, invalid escapes
//! preserved literally. The DSL plugin layer keeps its own decoder
//! because of distinct string-length limit checks; that one is intentionally
//! not unified here.

use std::collections::HashMap;

/// Parse a `&`-separated query string into a key→value map. Both keys and
/// values are percent-decoded via [`url_decode`]. Empty pairs are skipped;
/// a pair without `=` yields an empty value.
pub fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();

    if query.is_empty() {
        return params;
    }

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }

        if let Some(eq_pos) = pair.find('=') {
            let key = url_decode(&pair[..eq_pos]);
            let value = url_decode(&pair[eq_pos + 1..]);
            params.insert(key, value);
        } else {
            let key = url_decode(pair);
            params.insert(key, String::new());
        }
    }

    params
}

/// Percent-decode a string with `+` → space (form-urlencoded semantics).
/// Multi-byte UTF-8 sequences are reassembled across consecutive `%XX`
/// runs; invalid escapes (`%` not followed by two hex digits) are
/// preserved as-is.
pub fn url_decode(s: &str) -> String {
    let mut result = String::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut chars = s.chars();

    // Flush accumulated percent-encoded bytes as UTF-8 into result
    let flush = |buf: &mut Vec<u8>, out: &mut String| {
        if !buf.is_empty() {
            match std::str::from_utf8(buf) {
                Ok(s) => out.push_str(s),
                Err(_) => {
                    // Invalid UTF-8: push each byte as Latin-1 char
                    for &b in buf.iter() {
                        out.push(b as char);
                    }
                }
            }
            buf.clear();
        }
    };

    while let Some(c) = chars.next() {
        if c == '%' {
            // Try to decode %XX
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    byte_buf.push(byte);
                    continue;
                }
            }
            // If decode failed, flush any pending bytes and keep the % and hex as is
            flush(&mut byte_buf, &mut result);
            result.push('%');
            result.push_str(&hex);
        } else {
            flush(&mut byte_buf, &mut result);
            if c == '+' {
                result.push(' ');
            } else {
                result.push(c);
            }
        }
    }

    flush(&mut byte_buf, &mut result);
    result
}

#[cfg(test)]
mod tests {
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
}
