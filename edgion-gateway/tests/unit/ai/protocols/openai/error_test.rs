use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

#[test]
fn content_filter_refusal_preserves_framing_escaping_and_model_bound() {
    for model in [
        String::new(),
        "plain-model".to_owned(),
        "quote\"\\\n\r\t".to_owned(),
        "café-π".to_owned(),
        "é".repeat(128),
        "π".repeat(129),
    ] {
        let expected_model: String = model.chars().take(128).collect();
        for streaming in [false, true] {
            let (content_type, body) = content_filter_refusal(&model, streaming);
            let json = if streaming {
                assert_eq!(content_type, "text/event-stream");
                let events: Vec<_> = body.split("\n\n").collect();
                assert_eq!(events.len(), 3);
                assert_eq!(events[1], "data: [DONE]");
                assert_eq!(events[2], "");
                events[0].strip_prefix("data: ").unwrap()
            } else {
                assert_eq!(content_type, "application/json");
                body.as_str()
            };
            let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
            let created = parsed["created"].as_i64().expect("integer timestamp");
            assert_eq!(parsed["model"], expected_model);
            let encoded_model = serde_json::to_string(&expected_model).unwrap();
            let (object, payload) = if streaming {
                ("chat.completion.chunk", "delta")
            } else {
                ("chat.completion", "message")
            };
            let expected_json = format!(
                "{{\"id\":\"chatcmpl-guard\",\"object\":\"{object}\",\"created\":{created},\"model\":{encoded_model},\"choices\":[{{\"index\":0,\"{payload}\":{{\"role\":\"assistant\",\"content\":\"\"}},\"finish_reason\":\"content_filter\"}}]}}"
            );
            let expected = if streaming {
                format!("data: {expected_json}\n\ndata: [DONE]\n\n")
            } else {
                expected_json
            };
            assert_eq!(body, expected);
        }
    }
}

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
