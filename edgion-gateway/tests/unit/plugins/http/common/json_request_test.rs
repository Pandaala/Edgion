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
