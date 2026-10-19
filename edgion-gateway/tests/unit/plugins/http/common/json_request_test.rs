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
    let supported =
        |content_type: &str, encoding: &[&str]| form_request_headers_are_supported(&headers(&[content_type], encoding));

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

fn original_supported(headers: &HeaderMap, form: bool) -> bool {
    let Ok(Some(value)) = unique_header_value(headers, "content-type") else {
        return false;
    };
    let Ok(media) = value.trim().parse::<mime::Mime>() else {
        return false;
    };
    let subtype = media.subtype();
    let supported = if form {
        subtype.as_str() == "x-www-form-urlencoded"
    } else {
        subtype == mime::JSON || (subtype.as_str() != "*" && media.suffix() == Some(mime::JSON))
    };
    if media.type_() != mime::APPLICATION || !supported {
        return false;
    }
    let mut charset_count = 0;
    for (name, value) in media.params() {
        if form && name != mime::CHARSET {
            return false;
        }
        if name == mime::CHARSET {
            charset_count += 1;
            if charset_count > 1 || !value.as_str().eq_ignore_ascii_case("utf-8") {
                return false;
            }
        }
    }
    supported_identity_encoding(headers)
}

#[test]
fn bare_media_fast_paths_match_original_parser_and_encoding_gates() {
    let types = [
        "application/json",
        "ApPlIcAtIoN/JsOn",
        " application/json \t",
        "application/x-www-form-urlencoded",
        " APPLICATION/X-WWW-FORM-URLENCODED \t",
        "application/json; charset=utf-8",
        "application/json; charset=\"UTF-8\"",
        "application/json; charset=latin1",
        "application/json; charset=utf-8; charset=utf-8",
        "application/json; version=1",
        "application/problem+json",
        "application/*+json",
        "application/x-www-form-urlencoded; charset=utf-8",
        "application/x-www-form-urlencoded; version=1",
        "application/x-www-form-urlencoded+json",
        "application/jsonx",
        "application/json, text/plain",
        "text/json",
        "application /json",
        "application/json; charset",
        "",
        "*/*",
    ];
    let encodings: &[&[&str]] = &[
        &[],
        &["identity"],
        &[" IDENTITY \t"],
        &["gzip"],
        &[""],
        &["identity, identity"],
        &["identity", "identity"],
    ];
    for content_type in types {
        for encoding in encodings {
            for copies in [0, 1, 2] {
                let values = vec![content_type; copies];
                let fixture = headers(&values, encoding);
                assert_eq!(
                    json_request_headers_are_supported(&fixture),
                    original_supported(&fixture, false)
                );
                assert_eq!(
                    form_request_headers_are_supported(&fixture),
                    original_supported(&fixture, true)
                );
            }
        }
    }
    for form in [false, true] {
        let content_type = if form {
            "application/x-www-form-urlencoded"
        } else {
            "application/json"
        };
        for invalid_name in [CONTENT_TYPE, CONTENT_ENCODING] {
            let mut fixture = headers(&[content_type], &[]);
            fixture.insert(invalid_name, HeaderValue::from_bytes(&[0xff]).unwrap());
            let actual = if form {
                form_request_headers_are_supported(&fixture)
            } else {
                json_request_headers_are_supported(&fixture)
            };
            assert!(!actual);
            assert_eq!(actual, original_supported(&fixture, form));
        }
    }
}

#[test]
fn bare_media_validation_avoids_mime_allocation() {
    for (form, value) in [
        (false, "application/json"),
        (false, " APPLICATION/JSON \t"),
        (true, "application/x-www-form-urlencoded"),
        (true, " APPLICATION/X-WWW-FORM-URLENCODED \t"),
    ] {
        for encoding in [&[][..], &["identity"][..]] {
            let fixture = headers(&[value], encoding);
            let (old, old_allocations) =
                crate::test_allocation_counter::count_allocations(|| original_supported(&fixture, form));
            let (new, new_allocations) = crate::test_allocation_counter::count_allocations(|| {
                if form {
                    form_request_headers_are_supported(&fixture)
                } else {
                    json_request_headers_are_supported(&fixture)
                }
            });
            assert!(old && new);
            assert!(old_allocations > 0);
            assert_eq!(new_allocations, 0);
        }
    }
}
