//! Shared request-header validation for plugins that parse JSON bodies.

/// Return whether the request headers describe one supported JSON
/// representation.
///
/// Accepted requests have exactly one `Content-Type` value with an
/// `application/json` or concrete `application/*+json` media type. An optional
/// charset must be UTF-8. `Content-Encoding` must be absent or contain exactly
/// one `identity` value.
pub fn json_request_headers_are_supported(
    content_types: Option<&[String]>,
    content_encodings: Option<&[String]>,
) -> bool {
    let Some(content_types) = content_types else {
        return false;
    };
    if content_types.len() != 1 {
        return false;
    }

    let Ok(content_type) = content_types[0].trim().parse::<mime::Mime>() else {
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

    let Some(content_encodings) = content_encodings else {
        return false;
    };
    match content_encodings {
        [] => true,
        [value] => value.trim().eq_ignore_ascii_case("identity"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supported(content_types: &[&str], content_encodings: &[&str]) -> bool {
        let content_types: Vec<String> = content_types.iter().map(|value| (*value).to_string()).collect();
        let content_encodings: Vec<String> = content_encodings.iter().map(|value| (*value).to_string()).collect();
        json_request_headers_are_supported(Some(&content_types), Some(&content_encodings))
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
            "application/json, text/plain",
        ] {
            assert!(!supported(&[rejected], &[]), "{rejected}");
        }

        assert!(!supported(&[], &[]));
        assert!(!supported(&["application/json", "application/json"], &[]));
        assert!(!json_request_headers_are_supported(None, Some(&[])));
    }

    #[test]
    fn allows_only_absent_or_single_identity_content_encoding() {
        assert!(supported(&["application/json"], &[]));
        assert!(supported(&["application/json"], &["identity"]));
        assert!(supported(&["application/json"], &[" IDENTITY \t"]));
        assert!(!supported(&["application/json"], &["gzip"]));
        assert!(!supported(&["application/json"], &["identity, identity"]));
        assert!(!supported(&["application/json"], &["identity", "identity"]));
        assert!(!json_request_headers_are_supported(
            Some(&["application/json".to_string()]),
            None
        ));
    }
}
