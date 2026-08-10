//! OpenAI-compatible local error replies for AI request handling.

use edgion_resources::filters::PluginRunningResult;
use serde::Serialize;

use crate::plugins::runtime::PluginSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProxyError {
    ModelNotFound,
    StreamingDisabled,
    ServiceUnavailable,
    MalformedBody,
    UnsupportedParameter,
    MissingRequiredParameter,
    UnsupportedEndpoint,
    RequestTooLarge,
    QuotaExceeded,
    QuotaUnavailable,
}

impl AiProxyError {
    const fn fields(self) -> (u16, &'static str, &'static str, &'static str) {
        match self {
            Self::ModelNotFound => (
                404,
                "The requested model was not found.",
                "invalid_request_error",
                "model_not_found",
            ),
            Self::StreamingDisabled => (
                400,
                "Streaming is not enabled for this AI service.",
                "invalid_request_error",
                "streaming_not_supported",
            ),
            Self::ServiceUnavailable => (
                503,
                "The AI service is temporarily unavailable.",
                "api_error",
                "service_unavailable",
            ),
            Self::MalformedBody => (
                400,
                "The request body must be a valid JSON object.",
                "invalid_request_error",
                "invalid_request_body",
            ),
            Self::UnsupportedParameter => (
                400,
                "The request contains a parameter that is not supported by this AI service.",
                "invalid_request_error",
                "unsupported_parameter",
            ),
            Self::MissingRequiredParameter => (
                400,
                "The request is missing a parameter required by this AI service.",
                "invalid_request_error",
                "missing_required_parameter",
            ),
            Self::UnsupportedEndpoint => (
                404,
                "This AI service supports only the Chat Completions endpoint.",
                "invalid_request_error",
                "unsupported_endpoint",
            ),
            Self::RequestTooLarge => (
                413,
                "The AI request is too large.",
                "invalid_request_error",
                "request_too_large",
            ),
            Self::QuotaExceeded => (
                429,
                "The token quota has been exceeded.",
                "rate_limit_error",
                "token_quota_exceeded",
            ),
            Self::QuotaUnavailable => (
                503,
                "Token quota enforcement is temporarily unavailable.",
                "api_error",
                "token_quota_unavailable",
            ),
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    message: &'static str,
    #[serde(rename = "type")]
    type_: &'static str,
    code: &'static str,
}

/// Queue the JSON content type and return an OpenAI-compatible local reply.
pub fn openai_error_response(session: &mut dyn PluginSession, error: AiProxyError) -> PluginRunningResult {
    let (status, body) = render_openai_error(error);
    let _ = session.set_response_header("content-type", "application/json");
    PluginRunningResult::ErrResponse {
        status,
        body: Some(body),
    }
}

/// Render one fixed OpenAI-compatible error without dependency or topology
/// detail. Both plugin-local replies and later peer-selection failures use
/// this canonical renderer.
pub(crate) fn render_openai_error(error: AiProxyError) -> (u16, String) {
    let (status, message, type_, code) = error.fields();
    let body = serde_json::to_string(&ErrorEnvelope {
        error: ErrorDetail { message, type_, code },
    })
    .unwrap_or_else(|_| {
        "{\"error\":{\"message\":\"The request could not be processed.\",\"type\":\"api_error\",\"code\":\"internal_error\"}}"
            .to_string()
    });
    (status, body)
}

#[cfg(test)]
mod tests {
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
            } => (status, serde_json::from_str(&body).unwrap()),
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
                AiProxyError::ServiceUnavailable,
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
}
