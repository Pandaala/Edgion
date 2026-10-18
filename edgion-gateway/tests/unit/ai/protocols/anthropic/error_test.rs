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

#[test]
fn fixed_errors_preserve_wire_bytes_without_building_value_trees() {
    for (error, kind) in [
        (AiProxyError::ModelNotFound, "not_found_error"),
        (AiProxyError::UpstreamResponseInvalid, "api_error"),
        (AiProxyError::UpstreamTimeout, "api_error"),
        (AiProxyError::StreamingDisabled, "invalid_request_error"),
        (AiProxyError::BackendUnavailable, "overloaded_error"),
        (AiProxyError::MalformedBody, "invalid_request_error"),
        (AiProxyError::UnsupportedParameter, "invalid_request_error"),
        (AiProxyError::MissingRequiredParameter, "invalid_request_error"),
        (AiProxyError::UnsupportedEndpoint, "not_found_error"),
        (AiProxyError::RequestTooLarge, "request_too_large"),
        (AiProxyError::QuotaExceeded, "rate_limit_error"),
        (AiProxyError::QuotaUnavailable, "api_error"),
        (AiProxyError::ContentFiltered, "permission_error"),
    ] {
        let (expected_status, message, _, _) = error.fields();
        let (expected, tree_allocations) = crate::test_allocation_counter::count_allocations(|| {
            serde_json::json!({"type":"error","error":{"type":kind,"message":message}}).to_string()
        });
        let ((status, body), allocations) = crate::test_allocation_counter::count_allocations(|| render(error));
        assert_eq!(status, expected_status);
        assert_eq!(body, expected);
        assert!(
            allocations < tree_allocations,
            "{error:?}: {allocations} versus {tree_allocations}"
        );
        assert_eq!(
            event(error).as_ref(),
            format!("\n\nevent: error\ndata: {expected}\n\n").as_bytes()
        );
    }
}
