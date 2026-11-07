use super::*;
use crate::observe::RequestDiagnostic;
use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
use edgion_resources::EdgionErrorCode;
use edgion_waf::seclang::{RequestHead, RuleCheckPolicy, RuleEngineMode, RuntimeLimits, Transaction};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn terminate_grpc_reply_borrows_text_and_preserves_status_semantics() {
    let text = b"quota";
    let (mapped_status, default_msg) = crate::runtime::server::http_to_grpc_status(403);
    for exact in [None, Some(0), Some(8)] {
        let expected_status = exact.unwrap_or(mapped_status);
        let fallback = exact.is_none().then_some(default_msg);
        assert_eq!(
            terminate_grpc_reply(403, exact, None),
            (expected_status, fallback, false)
        );
        assert_eq!(
            terminate_grpc_reply(403, exact, Some(&[0xff])),
            (expected_status, fallback, true)
        );
        assert_eq!(
            terminate_grpc_reply(403, exact, Some(b"")),
            (expected_status, Some(""), false)
        );
        let (status, message, invalid) = terminate_grpc_reply(403, exact, Some(text));
        assert_eq!((status, message, invalid), (expected_status, Some("quota"), false));
        assert_eq!(message.unwrap().as_ptr(), text.as_ptr());
    }
}

#[tokio::test]
async fn terminate_http_binary_reply_preserves_body_and_framing() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    let body = Bytes::from_static(&[0, 0xff, b'a', 0x80]);
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(0),
        Some(body.clone()),
        vec![
            crate::PendingHeaderOp::Set {
                name: "content-length".to_string(),
                value: http::HeaderValue::from_static("999"),
            },
            crate::PendingHeaderOp::Set {
                name: "transfer-encoding".to_string(),
                value: http::HeaderValue::from_static("chunked"),
            },
        ],
    )
    .await;
    let header = session.response_written().unwrap();
    assert_eq!(header.status.as_u16(), 403);
    assert_eq!(header.headers.get("content-length").unwrap(), "4");
    assert!(!header.headers.contains_key("transfer-encoding"));
    assert!(!header.headers.contains_key("grpc-status"));
    drop(session);
    let mut wire = Vec::new();
    client.read_to_end(&mut wire).await.unwrap();
    let body_start = wire.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&wire[body_start..], body.as_ref());
}

#[tokio::test]
async fn invalid_terminate_status_uses_a_bodyless_final_500() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    write_terminate_reply(
        &mut session,
        &mut ctx,
        100,
        None,
        Some(Bytes::from_static(b"internal detail")),
        Vec::new(),
    )
    .await;

    assert_eq!(ctx.final_downstream_status(), Some(500));
    drop(session);
    let mut wire = Vec::new();
    client.read_to_end(&mut wire).await.unwrap();
    assert!(!wire
        .windows(b"internal detail".len())
        .any(|window| window == b"internal detail"));
}

#[tokio::test]
async fn terminate_grpc_invalid_body_records_diagnostic_without_exact_message() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.is_grpc_route_matched = true;
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(0),
        Some(Bytes::from_static(&[0xff])),
        Vec::new(),
    )
    .await;
    let header = session.response_written().unwrap();
    assert_eq!(header.headers.get("grpc-status").unwrap(), "0");
    assert!(!header.headers.contains_key("grpc-message"));
    assert_eq!(ctx.request_info.grpc_status(), Some(0));
    assert_eq!(
        ctx.diagnostics
            .entries()
            .iter()
            .filter(|entry| { **entry == RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8) })
            .count(),
        1
    );
}

