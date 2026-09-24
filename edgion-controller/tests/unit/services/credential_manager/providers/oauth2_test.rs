use super::*;

fn bootstrap(client_id: &[u8], client_secret: &[u8]) -> BTreeMap<String, ByteString> {
    BTreeMap::from([
        (CLIENT_ID_KEY.to_string(), ByteString(client_id.to_vec())),
        (CLIENT_SECRET_KEY.to_string(), ByteString(client_secret.to_vec())),
    ])
}

#[test]
fn bootstrap_parser_enforces_fixed_keys_utf8_and_bounds_without_leaking_values() {
    let credentials = BootstrapCredentials::parse(&bootstrap(b"client-id-value", b"private-value")).expect("valid");
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("client-id-value"));
    assert!(!debug.contains("private-value"));

    assert_eq!(
        BootstrapCredentials::parse(&BTreeMap::new()).unwrap_err(),
        BootstrapCredentialError::MissingKey(CLIENT_ID_KEY)
    );
    assert_eq!(
        BootstrapCredentials::parse(&bootstrap(&[0xff], b"secret")).unwrap_err(),
        BootstrapCredentialError::InvalidUtf8(CLIENT_ID_KEY)
    );
    assert!(matches!(
        BootstrapCredentials::parse(&bootstrap(b"client", &vec![b'x'; CLIENT_SECRET_MAX_LEN + 1])),
        Err(BootstrapCredentialError::TooLong {
            key: CLIENT_SECRET_KEY,
            ..
        })
    ));
}

#[test]
fn basic_auth_independently_form_encodes_id_and_secret() {
    let credentials = BootstrapCredentials::parse(&bootstrap(b"client id", b"s:e+cret")).expect("valid");
    let header = build_basic_authorization(&credentials).expect("header");
    let encoded = header.to_str().expect("ASCII").strip_prefix("Basic ").expect("scheme");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64");
    assert_eq!(decoded, b"client+id:s%3Ae%2Bcret");
    assert!(header.is_sensitive());
}

#[test]
fn request_form_is_fixed_and_scope_is_encoded() {
    assert_eq!(build_request_form(&[]).unwrap(), "grant_type=client_credentials");
    assert_eq!(
        build_request_form(&["read:data".to_string(), "write/data".to_string()]).unwrap(),
        "grant_type=client_credentials&scope=read%3Adata+write%2Fdata"
    );
}

#[test]
fn success_response_requires_bearer_token_and_integral_bounded_lifetime() {
    let token = parse_success_response(br#"{"access_token":"abc-._~+/=","token_type":"bearer","expires_in":40}"#)
        .expect("valid token");
    assert_eq!(token.access_token, "abc-._~+/=");
    assert_eq!(token.expires_in, Duration::from_secs(40));

    for invalid in [
        br#"{"access_token":"abc","token_type":"MAC","expires_in":40}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":39}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":604801}"#.as_slice(),
        br#"{"access_token":"abc","token_type":"Bearer","expires_in":40.5}"#.as_slice(),
        br#"{"access_token":"bad token","token_type":"Bearer","expires_in":40}"#.as_slice(),
    ] {
        assert_eq!(
            parse_success_response(invalid).unwrap_err().kind,
            OAuth2FailureKind::Permanent
        );
    }
}

#[test]
fn response_classification_only_allows_typed_invalid_client_fallback() {
    let invalid_client = br#"{"error":"invalid_client","error_description":"ignored"}"#;
    assert_eq!(
        classify_response(StatusCode::UNAUTHORIZED, invalid_client, None)
            .unwrap_err()
            .kind,
        OAuth2FailureKind::InvalidClient
    );
    for (status, body, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            br#"{}"#.as_slice(),
            OAuth2FailureKind::Permanent,
        ),
        (
            StatusCode::BAD_REQUEST,
            br#"{"error":"invalid_scope"}"#.as_slice(),
            OAuth2FailureKind::Permanent,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":"slow_down"}"#.as_slice(),
            OAuth2FailureKind::Transient,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            b"not-json".as_slice(),
            OAuth2FailureKind::Transient,
        ),
    ] {
        assert_eq!(classify_response(status, body, None).unwrap_err().kind, expected);
    }
}

#[test]
fn retry_after_is_limited_to_429_and_503_and_capped() {
    let cap = Duration::from_secs(60);
    let value = HeaderValue::from_static("120");
    assert_eq!(
        retry_after_for(StatusCode::TOO_MANY_REQUESTS, Some(&value), cap),
        Some(cap)
    );
    assert_eq!(
        retry_after_for(StatusCode::SERVICE_UNAVAILABLE, Some(&value), cap),
        Some(cap)
    );
    assert_eq!(retry_after_for(StatusCode::BAD_GATEWAY, Some(&value), cap), None);
    assert_eq!(retry_after_for(StatusCode::UNAUTHORIZED, Some(&value), cap), None);
    assert_eq!(
        retry_after_for(StatusCode::TOO_MANY_REQUESTS, Some(&HeaderValue::from_static("0")), cap),
        None
    );
}
