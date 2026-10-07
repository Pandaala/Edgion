use super::*;
use edgion_common::ext_proc::proto::{BodyMutation, BodyResponse, CommonResponse, HttpStatus, ImmediateResponse};

/// Scripted exchange: pops pre-programmed results in order. Panics when
/// the handler exchanges more often than scripted — which is exactly what
/// the deactivation tests pin.
struct ScriptedExchange {
    script: Vec<Result<ProcessingResponse, tonic::Status>>,
}

impl ScriptedExchange {
    fn new(script: Vec<Result<ProcessingResponse, tonic::Status>>) -> Self {
        Self { script }
    }
}

#[async_trait]
impl BodyExchange for ScriptedExchange {
    async fn exchange_body(&mut self, _body: HttpBody, _last: bool) -> Result<ProcessingResponse, tonic::Status> {
        assert!(!self.script.is_empty(), "handler exchanged more often than scripted");
        self.script.remove(0)
    }
}

fn body_verdict(mutation: Option<body_mutation::Mutation>) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::RequestBody(BodyResponse {
            response: Some(CommonResponse {
                body_mutation: mutation.map(|mutation| BodyMutation {
                    mutation: Some(mutation),
                }),
                ..Default::default()
            }),
        })),
        mode_override: None,
    }
}

fn handler_with(
    script: Vec<Result<ProcessingResponse, tonic::Status>>,
    failure_mode_allow: bool,
) -> ExtProcStreamedBodyHandler {
    ExtProcStreamedBodyHandler::new(
        Box::new(ScriptedExchange::new(script)),
        failure_mode_allow,
        false,
        None,
        true,
    )
}

#[tokio::test]
async fn body_mutation_replaces_current_chunk() {
    let mut handler = handler_with(
        vec![Ok(body_verdict(Some(body_mutation::Mutation::Body(
            b"mutated".to_vec(),
        ))))],
        false,
    );
    let mut chunk = Some(Bytes::from_static(b"original"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert_eq!(chunk.as_deref(), Some(&b"mutated"[..]));
}

#[tokio::test]
async fn request_body_phase_ignores_mode_override_even_when_enum_is_unknown() {
    let mut verdict = body_verdict(None);
    verdict.mode_override = Some(edgion_common::ext_proc::proto::ProcessingMode {
        request_body_mode: 99,
        ..Default::default()
    });
    let mut handler = handler_with(vec![Ok(verdict)], false);
    let mut chunk = Some(Bytes::from_static(b"original"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
}

#[tokio::test]
async fn clear_body_empties_current_chunk() {
    let mut handler = handler_with(
        vec![Ok(body_verdict(Some(body_mutation::Mutation::ClearBody(true))))],
        false,
    );
    let mut chunk = Some(Bytes::from_static(b"secret"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert_eq!(chunk.as_deref(), Some(&b""[..]));
}

#[tokio::test]
async fn malformed_immediate_response_fails_before_using_body() {
    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
            status: Some(HttpStatus { code: 100 }),
            body: b"denied".to_vec(),
            ..Default::default()
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(immediate)], false);
    let mut chunk = Some(Bytes::from_static(b"payload"));
    match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
        ChunkVerdict::Terminate {
            status,
            body,
            header_ops,
            ..
        } => {
            assert_eq!(status, 500, "1xx is not a valid final reply");
            assert_eq!(body, None);
            assert!(header_ops.is_empty());
        }
        ChunkVerdict::Continue => panic!("immediate response must terminate"),
    }
    assert!(handler.take_log().is_none());
}

#[tokio::test]
async fn disabled_immediate_response_deactivates_and_preserves_chunk() {
    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(
            ImmediateResponse::default(),
        )),
        mode_override: None,
    };
    let mut handler = ExtProcStreamedBodyHandler::new(
        Box::new(ScriptedExchange::new(vec![Ok(immediate)])),
        false,
        true,
        None,
        true,
    );
    let mut chunk = Some(Bytes::from_static(b"original"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
}

#[tokio::test]
async fn valid_immediate_carries_exact_grpc_status() {
    use edgion_common::ext_proc::proto::GrpcStatus;

    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
            status: Some(HttpStatus { code: 403 }),
            grpc_status: Some(GrpcStatus { status: 0 }),
            body: b"exact".to_vec(),
            ..Default::default()
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(immediate)], false);
    let mut chunk = Some(Bytes::from_static(b"payload"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Terminate {
            status: 403,
            grpc_status: Some(0),
            ..
        }
    ));
}

#[tokio::test]
async fn immediate_response_preserves_binary_body_and_deactivates() {
    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
            status: Some(HttpStatus { code: 403 }),
            grpc_status: Some(edgion_common::ext_proc::proto::GrpcStatus { status: 0 }),
            body: vec![0, 0xff, b'a', 0x80],
            ..Default::default()
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(immediate)], false);
    let mut chunk = Some(Bytes::from_static(b"payload"));
    match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
        ChunkVerdict::Terminate {
            status,
            grpc_status,
            body,
            ..
        } => {
            assert_eq!(status, 403);
            assert_eq!(grpc_status, Some(0));
            assert_eq!(body.unwrap().as_ref(), &[0, 0xff, b'a', 0x80]);
        }
        ChunkVerdict::Continue => panic!("immediate response must terminate"),
    }
    assert_eq!(chunk.as_deref(), Some(&b"payload"[..]));
    assert!(handler.take_pending_outcome().is_none());
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Complete).await,
        ChunkVerdict::Continue
    ));
}