fn streaming_waf_context(body_limit: u64) -> EdgionHttpContext {
    let config = WafConfig {
        mode: WafMode::On,
        ..crate::plugins::http::waf::fixture::config(
            "SecRule REQUEST_BODY \"@pm blocked\" \"id:1,phase:2,deny,status:409\"\n",
        )
    };
    let generation = Arc::new(config.compile_generation().expect("test WAF compiles"));
    let request = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"request-1",
        &[],
    );
    let mut transaction = Transaction::new_shared_with_policy(
        generation,
        request,
        RuntimeLimits::default(),
        edgion_waf::seclang::EvaluationMode::Accelerated,
        RuleEngineMode::On,
        RuleCheckPolicy::default(),
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    transaction.start_request_body_stream().unwrap();
    let owner = Arc::new(());
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_waf(Some(crate::ctx::WafSelection::new(
        Arc::clone(&owner),
        crate::ctx::WafLogSource::Route,
        0,
        crate::ctx::WafLogMode::On,
    ))));
    let waf = ctx.waf.as_deref_mut().unwrap();
    assert!(waf.install(&owner, transaction));
    assert!(waf.enable_request_body_streaming(&owner, body_limit));
    ctx.activate_waf_request_body_streaming();
    ctx
}

fn waf_log_value(ctx: &EdgionHttpContext) -> serde_json::Value {
    serde_json::to_value(ctx.waf_access_log().expect("selected WAF summary")).unwrap()
}

#[test]
fn native_waf_stream_scans_cross_chunk_without_body_storage() {
    let mut ctx = streaming_waf_context(1024);

    assert!(run_waf_body_stream(b"prefix-blo", RequestBodyEvent::Data, &mut ctx).is_none());
    assert!(matches!(
        run_waf_body_stream(b"cked", RequestBodyEvent::Data, &mut ctx),
        Some(ChunkVerdict::Terminate { status: 409, .. })
    ));
    assert!(!ctx.request_body.has_cached_body());
    assert!(!ctx.waf.as_deref().unwrap().request_body_streaming());
    assert!(ctx.stage_logs().is_empty());
    let waf = waf_log_value(&ctx);
    assert_eq!(waf["outcome"], "blocked");
    assert_eq!(waf["phases"]["request_body"]["findings"], 1);
    assert_eq!(waf["intervention"]["phase"], "request_body");
    assert_eq!(waf["intervention"]["rule_id"], 1);
    assert_eq!(waf["intervention"]["status"], 409);
}

#[test]
fn native_waf_stream_fails_closed_on_gateway_cap_and_abandoned_body() {
    let mut capped = streaming_waf_context(3);
    assert!(matches!(
        run_waf_body_stream(b"four", RequestBodyEvent::Data, &mut capped),
        Some(ChunkVerdict::Terminate { status: 413, .. })
    ));
    assert!(capped.stage_logs().is_empty());
    let capped_waf = waf_log_value(&capped);
    assert_eq!(capped_waf["outcome"], "error");
    assert_eq!(capped_waf["terminal_error"]["phase"], "request_body");
    assert_eq!(capped_waf["terminal_error"]["code"], "body_limit");
    assert_eq!(capped_waf["terminal_error"]["status"], 413);

    let mut abandoned = streaming_waf_context(1024);
    assert!(matches!(
        run_waf_body_stream(&[], RequestBodyEvent::Abandoned, &mut abandoned),
        Some(ChunkVerdict::Terminate { status: 400, .. })
    ));
    assert!(!abandoned.waf.as_deref().unwrap().request_body_streaming());
    assert!(abandoned.stage_logs().is_empty());
    let abandoned_waf = waf_log_value(&abandoned);
    assert_eq!(abandoned_waf["outcome"], "error");
    assert_eq!(abandoned_waf["terminal_error"]["code"], "body_stream_abandoned");
    assert_eq!(abandoned_waf["terminal_error"]["status"], 400);
}

#[test]
fn terminate_writer_classifies_grpc_route_without_content_type_fact() {
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = false;
    ctx.is_grpc_route_matched = true;
    assert!(is_canonical_grpc(&ctx));
}

#[test]
fn terminate_does_not_replace_a_response_selected_by_pingora() {
    assert!(!request_body_should_write_terminate_reply(true));
    assert!(request_body_should_write_terminate_reply(false));
}

