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
mod tests {
    use super::*;
    use http::header::{CONTENT_ENCODING, CONTENT_TYPE};
    use http::HeaderValue;

    fn headers(content_types: &[&str], content_encodings: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in content_types {
            headers.append(CONTENT_TYPE, HeaderValue::from_str(value).expect("valid header value"));
        }
        for value in content_encodings {
            headers.append(
                CONTENT_ENCODING,
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        headers
    }

    fn supported(content_types: &[&str], content_encodings: &[&str]) -> bool {
        json_request_headers_are_supported(&headers(content_types, content_encodings))
    }

    #[test]
    fn accepts_json_media_types_with_supported_parameters() {
        for accepted in [
            "application/json",
            "APPLICATION/JSON",
            " application/json \t",
            "application/problem+json",
            "application/problem+json; charset=utf-8; version=1",
        ] {
            assert!(supported(&[accepted], &[]), "{accepted}");
        }
    }

    #[test]
    fn rejects_missing_ambiguous_or_non_json_content_type() {
        for rejected in [
            "text/json",
            "text/problem+json",
            "application/*+json",
            "application/json; charset=iso-8859-1",
            "application/json; charset=utf-8; charset=utf-8",
            "application/json, text/plain",
        ] {
            assert!(!supported(&[rejected], &[]), "{rejected}");
        }

        assert!(!supported(&[], &[]));
        assert!(!supported(&["application/json", "application/json"], &[]));
    }

    #[test]
    fn allows_only_absent_or_single_identity_content_encoding() {
        assert!(supported(&["application/json"], &[]));
        assert!(supported(&["application/json"], &["identity"]));
        assert!(supported(&["application/json"], &[" IDENTITY \t"]));
        assert!(!supported(&["application/json"], &["gzip"]));
        assert!(!supported(&["application/json"], &["identity, identity"]));
        assert!(!supported(&["application/json"], &["identity", "identity"]));
    }

    #[test]
    fn validates_form_media_type_and_parameters() {
        let supported = |content_type: &str, encoding: &[&str]| {
            form_request_headers_are_supported(&headers(&[content_type], encoding))
        };

        assert!(supported("application/x-www-form-urlencoded", &[]));
        assert!(supported(
            "APPLICATION/X-WWW-FORM-URLENCODED; charset=UTF-8",
            &["identity"]
        ));
        assert!(!supported("application/x-www-form-urlencoded; version=1", &[]));
        assert!(!supported("application/x-www-form-urlencoded; charset=latin1", &[]));
        assert!(!supported(
            "application/x-www-form-urlencoded; charset=utf-8; charset=utf-8",
            &[]
        ));
        assert!(!supported("application/json", &[]));
        assert!(!supported("application/x-www-form-urlencoded", &["gzip"]));
        assert!(!form_request_headers_are_supported(&HeaderMap::new()));
    }

    #[test]
    fn rejects_non_utf8_content_type_and_content_encoding() {
        let mut invalid_content_type = HeaderMap::new();
        invalid_content_type.insert(
            CONTENT_TYPE,
            HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
        );
        assert!(!json_request_headers_are_supported(&invalid_content_type));
        assert!(!form_request_headers_are_supported(&invalid_content_type));

        let mut invalid_content_encoding = headers(&["application/json"], &[]);
        invalid_content_encoding.insert(
            CONTENT_ENCODING,
            HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
        );
        assert!(!json_request_headers_are_supported(&invalid_content_encoding));

        let mut invalid_form_encoding = headers(&["application/x-www-form-urlencoded"], &[]);
        invalid_form_encoding.insert(
            CONTENT_ENCODING,
            HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
        );
        assert!(!form_request_headers_are_supported(&invalid_form_encoding));
    }
}
