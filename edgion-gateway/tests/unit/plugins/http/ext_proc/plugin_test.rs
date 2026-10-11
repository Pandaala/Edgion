#![allow(clippy::field_reassign_with_default)]

use super::*;
use edgion_resources::plugin_configs::ExtProcConfig;

impl ExtProc {
    fn build(config: &ExtProcConfig) -> Option<Self> {
        let owner = super::super::client::tests::actual_owner(config);
        let prepared = edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        Self::build_with_identity(
            &entry,
            &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
            "test/requestPlugins/#0".to_string(),
        )
    }
}

use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_common::ext_proc::proto::external_processor_server::{ExternalProcessor, ExternalProcessorServer};
use edgion_common::ext_proc::proto::{HeadersResponse, ProcessingMode as WireProcessingMode, ProcessingResponse};
use edgion_resources::plugin_configs::{BodySendMode, GrpcServiceConfig, HeaderSendMode};
use parking_lot::Mutex;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

#[test]
fn response_terminal_plan_uses_real_header_eos_and_both_future_phases() {
    let mut mode = ProcessingMode::default();
    mode.response_body_mode = BodySendMode::Streamed;
    mode.response_trailer_mode = HeaderSendMode::Send;
    assert_eq!(
        response_terminal_plan(&mode, false),
        ResponseTerminalPlan {
            header_last: false,
            install_body: true,
            install_trailer: true,
        }
    );
    assert_eq!(
        response_terminal_plan(&mode, true),
        ResponseTerminalPlan {
            header_last: true,
            install_body: false,
            install_trailer: false,
        }
    );

    mode.response_body_mode = BodySendMode::None;
    assert_eq!(
        response_terminal_plan(&mode, false),
        ResponseTerminalPlan {
            header_last: false,
            install_body: false,
            install_trailer: true,
        }
    );
    mode.response_trailer_mode = HeaderSendMode::Skip;
    assert!(response_terminal_plan(&mode, false).header_last);
}

#[derive(Clone, Copy)]
enum LiveOverrideOn {
    RequestHeaders,
    ResponseHeaders,
}

struct LiveHalfCloseProcessor {
    override_on: LiveOverrideOn,
    mode_override: WireProcessingMode,
    observed: Mutex<Option<tokio::sync::oneshot::Sender<(usize, bool, bool)>>>,
}

#[tonic::async_trait]
impl ExternalProcessor for LiveHalfCloseProcessor {
    type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

    async fn process(
        &self,
        request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let observed = self.observed.lock().take();
        let override_on = self.override_on;
        let mode_override = self.mode_override;
        tokio::spawn(async move {
            let mut phases = 0;
            let observation = loop {
                match inbound.message().await {
                    Ok(Some(message)) => {
                        phases += 1;
                        let (response, attach_override) = match message.request {
                            Some(processing_request::Request::RequestHeaders(_)) => (
                                processing_response::Response::RequestHeaders(HeadersResponse {
                                    response: Some(CommonResponse::default()),
                                }),
                                matches!(override_on, LiveOverrideOn::RequestHeaders),
                            ),
                            Some(processing_request::Request::ResponseHeaders(_)) => (
                                processing_response::Response::ResponseHeaders(HeadersResponse {
                                    response: Some(CommonResponse::default()),
                                }),
                                matches!(override_on, LiveOverrideOn::ResponseHeaders),
                            ),
                            _ => break (phases, false, false),
                        };
                        if tx
                            .send(Ok(ProcessingResponse {
                                response: Some(response),
                                mode_override: attach_override.then_some(mode_override),
                            }))
                            .await
                            .is_err()
                        {
                            break (phases, false, true);
                        }
                    }
                    Ok(None) => break (phases, true, false),
                    Err(_) => break (phases, false, true),
                }
            };
            drop(tx);
            if let Some(observed) = observed {
                let _ = observed.send(observation);
            }
        });
        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }
}

async fn spawn_live_half_close_processor(
    override_on: LiveOverrideOn,
    mode_override: WireProcessingMode,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
    tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(ExternalProcessorServer::new(LiveHalfCloseProcessor {
                override_on,
                mode_override,
                observed: Mutex::new(Some(observed_tx)),
            }))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    (address, observed_rx, server)
}

struct LiveImmediateProcessor {
    observed: Mutex<Option<tokio::sync::oneshot::Sender<(usize, bool, bool)>>>,
}

#[tonic::async_trait]
impl ExternalProcessor for LiveImmediateProcessor {
    type ProcessStream = ReceiverStream<Result<ProcessingResponse, tonic::Status>>;

