use super::*;
use edgion_common::ext_proc::proto::{processing_request, HttpHeaders, HttpTrailers, ProcessingMode};
use edgion_resources::plugin_configs::ExtProcConfig;

fn wire_mode(
    response_header: processing_mode::HeaderSendMode,
    request_body: processing_mode::BodySendMode,
    response_body: processing_mode::BodySendMode,
    response_trailer: processing_mode::HeaderSendMode,
) -> ProcessingMode {
    ProcessingMode {
        request_header_mode: processing_mode::HeaderSendMode::Default as i32,
        response_header_mode: response_header as i32,
        request_body_mode: request_body as i32,
        response_body_mode: response_body as i32,
        request_trailer_mode: processing_mode::HeaderSendMode::Default as i32,
        response_trailer_mode: response_trailer as i32,
    }
}

fn override_owner(base: ConfigProcessingMode, enabled: bool, allowed: Vec<ConfigProcessingMode>) -> AttachedExtProc {
    let mut config = ExtProcConfig::default();
    config.grpc_service.target = "override.example:9000".to_string();
    let allowed = allowed.iter().map(ConfigProcessingMode::normalized_override).collect();
    AttachedExtProc::new(
        super::super::client::tests::actual_client(&config).unwrap(),
        base,
        enabled,
        allowed,
    )
}

#[tokio::test]
async fn shared_override_modes_keep_request_state_independent() {
    let mut config = ExtProcConfig::default();
    config.grpc_service.target = "shared-override.example:9000".to_string();
    let client = super::super::client::tests::actual_client(&config).unwrap();
    let base = ConfigProcessingMode {
        response_body_mode: BodySendMode::Streamed,
        ..Default::default()
    };
    let reduction = ConfigProcessingMode::default().normalized_override();
    let allowed: Arc<[ConfigProcessingMode]> = Arc::from([reduction.clone()]);
    let first = AttachedExtProc::new(client.clone(), base.clone(), true, Arc::clone(&allowed));
    let second = AttachedExtProc::new(client, base.clone(), true, Arc::clone(&allowed));
    assert!(Arc::ptr_eq(&allowed, &first.allowed_override_modes));
    assert!(Arc::ptr_eq(
        &first.allowed_override_modes,
        &second.allowed_override_modes
    ));
    drop(allowed);

    let wire = wire_mode(
        processing_mode::HeaderSendMode::Default,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Default,
    );
    assert_eq!(
        first
            .apply_mode_override(OverridePhase::RequestHeaders, Some(&wire))
            .unwrap(),
        reduction
    );
    assert_eq!(second.effective_mode().unwrap(), base.normalized_override());
    assert!(matches!(*second.state.lock(), StreamState::Lazy));
    drop(first);
    assert_eq!(
        second
            .apply_mode_override(OverridePhase::RequestHeaders, Some(&wire))
            .unwrap(),
        reduction
    );
}

#[tokio::test]
async fn response_only_owner_stays_lazy_until_a_phase_exchange() {
    let mut config = ExtProcConfig::default();
    config.grpc_service.target = "lazy.example:9000".to_string();
    let owner = AttachedExtProc::new(
        super::super::client::tests::actual_client(&config).unwrap(),
        config.processing_mode,
        false,
        Arc::from([]),
    );
    assert!(matches!(*owner.state.lock(), StreamState::Lazy));
    owner.terminalize_for_logging();
    let line = owner.take_summary_line().unwrap();
    assert!(line.contains("phases=0"));
    assert!(line.contains("reason=logging-fallback"));
    assert!(
        owner.take_summary_line().is_none(),
        "one owner emits one aggregate summary"
    );
}