/// Immediate-response header mutations are carried into the terminate
/// verdict, with protected/pseudo headers filtered out by the handler.
#[tokio::test]
async fn immediate_response_headers_are_carried_and_filtered() {
    use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue};

    let set = |key: &str, value: &str| edgion_common::ext_proc::proto::HeaderValueOption {
        header: Some(HeaderValue {
            key: key.to_string(),
            value: value.to_string(),
            raw_value: Vec::new(),
        }),
        append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
        keep_empty_value: false,
    };
    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
            status: Some(HttpStatus { code: 403 }),
            headers: Some(HeaderMutation {
                set_headers: vec![set("x-denied-by", "policy"), set("content-length", "999")],
                remove_headers: Vec::new(),
            }),
            body: b"no".to_vec(),
            ..Default::default()
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(immediate)], false);
    let mut chunk = Some(Bytes::from_static(b"payload"));
    match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
        ChunkVerdict::Terminate { status, header_ops, .. } => {
            assert_eq!(status, 403);
            assert!(matches!(
                header_ops.as_slice(),
                [crate::PendingHeaderOp::Set { name, value }]
                    if name == "x-denied-by" && value == "policy"
            ));
        }
        ChunkVerdict::Continue => panic!("immediate response must terminate"),
    }
    assert!(handler
        .take_log()
        .is_some_and(|log| log.contains("protected header mutation rejected")));
}

#[tokio::test]
async fn transport_error_fail_close_terminates_500() {
    let mut handler = handler_with(vec![Err(tonic::Status::unavailable("down"))], false);
    let mut chunk = Some(Bytes::from_static(b"x"));
    match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
        ChunkVerdict::Terminate { status, body, .. } => {
            assert_eq!(status, 500);
            assert!(body.is_none());
        }
        ChunkVerdict::Continue => panic!("fail-close must terminate"),
    }
}

