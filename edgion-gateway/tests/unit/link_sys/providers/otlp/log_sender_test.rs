use super::*;
use edgion_resources::sensitive::{SensitiveMode, SENSITIVE_MODE};
use std::sync::atomic::Ordering;

#[test]
fn request_trace_id_is_typed_without_claiming_a_gateway_span() {
    let provider = SdkLoggerProvider::builder().build();
    let logger = provider.logger("request-id-test");
    let mut record = logger.create_log_record();
    attach_request_trace_id(&mut record, None);
    assert!(record.trace_context().is_none());
    attach_request_trace_id(&mut record, Some([7; 16]));
    let context = record.trace_context().unwrap();
    assert_eq!(context.trace_id.to_bytes(), [7; 16]);
    assert_eq!(context.span_id, opentelemetry::SpanId::INVALID);
    assert_eq!(context.trace_flags, None);
}

fn assert_sensitive_path_in_subprocess(test_name: &str, mode: SensitiveMode, expected: &str) {
    const CHILD_ENV: &str = "EDGION_OTLP_SENSITIVE_PATH_CHILD";
    const TEST_MODULE: &str = "link_sys::providers::otlp::log_sender::tests";
    crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, || {
        SENSITIVE_MODE.store(mode as u8, Ordering::SeqCst);
        assert_eq!(render_sensitive_path("/customers/alice"), expected);
    });
}

#[test]
fn test_severity_mapping() {
    assert_eq!(numeric_to_severity(0), None);
    assert_eq!(numeric_to_severity(9), Some(Severity::Info));
    assert_eq!(numeric_to_severity(13), Some(Severity::Warn));
    assert_eq!(numeric_to_severity(17), Some(Severity::Error));
    assert_eq!(numeric_to_severity(24), Some(Severity::Fatal));
    assert_eq!(numeric_to_severity(25), None);
}

#[test]
fn url_path_is_clear_when_sensitive_mode_is_off() {
    assert_sensitive_path_in_subprocess(
        "url_path_is_clear_when_sensitive_mode_is_off",
        SensitiveMode::Off,
        "/customers/alice",
    );
}

#[test]
fn url_path_is_replaced_when_sensitive_mode_is_replace() {
    assert_sensitive_path_in_subprocess(
        "url_path_is_replaced_when_sensitive_mode_is_replace",
        SensitiveMode::Replace,
        "***",
    );
}

#[test]
fn url_path_is_wrapped_when_sensitive_mode_is_wrap() {
    assert_sensitive_path_in_subprocess(
        "url_path_is_wrapped_when_sensitive_mode_is_wrap",
        SensitiveMode::Wrap,
        "<E>/customers/alice</E>",
    );
}

#[test]
fn programmatic_output_reference_is_checked_before_sender_construction() {
    for (namespace, name) in [
        ("", "sink"),
        ("ns", ""),
        ("bad/ns", "sink"),
        ("ns", "bad/name"),
        ("NS", "sink"),
    ] {
        let reference = edgion_resources::output::LinkSysRef {
            namespace: namespace.into(),
            name: name.into(),
        };
        let error = OtlpLogSender::new(reference).err().expect("invalid identity must fail");
        assert_eq!(error.to_string(), "invalid LinkSys reference");
    }
    let reference = edgion_resources::output::LinkSysRef {
        namespace: "r6f-output".into(),
        name: "missing.sink".into(),
    };
    let sender = OtlpLogSender::new(reference).unwrap();
    assert_eq!(sender.name(), "r6f-output/missing.sink");
    assert!(!sender.healthy());
}