#[tokio::test]
async fn duplicate_and_regressed_phases_are_rejected_before_network_work() {
    let mut config = ExtProcConfig::default();
    config.grpc_service.target = "phase.example:9000".to_string();
    let owner = AttachedExtProc::new(
        super::super::client::tests::actual_client(&config).unwrap(),
        config.processing_mode,
        false,
        Arc::from([]),
    );
    let headers = ProcessingRequest {
        request: Some(processing_request::Request::ResponseHeaders(HttpHeaders::default())),
    };
    owner.validate_phase(&headers).unwrap();
    assert_eq!(
        owner.validate_phase(&headers).unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let request_headers = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };
    assert_eq!(
        owner.validate_phase(&request_headers).unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );

    let owner = override_owner(ConfigProcessingMode::default(), false, Vec::new());
    let body = ProcessingRequest {
        request: Some(processing_request::Request::ResponseBody(
            edgion_common::ext_proc::proto::HttpBody {
                body: Vec::new(),
                end_of_stream: false,
            },
        )),
    };
    let trailers = ProcessingRequest {
        request: Some(processing_request::Request::ResponseTrailers(HttpTrailers::default())),
    };
    owner.validate_phase(&body).unwrap();
    owner.validate_phase(&trailers).unwrap();
    assert_eq!(
        owner.validate_phase(&trailers).unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(
        owner.validate_phase(&body).unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
}

#[tokio::test]
async fn disabled_override_ignores_even_malformed_wire_values() {
    let owner = override_owner(ConfigProcessingMode::default(), false, Vec::new());
    let malformed = ProcessingMode {
        request_header_mode: 99,
        ..Default::default()
    };
    let before = owner.effective_mode().unwrap();
    assert_eq!(
        owner
            .apply_mode_override(OverridePhase::RequestHeaders, Some(&malformed))
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn malformed_enabled_override_fails_atomically() {
    let base = ConfigProcessingMode {
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::Buffered,
        response_body_mode: BodySendMode::Streamed,
        ..Default::default()
    };
    let owner = override_owner(base, true, vec![ConfigProcessingMode::default()]);
    let malformed = ProcessingMode {
        response_header_mode: 99,
        ..Default::default()
    };
    let before = owner.effective_mode().unwrap();
    assert!(owner
        .apply_mode_override(OverridePhase::RequestHeaders, Some(&malformed))
        .is_err());
    assert_eq!(owner.effective_mode().unwrap(), before);
}

#[tokio::test]
async fn exact_membership_and_successive_default_inheritance_are_enforced() {
    let base = ConfigProcessingMode {
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::Buffered,
        response_body_mode: BodySendMode::Streamed,
        ..Default::default()
    };
    let request_reduction = ConfigProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::Streamed,
        response_trailer_mode: HeaderSendMode::Skip,
    };
    let response_reduction = ConfigProcessingMode {
        response_body_mode: BodySendMode::None,
        ..request_reduction.clone()
    };
    let owner = override_owner(base, true, vec![request_reduction.clone(), response_reduction.clone()]);
    let first = wire_mode(
        processing_mode::HeaderSendMode::Send,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::Streamed,
        processing_mode::HeaderSendMode::Default,
    );
    assert_eq!(
        owner
            .apply_mode_override(OverridePhase::RequestHeaders, Some(&first))
            .unwrap(),
        request_reduction
    );

    let second = wire_mode(
        processing_mode::HeaderSendMode::Default,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Default,
    );
    assert_eq!(
        owner
            .apply_mode_override(OverridePhase::ResponseHeaders, Some(&second))
            .unwrap(),
        response_reduction
    );

    let unlisted = wire_mode(
        processing_mode::HeaderSendMode::Skip,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Default,
    );
    assert_eq!(
        owner
            .apply_mode_override(OverridePhase::ResponseHeaders, Some(&unlisted))
            .unwrap(),
        response_reduction
    );
}

#[tokio::test]
async fn late_request_fields_and_valid_unsupported_body_modes_are_ignored() {
    let base = ConfigProcessingMode {
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::Buffered,
        response_body_mode: BodySendMode::Streamed,
        ..Default::default()
    };
    let allowed = ConfigProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::None,
        response_trailer_mode: HeaderSendMode::Skip,
    };
    let owner = override_owner(base, true, vec![allowed]);
    let late = wire_mode(
        processing_mode::HeaderSendMode::Send,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Skip,
    );
    let effective = owner
        .apply_mode_override(OverridePhase::ResponseHeaders, Some(&late))
        .unwrap();
    assert_eq!(effective.request_body_mode, BodySendMode::Buffered);
    assert_eq!(effective.response_body_mode, BodySendMode::None);

    for unsupported in [
        processing_mode::BodySendMode::BufferedPartial,
        processing_mode::BodySendMode::FullDuplexStreamed,
        processing_mode::BodySendMode::Grpc,
    ] {
        let mode = wire_mode(
            processing_mode::HeaderSendMode::Send,
            unsupported,
            processing_mode::BodySendMode::None,
            processing_mode::HeaderSendMode::Skip,
        );
        assert_eq!(
            owner
                .apply_mode_override(OverridePhase::RequestHeaders, Some(&mode))
                .unwrap(),
            effective
        );
    }
}

#[tokio::test]
async fn response_side_unsupported_modes_and_trailer_send_are_ignored() {
    let base = ConfigProcessingMode {
        response_header_mode: HeaderSendMode::Send,
        response_body_mode: BodySendMode::Streamed,
        ..Default::default()
    };
    let reduction = ConfigProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::None,
        response_trailer_mode: HeaderSendMode::Skip,
    };
    let owner = override_owner(base, true, vec![reduction]);
    let before = owner.effective_mode().unwrap();

    for unsupported in [
        processing_mode::BodySendMode::Buffered,
        processing_mode::BodySendMode::BufferedPartial,
        processing_mode::BodySendMode::FullDuplexStreamed,
        processing_mode::BodySendMode::Grpc,
    ] {
        let mode = wire_mode(
            processing_mode::HeaderSendMode::Send,
            processing_mode::BodySendMode::None,
            unsupported,
            processing_mode::HeaderSendMode::Skip,
        );
        assert_eq!(
            owner
                .apply_mode_override(OverridePhase::ResponseHeaders, Some(&mode))
                .unwrap(),
            before
        );
    }

    let trailer_send = wire_mode(
        processing_mode::HeaderSendMode::Send,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Send,
    );
    assert_eq!(
        owner
            .apply_mode_override(OverridePhase::ResponseHeaders, Some(&trailer_send))
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn none_cannot_expand_and_buffered_or_streamed_can_reduce_to_none() {
    for base_body in [BodySendMode::None, BodySendMode::Buffered, BodySendMode::Streamed] {
        let base = ConfigProcessingMode {
            request_body_mode: base_body,
            ..Default::default()
        };
        let reduced = ConfigProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            request_body_mode: BodySendMode::None,
            ..Default::default()
        };
        let owner = override_owner(base, true, vec![reduced.clone()]);
        let requested_body = if base_body == BodySendMode::None {
            processing_mode::BodySendMode::Buffered
        } else {
            processing_mode::BodySendMode::None
        };
        let wire = wire_mode(
            processing_mode::HeaderSendMode::Default,
            requested_body,
            processing_mode::BodySendMode::None,
            processing_mode::HeaderSendMode::Default,
        );
        let effective = owner
            .apply_mode_override(OverridePhase::RequestHeaders, Some(&wire))
            .unwrap();
        if base_body == BodySendMode::None {
            assert_eq!(effective.request_body_mode, BodySendMode::None);
        } else {
            assert_eq!(effective, reduced);
        }
    }
}

#[tokio::test]
async fn unary_compatibility_uses_effective_body_capability_and_forbids_expansion() {
    let base = ConfigProcessingMode::default();
    let expanded = ConfigProcessingMode {
        response_body_mode: BodySendMode::Streamed,
        ..base.clone()
    };
    let owner = override_owner(base, true, vec![expanded.clone()]);
    assert!(!owner.consumes_response_body());
    let wire = wire_mode(
        processing_mode::HeaderSendMode::Default,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::Streamed,
        processing_mode::HeaderSendMode::Default,
    );
    owner
        .apply_mode_override(OverridePhase::ResponseHeaders, Some(&wire))
        .unwrap();
    assert!(!owner.consumes_response_body());
    let reduced = ConfigProcessingMode::default();
    let owner = override_owner(expanded, true, vec![reduced]);
    assert!(owner.consumes_response_body());
    let wire = wire_mode(
        processing_mode::HeaderSendMode::Default,
        processing_mode::BodySendMode::None,
        processing_mode::BodySendMode::None,
        processing_mode::HeaderSendMode::Default,
    );
    owner
        .apply_mode_override(OverridePhase::ResponseHeaders, Some(&wire))
        .unwrap();
    assert!(!owner.consumes_response_body());
}

pub(crate) fn test_body_owner(response_body: bool) -> AttachedExtProc {
    let base = ConfigProcessingMode {
        response_body_mode: if response_body {
            BodySendMode::Streamed
        } else {
            BodySendMode::None
        },
        ..Default::default()
    };
    override_owner(base, false, vec![])
}

impl AttachedExtProc {
    pub(crate) fn install_test_stream(&self, stream: ExtProcStream) {
        let mut state = self.state.lock();
        assert!(matches!(*state, StreamState::Lazy));
        *state = StreamState::Open(stream);
    }

    pub(crate) fn test_lazy() -> Self {
        use edgion_resources::plugin_configs::ExtProcConfig;

        let mut config = ExtProcConfig::default();
        config.grpc_service.target = "logging.example:9000".to_string();
        Self::new(
            super::super::client::tests::actual_client(&config).expect("test target must build lazily"),
            config.processing_mode,
            false,
            Arc::from([]),
        )
    }
}

#[tokio::test]
async fn logging_terminalization_preserves_settled_summaries_and_emission() {
    for state in ["lazy", "finished", "failed"] {
        for outcome in ["pending", "success", "dependency-failure"] {
            for emitted in [false, true] {
                let owner = override_owner(ConfigProcessingMode::default(), false, Vec::new());
                *owner.state.lock() = match state {
                    "lazy" => StreamState::Lazy,
                    "finished" => StreamState::Finished,
                    _ => StreamState::Failed,
                };
                *owner.summary.lock() = Summary {
                    phases: 3,
                    input_bytes: 17,
                    output_bytes: 29,
                    outcome,
                    reason: "prior-reason",
                    emitted,
                };
                for _ in 0..2 {
                    owner.terminalize_for_logging();
                    assert_eq!(matches!(*owner.state.lock(), StreamState::Failed), state == "failed");
                    assert_eq!(matches!(*owner.state.lock(), StreamState::Finished), state != "failed");
                    let summary = owner.summary.lock();
                    assert_eq!(summary.phases, 3);
                    assert_eq!(summary.input_bytes, 17);
                    assert_eq!(summary.output_bytes, 29);
                    assert_eq!(summary.emitted, emitted);
                    assert_eq!(
                        summary.outcome,
                        if outcome == "pending" { "cancelled" } else { outcome }
                    );
                    assert_eq!(
                        summary.reason,
                        if outcome == "pending" {
                            "logging-fallback"
                        } else {
                            "prior-reason"
                        }
                    );
                }
                let expected = (!emitted).then(|| {
                    let (outcome, reason) = if outcome == "pending" {
                        ("cancelled", "logging-fallback")
                    } else {
                        (outcome, "prior-reason")
                    };
                    format!(
                        "ext_proc attached phases=3 input_bytes=17 output_bytes=29 outcome={outcome} reason={reason}"
                    )
                });
                assert_eq!(owner.take_summary_line(), expected);
                assert!(owner.take_summary_line().is_none());
            }
        }
    }
}
