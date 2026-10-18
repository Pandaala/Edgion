//! Fixed native Messages error envelopes without provider or credential detail.
use crate::ai::AiProxyError;
use bytes::Bytes;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
    #[serde(rename = "type")]
    type_: &'static str,
}

#[derive(Serialize)]
struct ErrorDetail {
    message: &'static str,
    #[serde(rename = "type")]
    type_: &'static str,
}

pub(crate) fn render(error: AiProxyError) -> (u16, String) {
    let (status, message, _, _) = error.fields();
    let kind = match error {
        AiProxyError::ModelNotFound | AiProxyError::UnsupportedEndpoint => "not_found_error",
        AiProxyError::RequestTooLarge => "request_too_large",
        AiProxyError::QuotaExceeded => "rate_limit_error",
        AiProxyError::BackendUnavailable => "overloaded_error",
        AiProxyError::ContentFiltered => "permission_error",
        AiProxyError::MalformedBody
        | AiProxyError::UnsupportedParameter
        | AiProxyError::MissingRequiredParameter
        | AiProxyError::StreamingDisabled => "invalid_request_error",
        _ => "api_error",
    };
    let body = serde_json::to_string(&ErrorEnvelope {
        error: ErrorDetail { message, type_: kind },
        type_: "error",
    })
    .unwrap_or_else(|_| serde_json::json!({"type":"error","error":{"type":kind,"message":message}}).to_string());
    (status, body)
}
pub(crate) fn event(error: AiProxyError) -> Bytes {
    let (_, body) = render(error);
    Bytes::from(format!("\n\nevent: error\ndata: {body}\n\n"))
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/anthropic/error_test.rs"]
mod tests;