/// Fail-open deactivates the handler: the scripted exchange holds exactly
/// one error, so a second exchange attempt would panic the script.
#[tokio::test]
async fn transport_error_fail_open_deactivates() {
    let mut handler = handler_with(vec![Err(tonic::Status::unavailable("down"))], true);
    let mut chunk = Some(Bytes::from_static(b"x"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert_eq!(chunk.as_deref(), Some(&b"x"[..]), "fail-open must not drop the chunk");
    let outcome = handler.take_pending_outcome().expect("fail-open records a dye outcome");
    assert!(matches!(outcome.0, DyeOutcome::FailOpen));
    let mut next = Some(Bytes::from_static(b"y"));
    assert!(matches!(
        handler.on_chunk(&mut next, RequestBodyEvent::Complete).await,
        ChunkVerdict::Continue
    ));
    assert!(handler.take_pending_outcome().is_none(), "outcome drains once");
    assert!(handler.take_log().is_none());
}

/// CONTINUE_AND_REPLACE mid-stream is unimplemented ("replace and stop")
/// and must be rejected per the failure policy, never silently downgraded
/// to CONTINUE.
#[tokio::test]
async fn mid_body_continue_and_replace_is_bad_verdict() {
    let cr = ProcessingResponse {
        response: Some(processing_response::Response::RequestBody(BodyResponse {
            response: Some(CommonResponse {
                status: ResponseStatus::ContinueAndReplace as i32,
                body_mutation: Some(BodyMutation {
                    mutation: Some(body_mutation::Mutation::Body(b"replacement".to_vec())),
                }),
                ..Default::default()
            }),
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(cr)], false);
    let mut chunk = Some(Bytes::from_static(b"x"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Terminate { status: 500, .. }
    ));
    assert_eq!(
        chunk.as_deref(),
        Some(&b"x"[..]),
        "the rejected CR mutation must not have been applied"
    );
}

#[tokio::test]
async fn streamed_body_rejects_unimplemented_common_fields_before_mutation() {
    let response = ProcessingResponse {
        response: Some(processing_response::Response::RequestBody(BodyResponse {
            response: Some(CommonResponse {
                clear_route_cache: true,
                body_mutation: Some(BodyMutation {
                    mutation: Some(body_mutation::Mutation::Body(b"changed".to_vec())),
                }),
                ..Default::default()
            }),
        })),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(response)], false);
    let mut chunk = Some(Bytes::from_static(b"original"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Terminate { status: 500, .. }
    ));
    assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
}

#[tokio::test]
async fn wrong_phase_verdict_follows_failure_policy() {
    use edgion_common::ext_proc::proto::HeadersResponse;
    let wrong = ProcessingResponse {
        response: Some(processing_response::Response::ResponseHeaders(
            HeadersResponse::default(),
        )),
        mode_override: None,
    };
    let mut handler = handler_with(vec![Ok(wrong)], false);
    let mut chunk = Some(Bytes::from_static(b"x"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Terminate { status: 500, .. }
    ));
}

#[tokio::test]
async fn missing_unknown_and_malformed_streamed_body_verdicts_fail_closed() {
    let verdicts = [
        ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: None,
            })),
            mode_override: None,
        },
        ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    status: 99,
                    ..Default::default()
                }),
            })),
            mode_override: None,
        },
        ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    body_mutation: Some(BodyMutation { mutation: None }),
                    ..Default::default()
                }),
            })),
            mode_override: None,
        },
    ];
    for verdict in verdicts {
        let mut handler = handler_with(vec![Ok(verdict)], false);
        let mut chunk = Some(Bytes::from_static(b"original"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
    }
}

/// A pure EOS event (None chunk) still reaches the processor as an empty
/// end_of_stream body message, half-closing the outbound direction. The
/// recording exchange asserts the on-wire shape from inside the script.
#[tokio::test]
async fn eos_event_sends_empty_terminal_body() {
    struct AssertingExchange;

    #[async_trait]
    impl BodyExchange for AssertingExchange {
        async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status> {
            assert!(body.body.is_empty());
            assert!(body.end_of_stream);
            assert!(last, "EOS must half-close the outbound direction");
            Ok(body_verdict(None))
        }
    }

    let mut handler = ExtProcStreamedBodyHandler::new(Box::new(AssertingExchange), false, false, None, true);
    let mut chunk = None;
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Complete).await,
        ChunkVerdict::Continue
    ));
    assert!(
        handler.take_log().is_none(),
        "attached owner owns the only aggregate log"
    );
}

#[tokio::test]
async fn abandoned_event_cancels_without_sending_clean_eos() {
    let mut handler = handler_with(Vec::new(), false);
    let mut chunk = None;

    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Abandoned).await,
        ChunkVerdict::Continue
    ));
    assert!(handler.exchange.is_none(), "the processor stream must be dropped");
    assert!(handler.take_log().is_none(), "attached owner owns abandonment logging");

    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Complete).await,
        ChunkVerdict::Continue
    ));
}

#[tokio::test]
async fn take_log_is_none_without_entries() {
    let mut handler = handler_with(vec![Ok(body_verdict(None))], false);
    let mut chunk = Some(Bytes::from_static(b"x"));
    assert!(matches!(
        handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
        ChunkVerdict::Continue
    ));
    assert!(handler.take_log().is_none(), "mid-stream success logs nothing");
}

#[tokio::test]
async fn completed_request_abandonment_closes_pending_response_owner_without_overwriting_terminal_results() {
    for prior in [None, Some("success"), Some("immediate-response"), Some("fail-close")] {
        let owner = Arc::new(AttachedExtProc::test_lazy());
        if let Some(outcome) = prior {
            owner.mark_terminal(outcome, "test-terminal");
        }
        let mut handler = ExtProcStreamedBodyHandler::new(Box::new(owner.clone()), false, false, None, false);
        // The request direction is done, but the shared response transaction
        // can still be pending. No network exchange is needed to test close.
        handler.done = true;
        assert!(matches!(
            handler.on_chunk(&mut None, RequestBodyEvent::Abandoned).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.exchange.is_none());
        let text = owner.take_summary_line().unwrap();
        assert!(text.contains(prior.unwrap_or("cancelled")), "{text}");
        assert!(!text.contains("logging-fallback"));
    }
}