#[tokio::test]
async fn grpc_route_only_terminate_writes_exact_status_and_local_header() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.is_grpc_route_matched = true;
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(8),
        Some(Bytes::from_static(b"quota")),
        vec![crate::PendingHeaderOp::Append {
            name: "x-local".to_string(),
            value: http::HeaderValue::from_static("reply"),
        }],
    )
    .await;

    let written = session.response_written().expect("gRPC local reply header");
    assert_eq!(written.status.as_u16(), 200);
    assert_eq!(written.headers.get("grpc-status").unwrap(), "8");
    assert_eq!(written.headers.get("x-local").unwrap(), "reply");
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
}

use crate::ctx::MirrorCancellation;
use crate::plugins::http::ext_proc::{BodyExchange, ExtProcStreamedBodyHandler};
use edgion_common::ext_proc::proto::{
    body_mutation, processing_response, BodyMutation, BodyResponse, CommonResponse, HttpBody, ProcessingResponse,
};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

#[derive(Default)]
struct ExchangeFacts {
    chunks: Vec<Vec<u8>>,
    eos: usize,
    cancelled: usize,
    terminal: Vec<&'static str>,
}

struct TestExchange {
    facts: Arc<Mutex<ExchangeFacts>>,
    mutation: Option<body_mutation::Mutation>,
    fail: bool,
    gate: Option<oneshot::Receiver<()>>,
}

#[async_trait::async_trait]
impl BodyExchange for TestExchange {
    async fn exchange_body(&mut self, body: HttpBody, _last: bool) -> Result<ProcessingResponse, tonic::Status> {
        {
            let mut facts = self.facts.lock();
            facts.chunks.push(body.body);
            facts.eos += usize::from(body.end_of_stream);
        }
        if let Some(gate) = self.gate.take() {
            gate.await.unwrap();
        }
        if self.fail {
            return Err(tonic::Status::unavailable("test dependency failed"));
        }
        Ok(ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    body_mutation: self.mutation.clone().map(|mutation| BodyMutation {
                        mutation: Some(mutation),
                    }),
                    ..Default::default()
                }),
            })),
            mode_override: None,
        })
    }
    fn cancel(&mut self) {
        self.facts.lock().cancelled += 1;
    }
    fn terminal(&mut self, outcome: &'static str, _reason: &'static str) {
        self.facts.lock().terminal.push(outcome);
    }
}

fn install_exchange(
    ctx: &mut EdgionHttpContext,
    mutation: Option<body_mutation::Mutation>,
    fail_open: bool,
    fail: bool,
    gate: Option<oneshot::Receiver<()>>,
) -> Arc<Mutex<ExchangeFacts>> {
    let facts = Arc::new(Mutex::new(ExchangeFacts::default()));
    assert!(
        ctx.push_request_body_stream_handler(Box::new(ExtProcStreamedBodyHandler::new(
            Box::new(TestExchange {
                facts: facts.clone(),
                mutation,
                fail,
                gate
            }),
            fail_open,
            false,
            None,
            true,
        )))
    );
    facts
}

fn install_mirror(
    ctx: &mut EdgionHttpContext,
) -> (mpsc::Receiver<Result<Bytes, std::io::Error>>, oneshot::Receiver<()>) {
    let (body_tx, body_rx) = mpsc::channel(8);
    let (cancellation, cancel_rx) = MirrorCancellation::new();
    ctx.set_mirror_state(crate::MirrorState::Streaming {
        body_tx,
        body_budget: crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(1024),
        writer_handle: tokio::spawn(async {}),
        cancellation,
        channel_full_timeout_ms: 0,
    });
    (body_rx, cancel_rx)
}

fn body_proxy() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("127.0.0.1:80/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    }
}

async fn body_session() -> (tokio::io::DuplexStream, Session) {
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    (client, session)
}

