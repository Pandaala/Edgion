use super::*;

#[test]
fn mixed_entries_preserve_order_and_repeats() {
    let mut diagnostics = RequestDiagnostics::default();
    diagnostics.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);
    diagnostics.record_info(EdgionInfoCode::PluginResponseReplaced);
    diagnostics.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);

    assert_eq!(
        diagnostics.entries(),
        &[
            RequestDiagnostic::Error(EdgionErrorCode::BackendRequestTimeoutRetry),
            RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced),
            RequestDiagnostic::Error(EdgionErrorCode::BackendRequestTimeoutRetry),
        ]
    );
    assert_eq!(
        serde_json::to_value(diagnostics.entries()).unwrap(),
        serde_json::json!([
            {"kind": "error", "code": "BackendRequestTimeoutRetry"},
            {"kind": "info", "code": "PluginResponseReplaced"},
            {"kind": "error", "code": "BackendRequestTimeoutRetry"}
        ])
    );
}

#[test]
fn storage_spills_once_and_drops_only_after_the_hard_limit() {
    let mut diagnostics = RequestDiagnostics::default();
    for _ in 0..INLINE_REQUEST_DIAGNOSTICS {
        assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
    }
    assert!(!diagnostics.entries.spilled());

    assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
    assert!(diagnostics.entries.spilled());
    let spilled_capacity = diagnostics.entries.capacity();
    assert!(spilled_capacity >= MAX_REQUEST_DIAGNOSTICS);

    while diagnostics.entries().len() < MAX_REQUEST_DIAGNOSTICS {
        assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
        assert_eq!(diagnostics.entries.capacity(), spilled_capacity);
    }
    assert!(!diagnostics.is_full());
    assert!(!diagnostics.record_error(EdgionErrorCode::ClientClosed));
    assert!(diagnostics.is_full());
    assert_eq!(diagnostics.entries().len(), MAX_REQUEST_DIAGNOSTICS);
    assert!(!diagnostics.contains_error(EdgionErrorCode::ClientClosed));
}