    async fn process(
        &self,
        request: tonic::Request<tonic::Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<Self::ProcessStream>, tonic::Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let observed = self.observed.lock().take();
        tokio::spawn(async move {
            let mut phases = 0;
            let mut clean_eof = false;
            let mut cancelled = false;
            while let Ok(Some(message)) = inbound.message().await {
                phases += 1;
                let response = match message.request {
                    Some(processing_request::Request::RequestHeaders(_)) => {
                        processing_response::Response::ImmediateResponse(ImmediateResponse {
                            status: Some(edgion_common::ext_proc::proto::HttpStatus { code: 403 }),
                            headers: Some(edgion_common::ext_proc::proto::HeaderMutation {
                                set_headers: vec![edgion_common::ext_proc::proto::HeaderValueOption {
                                    header: Some(edgion_common::ext_proc::proto::HeaderValue {
                                        key: "x-ext-proc-denied".to_string(),
                                        value: String::new(),
                                        raw_value: b"1".to_vec(),
                                    }),
                                    append_action:
                                        edgion_common::ext_proc::proto::HeaderAppendAction::OverwriteIfExistsOrAdd
                                            as i32,
                                    keep_empty_value: false,
                                }],
                                remove_headers: Vec::new(),
                            }),
                            ..Default::default()
                        })
                    }
                    Some(processing_request::Request::ResponseHeaders(_)) => {
                        processing_response::Response::ResponseHeaders(HeadersResponse {
                            response: Some(CommonResponse::default()),
                        })
                    }
                    _ => break,
                };
                if tx
                    .send(Ok(ProcessingResponse {
                        response: Some(response),
                        mode_override: None,
                    }))
                    .await
                    .is_err()
                {
                    cancelled = true;
                    break;
                }
            }
            if !cancelled {
                match inbound.message().await {
                    Ok(None) => clean_eof = true,
                    Ok(Some(_)) => phases += 1,
                    Err(_) => cancelled = true,
                }
            }
            drop(tx);
            if let Some(observed) = observed {
                let _ = observed.send((phases, clean_eof, cancelled));
            }
        });
        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }
}

async fn spawn_live_immediate_processor() -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
    tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(ExternalProcessorServer::new(LiveImmediateProcessor {
                observed: Mutex::new(Some(observed_tx)),
            }))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    (address, observed_rx, server)
}

fn terminal_reduction(base: &ProcessingMode) -> ProcessingMode {
    ProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: base.response_header_mode,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::None,
        response_trailer_mode: HeaderSendMode::Skip,
    }
}

fn wire_terminal_reduction(response_header_mode: HeaderSendMode) -> WireProcessingMode {
    WireProcessingMode {
        request_header_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
        response_header_mode: match response_header_mode {
            HeaderSendMode::Send => edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Send as i32,
            HeaderSendMode::Skip => edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
        },
        request_body_mode: edgion_common::ext_proc::proto::processing_mode::BodySendMode::None as i32,
        response_body_mode: edgion_common::ext_proc::proto::processing_mode::BodySendMode::None as i32,
        request_trailer_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
        response_trailer_mode: edgion_common::ext_proc::proto::processing_mode::HeaderSendMode::Skip as i32,
    }
}

fn expect_empty_request_headers(session: &mut MockPluginSession) {
    session.expect_get_method().return_const("POST".to_string());
    session.expect_get_uri().return_const("/half-close".to_string());
    session.expect_request_header_map().return_const(http::HeaderMap::new());
}

async fn assert_clean_half_close(
    path: &str,
    observed: tokio::sync::oneshot::Receiver<(usize, bool, bool)>,
    server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let (phases, clean_eof, cancelled) = tokio::time::timeout(std::time::Duration::from_secs(2), observed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(phases, 1, "{path} sent a phase after its dynamic terminal reduction");
    assert!(clean_eof, "{path} did not produce processor-side EOF");
    assert!(!cancelled, "{path} was observed as cancellation");
    server.abort();
}

fn streamed_config(failure_mode_allow: bool, header_send: bool) -> ExtProcConfig {
    use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode};
    let mut config = test_config(failure_mode_allow);
    config.processing_mode.request_body_mode = BodySendMode::Streamed;
    if !header_send {
        config.processing_mode.request_header_mode = HeaderSendMode::Skip;
    }
    config
}

/// STREAMED on a session that cannot supply the transport fact
/// (custom-connector, subrequest, mocks) must fail per policy, before any
/// network work.
#[tokio::test]
async fn streamed_without_transport_fact_fails_per_policy() {
    let ext_proc = ExtProc::build(&streamed_config(false, true)).expect("build must not dial");
    let mut session = MockPluginSession::new();
    session.expect_install_attached_ext_proc().times(1).return_const(true);
    session.expect_request_headers_end_stream().return_const(None);
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "streamed-unsupported")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

