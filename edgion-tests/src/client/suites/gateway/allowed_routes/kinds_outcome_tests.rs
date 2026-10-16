use super::*;
use http::{HeaderMap, HeaderValue, StatusCode};

fn denial_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/grpc"));
    headers.insert("grpc-status", HeaderValue::from_static("5"));
    headers
}

#[test]
fn arch338_accepts_only_completed_empty_native_not_found() {
    let headers = denial_headers();
    assert_eq!(validate_kind_denial(StatusCode::OK, &headers, true), Ok(()));
    assert_eq!(
        validate_kind_denial(StatusCode::OK, &headers, false),
        Err("Expected a completed empty native gRPC kind-denial response")
    );
}

#[test]
fn arch338_rejects_http_errors_even_with_grpc_denial_headers() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::BAD_GATEWAY,
    ] {
        assert_eq!(
            validate_kind_denial(status, &denial_headers(), true),
            Err("Expected HTTP 200 for native gRPC kind denial")
        );
    }
}

#[test]
fn arch338_requires_one_exact_native_content_type() {
    let mut missing = denial_headers();
    missing.remove("content-type");
    let mut duplicate = denial_headers();
    duplicate.append("content-type", HeaderValue::from_static("application/grpc"));
    let mut conflicting = denial_headers();
    conflicting.append("content-type", HeaderValue::from_static("application/json"));
    let mut candidates = vec![missing, duplicate, conflicting];
    for value in [
        HeaderValue::from_static("text/plain"),
        HeaderValue::from_static("application/grpc-web"),
        HeaderValue::from_static("application/grpc+proto"),
        HeaderValue::from_bytes(&[0xff]).unwrap(),
    ] {
        let mut headers = denial_headers();
        headers.insert("content-type", value);
        candidates.push(headers);
    }
    for headers in candidates {
        assert_eq!(
            validate_kind_denial(StatusCode::OK, &headers, true),
            Err("Expected exactly one application/grpc content-type for kind denial")
        );
    }
}

#[test]
fn arch338_rejects_missing_duplicate_and_unrelated_grpc_statuses() {
    let mut missing = denial_headers();
    missing.remove("grpc-status");
    let mut duplicate = denial_headers();
    duplicate.append("grpc-status", HeaderValue::from_static("5"));
    let mut conflicting = denial_headers();
    conflicting.append("grpc-status", HeaderValue::from_static("14"));
    let mut candidates = vec![missing, duplicate, conflicting];
    for value in ["", "0", "1", "4", "7", "12", "13", "14", "05", "5, 14", "NOT_FOUND"] {
        let mut headers = denial_headers();
        headers.insert("grpc-status", HeaderValue::from_static(value));
        candidates.push(headers);
    }
    let mut binary = denial_headers();
    binary.insert("grpc-status", HeaderValue::from_bytes(&[0xff]).unwrap());
    candidates.push(binary);
    for headers in candidates {
        assert_eq!(
            validate_kind_denial(StatusCode::OK, &headers, true),
            Err("Expected exactly one grpc-status 5 for kind denial")
        );
    }
}

#[test]
fn arch338_preserves_both_original_integration_case_identities() {
    let names: Vec<_> = AllowedRoutesKindsTestSuite
        .test_cases()
        .into_iter()
        .map(|case| case.name)
        .collect();
    assert_eq!(names, ["http_route_allowed", "grpc_route_denied"]);
}