#[tokio::test]
async fn streamed_chain_waits_for_verdicts_and_mirrors_only_final_bytes() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.max_body_size_bytes = 6;
    let (release, gate) = oneshot::channel();
    let first = install_exchange(
        &mut ctx,
        Some(body_mutation::Mutation::Body(b"expanded-intermediate".to_vec())),
        false,
        false,
        Some(gate),
    );
    let second = install_exchange(
        &mut ctx,
        Some(body_mutation::Mutation::Body(b"end".to_vec())),
        false,
        false,
        None,
    );
    let (mut mirror, cancelled) = install_mirror(&mut ctx);
    let proxy = body_proxy();
    let mut body = Some(Bytes::from_static(b"abc"));
    {
        let future = request_body_filter_action(&proxy, &mut session, &mut body, RequestBodyEvent::Data, &mut ctx);
        tokio::pin!(future);
        assert!(futures::poll!(future.as_mut()).is_pending());
        assert_eq!(first.lock().chunks, vec![b"abc".to_vec()]);
        assert!(second.lock().chunks.is_empty());
        assert!(mirror.try_recv().is_err());
        release.send(()).unwrap();
        assert_eq!(future.await.unwrap(), RequestBodyAction::Continue);
    }
    assert_eq!(body.as_deref(), Some(b"end".as_slice()));
    assert_eq!(mirror.recv().await.unwrap().unwrap(), "end");
    assert_eq!(second.lock().chunks, vec![b"expanded-intermediate".to_vec()]);
    body = Some(Bytes::from_static(b"def"));
    assert_eq!(
        request_body_filter_action(&proxy, &mut session, &mut body, RequestBodyEvent::Complete, &mut ctx)
            .await
            .unwrap(),
        RequestBodyAction::Continue
    );
    assert_eq!(mirror.recv().await.unwrap().unwrap(), "end");
    assert!(mirror.recv().await.is_none());
    assert!(cancelled.await.is_err());
    assert_eq!(first.lock().eos, 1);
    assert_eq!(second.lock().eos, 1);
    assert!(!ctx.request_body.has_cached_body());
}

#[tokio::test]
async fn streamed_caps_are_cumulative_independent_and_fail_closed() {
    for (mutation, input, output_limit) in [
        (Some(body_mutation::Mutation::ClearBody(true)), b"ab".as_slice(), false),
        (
            Some(body_mutation::Mutation::Body(b"abc".to_vec())),
            b"a".as_slice(),
            true,
        ),
    ] {
        let (_client, mut session) = body_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_body_config.max_body_size_bytes = 3;
        let facts = install_exchange(&mut ctx, mutation, true, false, None);
        let (mut mirror, cancelled) = install_mirror(&mut ctx);
        let proxy = body_proxy();
        let mut first = Some(Bytes::copy_from_slice(input));
        assert_eq!(
            request_body_filter_action(&proxy, &mut session, &mut first, RequestBodyEvent::Data, &mut ctx)
                .await
                .unwrap(),
            RequestBodyAction::Continue
        );
        let forwarded = mirror.recv().await.unwrap().unwrap();
        assert_eq!(forwarded.len(), if output_limit { 3 } else { 0 });
        let mut second = Some(Bytes::copy_from_slice(input));
        assert_eq!(
            request_body_filter_action(&proxy, &mut session, &mut second, RequestBodyEvent::Data, &mut ctx)
                .await
                .unwrap(),
            RequestBodyAction::Terminate
        );
        assert_eq!(session.response_written().unwrap().status.as_u16(), 413);
        assert!(cancelled.await.is_ok());
        assert!(mirror.recv().await.is_none());
        assert_eq!(facts.lock().chunks.len(), if output_limit { 2 } else { 1 });
        assert_eq!(facts.lock().cancelled, 1);
        assert_eq!(facts.lock().eos, 0);
        assert!(ctx
            .diagnostics
            .entries()
            .contains(&RequestDiagnostic::Error(EdgionErrorCode::BodyReadTooLarge)));
    }
}