/// SKIP headers + a body-bearing request: the handler installs without any
/// network exchange, and the request-stage verdict is GoodNext.
#[tokio::test]
async fn streamed_skip_headers_installs_handler_without_network() {
    let ext_proc = ExtProc::build(&streamed_config(false, false)).expect("build must not dial");
    let mut session = MockPluginSession::new();
    session.expect_install_attached_ext_proc().times(1).return_const(true);
    session.expect_request_headers_end_stream().return_const(Some(false));
    session.expect_current_dye_plan().return_const(None);
    session
        .expect_install_request_body_stream_handler()
        .times(1)
        .return_const(true);
    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::GoodNext));
}

#[tokio::test]
async fn constructor_normalizes_allowed_override_modes() {
    let mut config = test_config(false);
    config.processing_mode.response_header_mode = HeaderSendMode::Send;
    config.processing_mode.response_trailer_mode = HeaderSendMode::Send;
    config.allow_mode_override = true;
    let configured = ProcessingMode {
        request_header_mode: HeaderSendMode::Send,
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::Skip,
        response_body_mode: BodySendMode::Skip,
        response_trailer_mode: HeaderSendMode::Send,
    };
    config.allowed_override_modes = vec![configured.clone()];
    let ext_proc = ExtProc::build(&config).expect("build must not dial");
    assert_eq!(
        ext_proc.allowed_override_modes.as_ref(),
        &[ProcessingMode {
            request_header_mode: HeaderSendMode::Skip,
            response_header_mode: HeaderSendMode::Send,
            request_body_mode: BodySendMode::None,
            response_body_mode: BodySendMode::None,
            response_trailer_mode: HeaderSendMode::Send,
        }]
    );
    assert_eq!(config.allowed_override_modes, vec![configured]);
    assert_eq!(ext_proc.mode, config.processing_mode);
    let companion = ext_proc.clone();
    assert!(Arc::ptr_eq(
        &ext_proc.allowed_override_modes,
        &companion.allowed_override_modes
    ));
}

#[tokio::test]
async fn response_framing_reservation_remains_base_mode_derived_under_override() {
    let mut config = test_config(false);
    config.processing_mode.request_header_mode = HeaderSendMode::Skip;
    config.processing_mode.response_body_mode = BodySendMode::Streamed;
    config.allow_mode_override = true;
    let mut reduction = config.processing_mode.clone();
    reduction.response_body_mode = BodySendMode::None;
    config.allowed_override_modes = vec![reduction];
    let ext_proc = ExtProc::build(&config).expect("build must not dial");

    let mut session = MockPluginSession::new();
    session
        .expect_install_attached_ext_proc()
        .withf(|_, _, reserve_framing| *reserve_framing)
        .times(1)
        .return_const(true);
    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.run_request(&mut session, &mut log).await;

    assert!(matches!(result, PluginRunningResult::GoodNext));
}

#[tokio::test]
async fn dynamic_request_header_reductions_cleanly_half_close_live_streams() {
    for (path, request_body_mode) in [
        ("streamed-request", BodySendMode::Streamed),
        ("buffered-request", BodySendMode::Buffered),
    ] {
        let base = ProcessingMode {
            request_header_mode: HeaderSendMode::Send,
            response_header_mode: HeaderSendMode::Skip,
            request_body_mode,
            response_body_mode: BodySendMode::None,
            response_trailer_mode: HeaderSendMode::Skip,
        };
        let reduction = terminal_reduction(&base);
        let (address, observed, server) = spawn_live_half_close_processor(
            LiveOverrideOn::RequestHeaders,
            wire_terminal_reduction(base.response_header_mode),
        )
        .await;
        let config = ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: address.to_string(),
                timeout_ms: 2_000,
                ..ExtProcConfig::default().grpc_service
            },
            processing_mode: base,
            allow_mode_override: true,
            allowed_override_modes: vec![reduction],
            ..Default::default()
        };
        let ext_proc = ExtProc::build(&config).unwrap();
        let mut session = MockPluginSession::new();
        session.expect_install_attached_ext_proc().return_const(true);
        expect_empty_request_headers(&mut session);
        if request_body_mode == BodySendMode::Streamed {
            session.expect_request_headers_end_stream().return_const(Some(false));
        } else {
            session
                .expect_read_request_body()
                .returning(|_| Ok(Bytes::from_static(b"prebuilt-buffered-body")));
        }

        let result = ext_proc.run_request(&mut session, &mut PluginLog::new("ExtProc")).await;
        assert!(matches!(result, PluginRunningResult::GoodNext), "{path} failed");
        assert_clean_half_close(path, observed, server).await;
    }
}

