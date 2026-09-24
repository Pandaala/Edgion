//! Shared request-header validation for plugins that parse JSON bodies.

use http::HeaderMap;

/// Return whether the request headers describe one supported JSON
/// representation.
///
/// Accepted requests have exactly one `Content-Type` value with an
/// `application/json` or concrete `application/*+json` media type. An optional
/// charset must be UTF-8. `Content-Encoding` must be absent or contain exactly
/// one `identity` value.
pub fn json_request_headers_are_supported(headers: &HeaderMap) -> bool {
    let Ok(Some(content_type)) = unique_header_value(headers, "content-type") else {
        return false;
    };

    let Ok(content_type) = content_type.trim().parse::<mime::Mime>() else {
        return false;
    };
    let subtype = content_type.subtype();
    let is_json = subtype == mime::JSON || (subtype.as_str() != "*" && content_type.suffix() == Some(mime::JSON));
    if content_type.type_() != mime::APPLICATION || !is_json {
        return false;
    }

    let mut charset_count = 0usize;
    for (name, value) in content_type.params() {
        if name == mime::CHARSET {
            charset_count += 1;
            if charset_count > 1 || !value.as_str().eq_ignore_ascii_case("utf-8") {
                return false;
            }
        }
    }

    supported_identity_encoding(headers)
}

/// Return whether the request headers describe one supported form URL-encoded
/// representation.
///
/// Only `application/x-www-form-urlencoded` is accepted. Its sole optional
/// parameter is `charset=utf-8`. `Content-Encoding` must be absent or contain
/// exactly one `identity` value.
pub fn form_request_headers_are_supported(headers: &HeaderMap) -> bool {
    let Ok(Some(content_type)) = unique_header_value(headers, "content-type") else {
        return false;
    };

    let Ok(content_type) = content_type.trim().parse::<mime::Mime>() else {
        return false;
    };
    if content_type.type_() != mime::APPLICATION || content_type.subtype().as_str() != "x-www-form-urlencoded" {
        return false;
    }

    let mut charset_count = 0usize;
    for (name, value) in content_type.params() {
        if name != mime::CHARSET {
            return false;
        }
        charset_count += 1;
        if charset_count > 1 || !value.as_str().eq_ignore_ascii_case("utf-8") {
            return false;
        }
    }

    supported_identity_encoding(headers)
}

fn unique_header_value<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    value.to_str().map(Some).map_err(|_| ())
}

fn supported_identity_encoding(headers: &HeaderMap) -> bool {
    match unique_header_value(headers, "content-encoding") {
        Ok(None) => true,
        Ok(Some(value)) => value.trim().eq_ignore_ascii_case("identity"),
        Err(()) => false,
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/json_request_test.rs"]
mod tests;
