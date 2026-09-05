//! OpenAI-compatible local error replies for AI request handling.

use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use serde::Serialize;

use crate::plugins::runtime::PluginSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProxyError {
    ModelNotFound,
    StreamingDisabled,
    BackendUnavailable,
    MalformedBody,
    UnsupportedParameter,
    MissingRequiredParameter,
    UnsupportedEndpoint,
    RequestTooLarge,
    QuotaExceeded,
    QuotaUnavailable,
    /// The request was refused by a request-side content policy.
    ///
    /// Uses the same `content_filter_error` / `content_policy_violation`
    /// vocabulary as the response-side refusal in `ai::terminal`, so a client
    /// sees one content-policy contract whichever side blocked.
    ContentFiltered,
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
                "Streaming is not enabled for this AI backend.",
                "invalid_request_error",
                "streaming_not_supported",
            ),
            Self::BackendUnavailable => (
                503,
                "The AI backend is temporarily unavailable.",
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
                "The request contains a parameter that is not supported by this AI backend.",
                "invalid_request_error",
                "unsupported_parameter",
            ),
            Self::MissingRequiredParameter => (
                400,
                "The request is missing a parameter required by this AI backend.",
                "invalid_request_error",
                "missing_required_parameter",
            ),
            Self::UnsupportedEndpoint => (
                404,
                "This AI backend supports only the Chat Completions endpoint.",
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
            Self::ContentFiltered => (
                403,
                "The request was blocked by a content policy.",
                "content_filter_error",
                "content_policy_violation",
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
        body: Some(Bytes::from(body)),
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

/// Upper bound on the client-supplied model name echoed into a refusal.
const MAX_ECHOED_MODEL_CHARS: usize = 128;

/// Fixed OpenAI-compatible content-filter refusal delivered at the request
/// stage, as a complete bounded body rather than a stream.
///
/// The client asked for a chat completion and gets a syntactically valid one
/// whose only choice carries `finish_reason: "content_filter"`, so an agent
/// chain degrades instead of erroring. Nothing is proxied upstream and no
/// response-side processor, terminal coordinator, or terminal renderer is
/// involved.
///
/// `model` is echoed from the request so clients that key on the field keep
/// working. It is client input read before `AiProxy` has validated it, so it is
/// truncated here and JSON-escaped rather than trusted. No rule name, matched
/// text, or evaluator detail is ever included.
pub fn content_filter_refusal(model: &str, streaming: bool) -> (&'static str, String) {
    let created = chrono::Utc::now().timestamp();
    let truncated = match model.char_indices().nth(MAX_ECHOED_MODEL_CHARS) {
        Some((offset, _)) => &model[..offset],
        None => model,
    };
    let model = serde_json::to_string(truncated).unwrap_or_else(|_| "\"\"".to_string());
    if streaming {
        let chunk = format!(
            "{{\"id\":\"chatcmpl-guard\",\"object\":\"chat.completion.chunk\",\"created\":{created},\"model\":{model},\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"\"}},\"finish_reason\":\"content_filter\"}}]}}"
        );
        ("text/event-stream", format!("data: {chunk}\n\ndata: [DONE]\n\n"))
    } else {
        (
            "application/json",
            format!(
                "{{\"id\":\"chatcmpl-guard\",\"object\":\"chat.completion\",\"created\":{created},\"model\":{model},\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"finish_reason\":\"content_filter\"}}]}}"
            ),
        )
    }
}

/// Emit the request-stage content-filter refusal as a local reply.
pub fn content_filter_response(session: &mut dyn PluginSession, model: &str, streaming: bool) -> PluginRunningResult {
    let (content_type, body) = content_filter_refusal(model, streaming);
    let _ = session.set_response_header("content-type", content_type);
    PluginRunningResult::ErrResponse {
        status: 200,
        body: Some(Bytes::from(body)),
    }
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
}