#[tokio::test]
async fn dynamic_response_header_reduction_cleanly_half_closes_live_stream() {
    let base = ProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::Streamed,
        response_trailer_mode: HeaderSendMode::Skip,
    };
    let reduction = terminal_reduction(&base);
    let (address, observed, server) = spawn_live_half_close_processor(
        LiveOverrideOn::ResponseHeaders,
        wire_terminal_reduction(base.response_header_mode),
    )
    .await;
    let config = ExtProcConfig {
        grpc_service: GrpcServiceConfig {
            target: address.to_string(),
            timeout_ms: 2_000,
            ..ExtProcConfig::default().grpc_service
        },
        processing_mode: base.clone(),
        allow_mode_override: true,
        allowed_override_modes: vec![reduction.clone()],
        ..Default::default()
    };
    let ext_proc = ExtProc::build(&config).unwrap();
    let owner = Arc::new(AttachedExtProc::new(
        ext_proc.client.clone(),
        base,
        true,
        Arc::clone(&ext_proc.allowed_override_modes),
    ));
    let mut session = MockPluginSession::new();
    session
        .expect_attached_ext_proc()
        .return_const(Some(Arc::clone(&owner)));
    session.expect_get_response_status().return_const(Some(200));
    session.expect_response_headers_bytes().return_const(Vec::new());
    session
        .expect_upstream_response_headers_end_stream()
        .return_const(Some(false));

    let result = ext_proc
        .run_upstream_response(&mut session, &mut PluginLog::new("ExtProc"))
        .await;
    assert!(matches!(result, PluginRunningResult::GoodNext));
    assert_clean_half_close("response-body", observed, server).await;
}

#[tokio::test]
async fn disabled_request_header_immediate_deactivates_response_companion_with_clean_eof() {
    let (address, observed, server) = spawn_live_immediate_processor().await;
    let config = ExtProcConfig {
        grpc_service: GrpcServiceConfig {
            target: address.to_string(),
            timeout_ms: 2_000,
            ..ExtProcConfig::default().grpc_service
        },
        processing_mode: ProcessingMode {
            request_header_mode: HeaderSendMode::Send,
            response_header_mode: HeaderSendMode::Send,
            ..Default::default()
        },
        disable_immediate_response: true,
        failure_mode_allow: false,
        ..Default::default()
    };
    let ext_proc = ExtProc::build(&config).unwrap();
    let installed = Arc::new(Mutex::new(None));
    let installed_capture = Arc::clone(&installed);
    let mut request_session = MockPluginSession::new();
    request_session.expect_queue_local_reply_header_op().never();
    request_session
        .expect_install_attached_ext_proc()
        .times(1)
        .returning(move |_, owner, _| {
            *installed_capture.lock() = Some(owner);
            true
        });
    expect_empty_request_headers(&mut request_session);

    let result = ext_proc
        .run_request(&mut request_session, &mut PluginLog::new("ExtProc"))
        .await;
    assert!(matches!(result, PluginRunningResult::GoodNext));
    let owner = installed.lock().clone().expect("request owner installed");
    assert_eq!(owner.test_state(), ("suppressed", "immediate-disabled", true));

    let mut response_session = MockPluginSession::new();
    response_session
        .expect_attached_ext_proc()
        .times(1)
        .return_const(Some(owner));
    let response_result = ext_proc
        .run_upstream_response(&mut response_session, &mut PluginLog::new("ExtProc"))
        .await;
    assert!(matches!(response_result, PluginRunningResult::GoodNext));

    assert_clean_half_close("disabled-immediate", observed, server).await;
}

/// The install itself can refuse (capability re-check); the plugin must
/// then apply its failure policy rather than continue silently.
#[tokio::test]
async fn streamed_install_refusal_fails_per_policy() {
    let ext_proc = ExtProc::build(&streamed_config(true, false)).expect("build must not dial");
    let mut session = MockPluginSession::new();
    session.expect_install_attached_ext_proc().times(1).return_const(true);
    session.expect_request_headers_end_stream().return_const(Some(false));
    session.expect_current_dye_plan().return_const(None);
    session
        .expect_install_request_body_stream_handler()
        .times(1)
        .return_const(false);
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "streamed-unsupported")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::GoodNext));
}

/// A structurally valid config (routable authority, never actually
/// dialed by these tests — `fail_mode` is exercised directly).
fn test_config(failure_mode_allow: bool) -> ExtProcConfig {
    ExtProcConfig {
        grpc_service: GrpcServiceConfig {
            target: "127.0.0.1:1".to_string(),
            ..ExtProcConfig::default().grpc_service
        },
        failure_mode_allow,
        ..Default::default()
    }
}

