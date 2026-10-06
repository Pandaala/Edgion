use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn render(error: AiProxyError) -> (u16, serde_json::Value) {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "content-type" && value == "application/json")
        .times(1)
        .returning(|_, _| Ok(()));
    match openai_error_response(&mut session, error) {
        PluginRunningResult::ErrResponse {
            status,
            body: Some(body),
        } => (status, serde_json::from_slice(&body).unwrap()),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn fixed_error_mappings_have_stable_public_shapes() {
    for (error, expected_status, expected_type, expected_code) in [
        (
            AiProxyError::ModelNotFound,
            404,
            "invalid_request_error",
            "model_not_found",
        ),
        (
            AiProxyError::StreamingDisabled,
            400,
            "invalid_request_error",
            "streaming_not_supported",
        ),
        (
            AiProxyError::BackendUnavailable,
            503,
            "api_error",
            "service_unavailable",
        ),
        (
            AiProxyError::MalformedBody,
            400,
            "invalid_request_error",
            "invalid_request_body",
        ),
        (
            AiProxyError::UnsupportedParameter,
            400,
            "invalid_request_error",
            "unsupported_parameter",
        ),
        (
            AiProxyError::MissingRequiredParameter,
            400,
            "invalid_request_error",
            "missing_required_parameter",
        ),
        (
            AiProxyError::UnsupportedEndpoint,
            404,
            "invalid_request_error",
            "unsupported_endpoint",
        ),
        (
            AiProxyError::RequestTooLarge,
            413,
            "invalid_request_error",
            "request_too_large",
        ),
        (
            AiProxyError::QuotaExceeded,
            429,
            "rate_limit_error",
            "token_quota_exceeded",
        ),
        (
            AiProxyError::QuotaUnavailable,
            503,
            "api_error",
            "token_quota_unavailable",
        ),
    ] {
        let (status, body) = render(error);
        assert_eq!(status, expected_status);
        let detail = body["error"].as_object().expect("error detail object");
        assert_eq!(detail.len(), 3);
        assert!(detail["message"].is_string());
        assert_eq!(detail["type"], expected_type);
        assert_eq!(detail["code"], expected_code);
    }
}
