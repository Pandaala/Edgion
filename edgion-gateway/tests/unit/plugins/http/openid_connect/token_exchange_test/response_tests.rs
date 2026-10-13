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

#[tokio::test]
async fn caller_selected_small_cap_is_enforced() {
    let response = streamed(vec![Ok(Bytes::from_static(b"{}      "))]);
    assert_eq!(response.content_length(), None);
    assert_eq!(
        read_oidc_json::<Value>(response, 8).await.unwrap(),
        serde_json::json!({})
    );

    let response: reqwest::Response = http::Response::new(b"{}       ".to_vec()).into();
    assert_eq!(response.content_length(), Some(9));
    assert_eq!(
        read_oidc_json::<Value>(response, 8).await.unwrap_err(),
        OidcDependencyFailure::ResponseTooLarge
    );

    let response = streamed(vec![Ok(Bytes::from_static(b"{}      ")), Ok(Bytes::from_static(b" "))]);
    assert_eq!(response.content_length(), None);
    assert_eq!(
        read_oidc_json::<Value>(response, 8).await.unwrap_err(),
        OidcDependencyFailure::ResponseTooLarge
    );
}

#[tokio::test]
async fn cancelling_pending_json_read_drops_response_stream() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let probe = DropProbe(Arc::clone(&dropped));
    let parts = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"{}"))]).chain(stream::poll_fn(
        move |_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
            let _ = &probe;
            std::task::Poll::Pending
        },
    ));
    let response = http::Response::new(reqwest::Body::wrap_stream(parts)).into();
    let mut reader = Box::pin(read_oidc_json::<Value>(response, 64));

    assert!(futures::poll!(reader.as_mut()).is_pending());
    assert!(!dropped.load(Ordering::SeqCst));
    drop(reader);
    assert!(dropped.load(Ordering::SeqCst));
}