#[test]
fn immediate_validation_is_suppression_first_and_preserves_exact_zero() {
    use edgion_common::ext_proc::proto::{GrpcStatus, HttpStatus};

    assert!(matches!(
        immediate_action(ImmediateResponse::default(), true),
        Ok(ImmediateAction::Suppress)
    ));
    assert!(immediate_action(ImmediateResponse::default(), false).is_err());
    assert!(immediate_action(
        ImmediateResponse {
            status: Some(HttpStatus { code: 199 }),
            ..Default::default()
        },
        false,
    )
    .is_err());
    assert!(immediate_action(
        ImmediateResponse {
            status: Some(HttpStatus { code: 403 }),
            grpc_status: Some(GrpcStatus { status: 17 }),
            ..Default::default()
        },
        false,
    )
    .is_err());

    let Ok(ImmediateAction::Reply(valid)) = immediate_action(
        ImmediateResponse {
            status: Some(HttpStatus { code: 403 }),
            grpc_status: Some(GrpcStatus { status: 0 }),
            body: b"exact".to_vec(),
            ..Default::default()
        },
        false,
    ) else {
        panic!("valid exact status must produce a reply");
    };
    assert_eq!(valid.grpc_status, Some(0));
    assert_eq!(valid.body.as_deref(), Some(&b"exact"[..]));
}

#[test]
fn immediate_reply_uses_exact_grpc_result_only_for_canonical_grpc() {
    let mut grpc_session = MockPluginSession::new();
    grpc_session.expect_is_grpc_request().return_const(true);
    let mut log = PluginLog::new("ExtProc");
    let result = immediate_reply(
        &mut grpc_session,
        &mut log,
        ValidatedImmediateResponse {
            status: 403,
            grpc_status: Some(8),
            headers: None,
            body: Some(Bytes::from_static(b"quota")),
        },
    );
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 8,
            message: Some("quota".to_string())
        }
    );

    let mut http_session = MockPluginSession::new();
    http_session.expect_is_grpc_request().return_const(false);
    let mut log = PluginLog::new("ExtProc");
    let result = immediate_reply(
        &mut http_session,
        &mut log,
        ValidatedImmediateResponse {
            status: 403,
            grpc_status: Some(8),
            headers: None,
            body: Some(Bytes::from_static(b"denied")),
        },
    );
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"denied"))
        }
    );

    let mut grpc_without_exact = MockPluginSession::new();
    grpc_without_exact.expect_is_grpc_request().return_const(true);
    let mut log = PluginLog::new("ExtProc");
    let result = immediate_reply(
        &mut grpc_without_exact,
        &mut log,
        ValidatedImmediateResponse {
            status: 403,
            grpc_status: None,
            headers: None,
            body: Some(Bytes::from_static(b"mapped")),
        },
    );
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"mapped"))
        }
    );
}

#[test]
fn immediate_body_preserves_binary_bytes_and_allocation() {
    let body = vec![0, 0xff, b'a', 0x80];
    let allocation = body.as_ptr();
    let Ok(ImmediateAction::Reply(valid)) = immediate_action(
        ImmediateResponse {
            status: Some(edgion_common::ext_proc::proto::HttpStatus { code: 403 }),
            body,
            ..Default::default()
        },
        false,
    ) else {
        panic!("binary HTTP body must be accepted");
    };
    let body = valid.body.unwrap();
    assert_eq!(body.as_ref(), &[0, 0xff, b'a', 0x80]);
    assert_eq!(body.as_ptr(), allocation);
}

#[test]
fn immediate_reply_binary_http_body_is_unchanged() {
    for exact_status in [None, Some(0), Some(8)] {
        let mut session = MockPluginSession::new();
        session.expect_is_grpc_request().return_const(false);
        session.expect_record_error().never();
        let body = Bytes::from_static(&[0, 0xff, b'a', 0x80]);
        assert_eq!(
            immediate_reply(
                &mut session,
                &mut PluginLog::new("ExtProc"),
                ValidatedImmediateResponse {
                    status: 403,
                    grpc_status: exact_status,
                    headers: None,
                    body: Some(body.clone()),
                },
            ),
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(body)
            }
        );
    }
}

#[test]
fn immediate_reply_exact_grpc_invalid_body_records_error_without_message() {
    let mut session = MockPluginSession::new();
    session.expect_is_grpc_request().return_const(true);
    session
        .expect_record_error()
        .withf(|code| *code == EdgionErrorCode::PluginGrpcMessageBodyNotUtf8)
        .times(1)
        .return_const(());
    assert_eq!(
        immediate_reply(
            &mut session,
            &mut PluginLog::new("ExtProc"),
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: Some(0),
                headers: None,
                body: Some(Bytes::from_static(&[0xff])),
            },
        ),
        PluginRunningResult::ErrGrpcError {
            grpc_status: 0,
            message: None
        }
    );
}

