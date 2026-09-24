use super::*;
use futures::{stream, StreamExt};
use std::io;

fn streamed(parts: Vec<Result<Bytes, io::Error>>) -> reqwest::Response {
    http::Response::new(reqwest::Body::wrap_stream(stream::iter(parts))).into()
}

fn padded_json(size: usize) -> Vec<u8> {
    let mut body = br#"{"access_token":"fixture-token"}"#.to_vec();
    body.resize(size, b' ');
    body
}

#[tokio::test]
async fn known_oversize_body_is_rejected() {
    let response = http::Response::new(vec![b'x'; AUTH_RESPONSE_MAX_BYTES + 1]).into();
    assert_eq!(
        read_auth_response::<Value>(response, OidcOperation::TokenExchange)
            .await
            .unwrap_err(),
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseTooLarge)
    );
}

#[tokio::test]
async fn exact_limit_stream_preserves_token_fields() {
    let body = padded_json(AUTH_RESPONSE_MAX_BYTES);
    let parts = body.chunks(8192).map(|part| Ok(Bytes::copy_from_slice(part))).collect();
    let token: TokenEndpointResponse = read_auth_response(streamed(parts), OidcOperation::TokenExchange)
        .await
        .unwrap();
    assert_eq!(token.access_token, "fixture-token");
}

#[tokio::test]
async fn stream_stops_at_first_oversize_chunk_without_parsing_prefix() {
    let parts = stream::iter([
        Ok::<_, io::Error>(Bytes::from(padded_json(AUTH_RESPONSE_MAX_BYTES))),
        Ok(Bytes::from_static(b" ")),
    ])
    .chain(stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
            panic!("oversized response must be dropped without polling the remainder")
        },
    ));
    let response = http::Response::new(reqwest::Body::wrap_stream(parts)).into();
    assert_eq!(
        read_auth_response::<TokenEndpointResponse>(response, OidcOperation::TokenExchange)
            .await
            .unwrap_err(),
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseTooLarge)
    );
}

#[tokio::test]
async fn body_read_failure_is_static_and_rejects_valid_prefix() {
    let response = streamed(vec![
        Ok(Bytes::from_static(br#"{"access_token":"fixture-token"}"#)),
        Err(io::Error::other("private-provider-detail")),
    ]);
    let error = read_auth_response::<TokenEndpointResponse>(response, OidcOperation::TokenExchange)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseRead)
    );
    assert_eq!(error.user_message(), "Bad Gateway");
    assert_eq!(error.log_tag(), "exchange response read fail");
}

#[tokio::test]
async fn malformed_or_wrong_shape_json_has_no_provider_detail() {
    for body in [
        b"{private-provider-detail".as_slice(),
        br#"{"access_token":{"private":"credential"}}"#,
    ] {
        let error = read_auth_response::<TokenEndpointResponse>(
            http::Response::new(body.to_vec()).into(),
            OidcOperation::TokenExchange,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseJson)
        );
        assert_eq!(error.user_message(), "Bad Gateway");
        assert_eq!(error.log_tag(), "exchange response json fail");
    }
}

#[test]
fn response_errors_keep_wire_and_contextual_log_contracts() {
    for error in [
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseTooLarge),
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseRead),
        OidcVerifyError::dependency(OidcOperation::TokenExchange, OidcDependencyFailure::ResponseJson),
    ] {
        assert_eq!(error.http_status(), 502);
        assert_eq!(error.user_message(), "Bad Gateway");
        assert_eq!(error.response_log_tag_or("token refresh fail"), error.log_tag());
    }
    assert_eq!(
        OidcVerifyError::Expired.response_log_tag_or("token refresh fail"),
        "token refresh fail"
    );
}