#[tokio::test]
async fn fail_open_deactivation_keeps_caps_and_passes_pre_processor_bytes() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.max_body_size_bytes = 3;
    let facts = install_exchange(&mut ctx, None, true, true, None);
    let (mut mirror, cancelled) = install_mirror(&mut ctx);
    let proxy = body_proxy();
    let mut body = Some(Bytes::from_static(b"abc"));
    assert_eq!(
        request_body_filter_action(&proxy, &mut session, &mut body, RequestBodyEvent::Data, &mut ctx)
            .await
            .unwrap(),
        RequestBodyAction::Continue
    );
    assert_eq!(mirror.recv().await.unwrap().unwrap(), "abc");
    assert!(ctx.request_body_handlers_selected());
    let mut body = Some(Bytes::from_static(b"d"));
    assert_eq!(
        request_body_filter_action(&proxy, &mut session, &mut body, RequestBodyEvent::Data, &mut ctx)
            .await
            .unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(session.response_written().unwrap().status.as_u16(), 413);
    assert!(cancelled.await.is_ok());
    assert_eq!(facts.lock().terminal, vec!["fail-open"]);
}

#[tokio::test]
async fn fail_close_chain_cancels_later_processors_without_rewriting_initiator() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    let first = install_exchange(&mut ctx, None, false, true, None);
    let later = install_exchange(&mut ctx, None, false, false, None);
    let (mut mirror, cancelled) = install_mirror(&mut ctx);
    let mut body = Some(Bytes::from_static(b"abc"));
    assert_eq!(
        request_body_filter_action(&body_proxy(), &mut session, &mut body, RequestBodyEvent::Data, &mut ctx)
            .await
            .unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(first.lock().terminal, vec!["fail-close"]);
    assert!(later.lock().chunks.is_empty());
    assert_eq!(later.lock().cancelled, 1);
    assert!(cancelled.await.is_ok());
    assert!(mirror.recv().await.is_none());
}

#[tokio::test]
async fn final_output_overflow_preserves_completed_processor_outcome() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.max_body_size_bytes = 1;
    let facts = install_exchange(
        &mut ctx,
        Some(body_mutation::Mutation::Body(b"ab".to_vec())),
        false,
        false,
        None,
    );
    let mut body = None;
    assert_eq!(
        request_body_filter_action(
            &body_proxy(),
            &mut session,
            &mut body,
            RequestBodyEvent::Complete,
            &mut ctx
        )
        .await
        .unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(session.response_written().unwrap().status.as_u16(), 413);
    assert_eq!(facts.lock().eos, 1);
    assert_eq!(facts.lock().cancelled, 0);
    assert!(facts.lock().terminal.is_empty());
}

#[tokio::test]
async fn trailers_reject_streamed_handlers_before_clean_eos_and_preserve_ordinary_requests() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    assert_eq!(
        request_trailer_filter(&mut session, &mut ctx).await.unwrap(),
        RequestBodyAction::Continue
    );
    let facts = install_exchange(&mut ctx, None, true, false, None);
    let (mut mirror, cancelled) = install_mirror(&mut ctx);
    assert_eq!(
        request_trailer_filter(&mut session, &mut ctx).await.unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(session.response_written().unwrap().status.as_u16(), 400);
    assert_eq!(facts.lock().cancelled, 1);
    assert_eq!(facts.lock().eos, 0);
    assert!(cancelled.await.is_ok());
    assert!(mirror.recv().await.is_none());
    assert!(ctx.diagnostics.entries().contains(&RequestDiagnostic::Error(
        EdgionErrorCode::BodyStreamTrailersUnsupported
    )));
}

#[tokio::test]
async fn streamed_rejection_preserves_already_committed_final_response() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    install_exchange(&mut ctx, None, false, false, None);
    session
        .write_response_header(Box::new(ResponseHeader::build(200, None).unwrap()), true)
        .await
        .unwrap();
    assert_eq!(
        request_trailer_filter(&mut session, &mut ctx).await.unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
    assert_eq!(ctx.final_downstream_status(), None);
}