#[test]
fn immediate_reply_exact_grpc_absent_body_preserves_zero() {
    let mut session = MockPluginSession::new();
    session.expect_is_grpc_request().return_const(true);
    session.expect_record_error().never();
    assert_eq!(
        immediate_reply(
            &mut session,
            &mut PluginLog::new("ExtProc"),
            ValidatedImmediateResponse {
                status: 403,
                grpc_status: Some(0),
                headers: None,
                body: None,
            },
        ),
        PluginRunningResult::ErrGrpcError {
            grpc_status: 0,
            message: None
        }
    );
}

#[test]
fn immediate_reply_queues_mutations_for_fresh_local_reply() {
    use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption};

    let mut session = MockPluginSession::new();
    session.expect_is_grpc_request().return_const(false);
    session.expect_set_response_header().never();
    session
        .expect_queue_local_reply_header_op()
        .withf(|op| {
            matches!(
                op,
                crate::PendingHeaderOp::Set { name, value }
                    if name == "x-local" && value == "reply"
            )
        })
        .times(1)
        .returning(|_| Ok(()));
    let mut log = PluginLog::new("ExtProc");
    let result = immediate_reply(
        &mut session,
        &mut log,
        ValidatedImmediateResponse {
            status: 403,
            grpc_status: None,
            headers: Some(HeaderMutation {
                set_headers: vec![HeaderValueOption {
                    header: Some(HeaderValue {
                        key: "x-local".to_string(),
                        value: "reply".to_string(),
                        raw_value: Vec::new(),
                    }),
                    append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                    keep_empty_value: false,
                }],
                remove_headers: Vec::new(),
            }),
            body: None,
        },
    );
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 403, .. }));
}

#[tokio::test]
async fn response_processor_install_failure_terminalizes_owner_per_policy() {
    for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
        let ext_proc = ExtProc::build(&test_config(failure_mode_allow)).expect("build must not dial");
        let owner = AttachedExtProc::test_lazy();
        let mut session = MockPluginSession::new();
        session.expect_record_outcome().times(1).return_const(());
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.fail_response_processor_install(&owner, &mut session, &mut log);
        assert_eq!(
            owner.test_state(),
            (expected_outcome, "response-processor-install", true)
        );
        assert_eq!(matches!(result, PluginRunningResult::GoodNext), failure_mode_allow);
    }
}

#[tokio::test]
async fn disabled_immediate_handlers_close_owner_without_mutation_or_failure() {
    let mut config = test_config(false);
    config.disable_immediate_response = true;
    let ext_proc = ExtProc::build(&config).expect("client build must not touch the network");

    let request_owner = AttachedExtProc::test_lazy();
    let mut request_session = MockPluginSession::new();
    let mut request_log = PluginLog::new("ExtProc");
    assert_eq!(
        ext_proc
            .handle_request_immediate(
                &request_owner,
                &mut request_session,
                &mut request_log,
                ImmediateResponse::default(),
            )
            .await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(request_owner.test_state().0, "suppressed");

    let response_owner = AttachedExtProc::test_lazy();
    let mut response_session = MockPluginSession::new();
    let mut response_log = PluginLog::new("ExtProc");
    assert_eq!(
        ext_proc
            .handle_response_header_immediate(
                &response_owner,
                &mut response_session,
                &mut response_log,
                ImmediateResponse::default(),
            )
            .await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(response_owner.test_state().0, "suppressed");
}

#[tokio::test]
async fn unsupported_common_fields_are_rejected_before_mutation() {
    use edgion_common::ext_proc::proto::{HeaderMap, HeaderValue};

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session.expect_record_outcome().times(2).return_const(());
    session.expect_set_request_header_value().never();
    for common in [
        CommonResponse {
            clear_route_cache: true,
            ..Default::default()
        },
        CommonResponse {
            trailers: Some(HeaderMap {
                headers: vec![HeaderValue {
                    key: "x-late".to_string(),
                    value: "value".to_string(),
                    raw_value: Vec::new(),
                }],
            }),
            ..Default::default()
        },
    ] {
        let mut log = PluginLog::new("ExtProc");
        assert!(matches!(
            ext_proc
                .apply_request_common_response(&mut session, &mut log, None, Some(common), true)
                .await,
            Err(PluginRunningResult::ErrResponse { status: 500, .. })
        ));
    }
}

#[tokio::test]
async fn malformed_header_override_is_rejected_before_header_mutation() {
    use edgion_common::ext_proc::proto::{HeaderMutation, ProcessingMode as WireProcessingMode};
    use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode};

    let mut config = test_config(false);
    config.processing_mode.request_body_mode = BodySendMode::Buffered;
    config.allow_mode_override = true;
    config.allowed_override_modes = vec![ProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        request_body_mode: BodySendMode::None,
        ..Default::default()
    }];
    let ext_proc = ExtProc::build(&config).expect("build must not dial");
    let owner = AttachedExtProc::new(
        ext_proc.client.clone(),
        config.processing_mode.clone(),
        true,
        Arc::clone(&ext_proc.allowed_override_modes),
    );
    let before = owner.effective_mode().unwrap();
    let common = CommonResponse {
        header_mutation: Some(HeaderMutation::default()),
        ..Default::default()
    };
    let malformed = WireProcessingMode {
        response_header_mode: 99,
        ..Default::default()
    };

    assert!(validate_request_header_verdict(&owner, &common, false, Some(&malformed)).is_err());
    assert_eq!(owner.effective_mode().unwrap(), before);
}

