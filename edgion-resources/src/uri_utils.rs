//! Generic URI utilities shared between the routing layer and other
//! consumers (request context, backend selector). Kept in `edgion-resources`
//! so that lower layers don't have to depend on gateway internals just to
//! decode a query string.
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
    if !s.contains(['%', '+']) {
        return s.to_owned();
    }

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
            // Borrow the token after consuming up to two complete Unicode scalars.
            let remaining = chars.as_str();
            chars.next();
            chars.next();
            let hex = &remaining[..remaining.len() - chars.as_str().len()];
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    byte_buf.push(byte);
                    continue;
                }
            }
            // If decode failed, flush any pending bytes and keep the % and hex as is
            flush(&mut byte_buf, &mut result);
            result.push('%');
            result.push_str(hex);
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
#[path = "../tests/unit/uri_utils_test.rs"]
mod tests;
