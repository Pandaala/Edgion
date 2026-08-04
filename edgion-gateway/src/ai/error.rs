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
    fn model_not_found_shape_is_stable() {
        let (status, body) = render(AiProxyError::ModelNotFound);
        assert_eq!(status, 404);
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "model_not_found");
        assert!(body["error"]["message"].is_string());
        assert_eq!(body["error"].as_object().unwrap().len(), 3);
    }

    #[test]
    fn fixed_error_mappings_do_not_expose_internal_detail() {
        for (error, expected_status) in [
            (AiProxyError::StreamingDisabled, 400),
            (AiProxyError::ServiceUnavailable, 503),
            (AiProxyError::MalformedBody, 400),
            (AiProxyError::UnsupportedParameter, 400),
            (AiProxyError::MissingRequiredParameter, 400),
            (AiProxyError::UnsupportedEndpoint, 404),
            (AiProxyError::RequestTooLarge, 413),
            (AiProxyError::QuotaExceeded, 429),
            (AiProxyError::QuotaUnavailable, 503),
        ] {
            let (status, body) = render(error);
            assert_eq!(status, expected_status);
            let rendered = body.to_string();
            assert!(!rendered.contains("Secret"));
            assert!(!rendered.contains("provider.example"));
            assert!(!rendered.contains("api-key"));
        }
    }
}