#[tokio::test]
async fn response_header_common_unsupported_fields_are_rejected_before_mutation() {
    use edgion_common::ext_proc::proto::{HeaderMap, HeaderValue};

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session.expect_record_outcome().times(2).return_const(());
    session.expect_set_response_header().never();
    for common in [
        CommonResponse {
            clear_route_cache: true,
            ..Default::default()
        },
        CommonResponse {
            trailers: Some(HeaderMap {
                headers: vec![HeaderValue {
                    key: "x-late".to_string(),
                    value: "value".to_string(),
                    raw_value: Vec::new(),
                }],
            }),
            ..Default::default()
        },
    ] {
        let mut log = PluginLog::new("ExtProc");
        assert!(matches!(
            ext_proc.apply_response_headers_response(&mut session, &mut log, Some(common)),
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
    }
}

#[tokio::test]
async fn request_body_response_overwrites_buffered_body() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == b"processed")
        .returning(|_| Ok(()));
    let response = CommonResponse {
        body_mutation: Some(BodyMutation {
            mutation: Some(body_mutation::Mutation::Body(b"processed".to_vec())),
        }),
        ..Default::default()
    };
    let mut log = PluginLog::new("ExtProc");
    assert!(ext_proc
        .apply_request_common_response(&mut session, &mut log, None, Some(response), true)
        .await
        .is_ok());
}

#[tokio::test]
async fn header_continue_ignores_header_phase_body_mutation() {
    use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session.expect_overwrite_request_body().never();
    let response = CommonResponse {
        status: ResponseStatus::Continue as i32,
        body_mutation: Some(BodyMutation {
            mutation: Some(body_mutation::Mutation::Body(b"must-not-apply".to_vec())),
        }),
        ..Default::default()
    };

    let mut log = PluginLog::new("ExtProc");
    assert!(ext_proc
        .apply_request_common_response(&mut session, &mut log, None, Some(response), false)
        .await
        .is_ok());
}

#[tokio::test]
async fn request_body_response_applies_header_mutation() {
    use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption};

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-body-verdict" && value == "applied")
        .times(1)
        .returning(|_, _| Ok(()));
    let response = CommonResponse {
        header_mutation: Some(HeaderMutation {
            set_headers: vec![HeaderValueOption {
                header: Some(HeaderValue {
                    key: "x-body-verdict".to_string(),
                    value: "applied".to_string(),
                    raw_value: Vec::new(),
                }),
                append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                keep_empty_value: false,
            }],
            remove_headers: Vec::new(),
        }),
        ..Default::default()
    };

    let mut log = PluginLog::new("ExtProc");
    assert!(ext_proc
        .apply_request_common_response(&mut session, &mut log, None, Some(response), true)
        .await
        .is_ok());
}

#[tokio::test]
async fn request_header_and_buffered_body_reject_missing_unknown_and_malformed_common_verdicts() {
    use edgion_common::ext_proc::proto::BodyMutation;

    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session.expect_record_outcome().times(3).return_const(());
    let cases = [
        None,
        Some(CommonResponse {
            status: 99,
            ..Default::default()
        }),
        Some(CommonResponse {
            body_mutation: Some(BodyMutation { mutation: None }),
            ..Default::default()
        }),
    ];
    for (index, common) in cases.into_iter().enumerate() {
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc
            .apply_request_common_response(&mut session, &mut log, None, common, index != 0)
            .await;
        assert!(matches!(
            result,
            Err(PluginRunningResult::ErrResponse { status: 500, .. })
        ));
    }
}

#[tokio::test]
async fn response_header_continue_and_replace_uses_failure_policy() {
    let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "bad-verdict")
        .times(1)
        .return_const(());
    let response = CommonResponse {
        status: ResponseStatus::ContinueAndReplace as i32,
        ..Default::default()
    };

    let mut log = PluginLog::new("ExtProc");
    assert!(matches!(
        ext_proc.apply_response_headers_response(&mut session, &mut log, Some(response)),
        PluginRunningResult::GoodNext
    ));
}