#[tokio::test]
async fn abandoned_event_skips_accounts_and_never_sends_processor_eos() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.max_body_size_bytes = 0;
    let facts = install_exchange(&mut ctx, None, false, false, None);
    let (mut mirror, cancelled) = install_mirror(&mut ctx);
    let mut body = Some(Bytes::from_static(b"incomplete"));
    assert_eq!(
        request_body_filter_action(
            &body_proxy(),
            &mut session,
            &mut body,
            RequestBodyEvent::Abandoned,
            &mut ctx
        )
        .await
        .unwrap(),
        RequestBodyAction::Continue
    );
    assert!(session.response_written().is_none());
    assert!(facts.lock().chunks.is_empty());
    assert_eq!(facts.lock().cancelled, 1);
    assert!(cancelled.await.is_ok());
    assert!(mirror.recv().await.is_none());
}

#[tokio::test]
async fn eos_output_rejection_closes_future_response_transaction_before_reply() {
    let (_client, mut session) = body_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_body_config.max_body_size_bytes = 1;
    let facts = Arc::new(Mutex::new(ExchangeFacts::default()));
    assert!(
        ctx.push_request_body_stream_handler(Box::new(ExtProcStreamedBodyHandler::new(
            Box::new(TestExchange {
                facts: facts.clone(),
                mutation: Some(body_mutation::Mutation::Body(b"ab".to_vec())),
                fail: false,
                gate: None,
            }),
            false,
            false,
            None,
            false,
        )))
    );
    let mut body = None;
    assert_eq!(
        request_body_filter_action(
            &body_proxy(),
            &mut session,
            &mut body,
            RequestBodyEvent::Complete,
            &mut ctx
        )
        .await
        .unwrap(),
        RequestBodyAction::Terminate
    );
    assert_eq!(facts.lock().eos, 1);
    assert_eq!(facts.lock().cancelled, 1);
    assert!(facts.lock().terminal.is_empty());
    assert_eq!(session.response_written().unwrap().status.as_u16(), 413);
}

#[tokio::test]
async fn ordinary_hard_admission_counts_native_bytes_and_exempts_only_written_101() {
    for response_status in [None, Some(200), Some(101)] {
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 9\r\nConnection: upgrade\r\nUpgrade: websocket\r\n\r\n123456789")
            .await
            .unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.enforce_request_body_limit = true;
        ctx.request_body_config.enabled = false;
        ctx.request_body_config.max_body_size_bytes = 8;
        assert!(!request_body_exceeds_hard_limit(&session, &ctx));
        session.read_request_body().await.unwrap();
        assert!(request_body_exceeds_hard_limit(&session, &ctx));
        ctx.request_body_config.max_body_size_bytes = 9;
        assert!(!request_body_exceeds_hard_limit(&session, &ctx));
        ctx.request_body_config.max_body_size_bytes = 8;
        if let Some(status) = response_status {
            let mut header = ResponseHeader::build(status, None).unwrap();
            if status == 101 {
                header.insert_header("connection", "upgrade").unwrap();
                header.insert_header("upgrade", "websocket").unwrap();
            }
            session.write_response_header(Box::new(header), false).await.unwrap();
        }
        assert_eq!(
            request_body_exceeds_hard_limit(&session, &ctx),
            response_status != Some(101)
        );
        if response_status == Some(200) {
            let mut chunk = Some(Bytes::from_static(b"123456789"));
            assert_eq!(
                request_body_filter_action(
                    &body_proxy(),
                    &mut session,
                    &mut chunk,
                    RequestBodyEvent::Data,
                    &mut ctx
                )
                .await
                .unwrap(),
                RequestBodyAction::Terminate
            );
            assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
            assert_eq!(ctx.final_downstream_status(), None);
            assert!(ctx
                .diagnostics
                .entries()
                .contains(&RequestDiagnostic::Error(EdgionErrorCode::BodyReadTooLarge)));
        }
        assert!(ctx.request_body.cached_len().unwrap().is_none());
    }
}
