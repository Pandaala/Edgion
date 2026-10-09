use super::*;
#[test]
fn native_json_and_named_sse_share_fixed_sdk_error_shape() {
    for error in [
        AiProxyError::QuotaExceeded,
        AiProxyError::UnsupportedParameter,
        AiProxyError::BackendUnavailable,
    ] {
        let (status, body) = render(error);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(status, error.fields().0);
        assert_eq!(value["type"], "error");
        assert!(value["error"]["message"].is_string());
        let wire = event(error);
        assert!(wire.starts_with(b"\n\nevent: error\ndata: "));
        assert!(wire.ends_with(b"\n\n"));
        assert!(!String::from_utf8(wire.to_vec()).unwrap().contains("[DONE]"));
    }
}

#[test]
fn native_local_503_distinguishes_backend_and_quota_dependencies() {
    for (error, expected) in [
        (AiProxyError::BackendUnavailable, "overloaded_error"),
        (AiProxyError::QuotaUnavailable, "api_error"),
    ] {
        let (status, body) = render(error);
        assert_eq!(status, 503);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["error"]["type"], expected);
    }
}