#[tokio::test]
async fn request_body_mutation_failure_replaces_final_success_summary_per_policy() {
    use crate::request_body::BodyReadError;
    use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

    for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
        let ext_proc =
            ExtProc::build(&test_config(failure_mode_allow)).expect("client build must not touch the network");
        let owner = AttachedExtProc::test_lazy();
        owner.mark_terminal("success", "normal-eos");
        let mut session = MockPluginSession::new();
        session
            .expect_overwrite_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        session
            .expect_record_outcome()
            .withf(move |outcome, value| {
                matches!(outcome, DyeOutcome::FailOpen) == failure_mode_allow && value == "body-mutation-failed"
            })
            .times(1)
            .return_const(());
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
            }),
            ..Default::default()
        };

        let result = ext_proc
            .apply_request_common_response(
                &mut session,
                &mut PluginLog::new("ExtProc"),
                Some(&owner),
                Some(response),
                true,
            )
            .await;
        assert_eq!(owner.test_state(), (expected_outcome, "request-stage-failure", true));
        assert_eq!(
            matches!(&result, Err(PluginRunningResult::GoodNext)),
            failure_mode_allow
        );
        assert_eq!(
            matches!(
                &result,
                Err(PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None
                })
            ),
            !failure_mode_allow
        );
    }
}

#[tokio::test]
async fn request_body_mutation_failure_deactivates_pending_response_per_policy() {
    use crate::request_body::BodyReadError;
    use edgion_common::ext_proc::proto::{
        body_mutation, BodyMutation, CommonResponse, HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption,
    };

    for (failure_mode_allow, expected_outcome) in [(true, "fail-open"), (false, "fail-close")] {
        let mut config = test_config(failure_mode_allow);
        config.processing_mode.response_header_mode = HeaderSendMode::Send;
        let ext_proc = ExtProc::build(&config).expect("client build must not touch the network");
        let owner = Arc::new(AttachedExtProc::new(
            ext_proc.client.clone(),
            config.processing_mode.clone(),
            ext_proc.allow_mode_override,
            Arc::clone(&ext_proc.allowed_override_modes),
        ));
        let mut request_session = MockPluginSession::new();
        request_session
            .expect_overwrite_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        request_session.expect_set_request_header_value().never();
        request_session
            .expect_record_outcome()
            .withf(move |outcome, value| {
                matches!(outcome, DyeOutcome::FailOpen) == failure_mode_allow && value == "body-mutation-failed"
            })
            .times(1)
            .return_const(());
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
            }),
            header_mutation: Some(HeaderMutation {
                set_headers: vec![HeaderValueOption {
                    header: Some(HeaderValue {
                        key: "x-must-not-apply".to_string(),
                        value: "after-failed-body".to_string(),
                        raw_value: Vec::new(),
                    }),
                    append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                    keep_empty_value: false,
                }],
                remove_headers: Vec::new(),
            }),
            ..Default::default()
        };

        let result = ext_proc
            .apply_request_common_response(
                &mut request_session,
                &mut PluginLog::new("ExtProc"),
                Some(owner.as_ref()),
                Some(response),
                true,
            )
            .await;
        assert_eq!(owner.test_state(), (expected_outcome, "request-stage-failure", true));
        assert_eq!(
            matches!(&result, Err(PluginRunningResult::GoodNext)),
            failure_mode_allow
        );
        assert_eq!(
            matches!(
                &result,
                Err(PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None
                })
            ),
            !failure_mode_allow
        );

        let mut response_session = MockPluginSession::new();
        response_session
            .expect_attached_ext_proc()
            .times(1)
            .return_const(Some(Arc::clone(&owner)));
        let response_result = ext_proc
            .run_upstream_response(&mut response_session, &mut PluginLog::new("ExtProc"))
            .await;
        assert!(matches!(response_result, PluginRunningResult::GoodNext));
    }
}

/// `fail_mode` is the single choke point both `run_request` and
/// `run_upstream_response` route through on a transport error or a
/// malformed/wrong-phase verdict. Fail-open arm: records `FailOpen` with
/// the kebab-cased `reason` as cause, then continues (`GoodNext`).
#[tokio::test]
async fn fail_mode_fail_open_records_outcome() {
    let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "unreachable")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.fail_mode(&mut session, &mut log, "unreachable", Some("deadline exceeded"));
    assert!(matches!(result, PluginRunningResult::GoodNext));
}

/// Fail-close arm: records `FailClose`, then returns `ErrResponse` (a
/// value the runtime writes AFTER this call returns — record_outcome is
/// therefore always ordered before that write by construction, since it
/// executes before `fail_mode`'s own `return`).
#[tokio::test]
async fn fail_mode_fail_close_records_outcome() {
    let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "bad-verdict")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("ExtProc");
    let result = ext_proc.fail_mode(&mut session, &mut log, "bad verdict", None);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}
