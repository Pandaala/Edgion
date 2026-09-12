//! Per-chunk request-body hook: drives installed streamed handlers, then the
//! request-mirror tee.
//!
//! The fast path is one nullable-state test shared by streamed handlers, native
//! WAF, observers, and the mirror tee.
//!
//! Handler chunks are post-mutation for the mirror: the tee runs after the
//! handlers so the mirror backend receives exactly what the primary upstream
//! receives.

use super::{pg_request_body_filter, EdgionHttpProxy};
use crate::ctx::{MirrorAbandonReason, WafErrorCode, WafLogPhase};
use crate::plugins::runtime::{ChunkVerdict, PingoraSessionAdapter, PluginSession, StageLogs};
use crate::EdgionHttpContext;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::{RequestBodyAction, RequestBodyEvent, Session};
use std::time::Instant;

type Handlers = crate::ctx::RequestBodyStreamHandlers;

#[inline]
fn is_canonical_grpc(ctx: &EdgionHttpContext) -> bool {
    ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched
}

pub async fn request_body_filter_action(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<Bytes>,
    event: RequestBodyEvent,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    // Every per-chunk consumer activates the same optional state at install
    // time. Requests without a handler, WAF stream, observer, or mirror pay one
    // nullable-pointer branch and no allocation.
    if !ctx.has_request_body_plugins() {
        return Ok(RequestBodyAction::Continue);
    }

    if ctx.has_request_body_stream_handlers() {
        // Handlers are moved out for the duration of the loop so the session
        // adapter (which borrows the whole ctx) can be built on the terminate
        // path. Normal returns restore the vector. If this future is cancelled,
        // the local handlers may be dropped, but the outer state's monotonic
        // source bit remains set and permanently prevents replay.
        let mut handlers = ctx.take_request_body_stream_handlers();
        for handler in handlers.iter_mut() {
            let handler = handler.get_mut();
            let verdict = handler.on_chunk(body, event).await;
            if let Some((outcome, cause)) = handler.take_pending_outcome() {
                // Record against the installing entry's dye plan, mirroring
                // the request-stage fail_mode choke point. At body phase only
                // response-direction dye rules can still take effect: the
                // upstream request headers are already committed, so
                // request-direction rule writes are inert here (and a
                // response-direction append is lost if the upstream response
                // header was also already committed).
                ctx.install_current_dye_plan(handler.dye_plan());
                PingoraSessionAdapter::new(session, ctx).record_outcome(outcome, &cause);
            }
            match verdict {
                ChunkVerdict::Continue => {}
                ChunkVerdict::Terminate {
                    status,
                    grpc_status,
                    body: reply,
                    header_ops,
                } => {
                    pg_request_body_filter::abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
                    if request_body_should_write_terminate_reply(session.response_head_selected_by_pipeline()) {
                        // Contract obligation: finish the downstream response
                        // before returning Terminate — the proxy never writes one.
                        write_terminate_reply(session, ctx, status, grpc_status, reply, header_ops).await;
                    }
                    drain_handler_logs(&mut handlers, ctx);
                    ctx.restore_request_body_stream_handlers(handlers);
                    return Ok(RequestBodyAction::Terminate);
                }
            }
        }
        if event.is_terminal() {
            drain_handler_logs(&mut handlers, ctx);
        }
        ctx.restore_request_body_stream_handlers(handlers);
    }

    // Native WAF observes the post-mutation bytes that are about to be sent
    // upstream. A terminating body handler has already returned above, so an
    // unforwarded chunk is never charged as inspected by the WAF.
    if let Some(verdict) = run_waf_body_stream(body.as_deref().unwrap_or_default(), event, ctx) {
        if let ChunkVerdict::Terminate {
            status,
            grpc_status,
            body: reply,
            header_ops,
        } = verdict
        {
            pg_request_body_filter::abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
            if request_body_should_write_terminate_reply(session.response_head_selected_by_pipeline()) {
                write_terminate_reply(session, ctx, status, grpc_status, reply, header_ops).await;
            }
            return Ok(RequestBodyAction::Terminate);
        }
    }

    // Observation sees the exact post-mutation bytes about to reach the
    // mirror tee and primary upstream. It is synchronous, read-only and
    // failure-open, so it cannot change the body action or backpressure path.
    // Retry-visible body processors use Pingora's canonical request-attempt
    // identity. `try_cnt` remains Edgion's backend-selection metric and can
    // diverge when product-specific sub-attempts are introduced.
    let attempt = session.request_attempt_id().map_or(0, |attempt| attempt.get());
    if let Some(observers) = ctx.request_body_observers_mut() {
        observers.observe(body.as_deref().unwrap_or_default(), event, attempt);
    }

    pg_request_body_filter::request_body_filter(edgion_http, session, body, event, ctx).await?;
    Ok(RequestBodyAction::Continue)
}

/// A selected pipeline response owns downstream delivery, so a terminal WAF or
/// handler verdict must not attempt to splice in a local reply. `Terminate`
/// still stops the request side; Pingora either drains a qualified complete
/// response or explicitly aborts an incomplete one.
fn request_body_should_write_terminate_reply(response_selected_by_pipeline: bool) -> bool {
    !response_selected_by_pipeline
}

fn run_waf_body_stream(chunk: &[u8], event: RequestBodyEvent, ctx: &mut EdgionHttpContext) -> Option<ChunkVerdict> {
    if !ctx.waf.as_deref().is_some_and(|waf| waf.request_body_streaming()) {
        return None;
    }
    if event == RequestBodyEvent::Abandoned {
        let waf = ctx.waf.as_deref_mut()?;
        waf.abandon_request_body_stream();
        let elapsed_micros = waf.take_request_body_execution_micros();
        crate::observe::metrics::record_waf_execution_micros("request_body", elapsed_micros);
        ctx.record_selected_waf_terminal_error(WafLogPhase::RequestBody, WafErrorCode::BodyStreamAbandoned, 400);
        return Some(ChunkVerdict::Terminate {
            status: 400,
            grpc_status: None,
            body: None,
            header_ops: Vec::new(),
        });
    }

    let started = Instant::now();
    let result = ctx
        .waf
        .as_deref_mut()?
        .execute_request_body_stream_chunk(chunk, event.is_complete());
    if result.is_ok() {
        crate::observe::metrics::record_waf_body_bytes_inspected("request", chunk.len());
    }
    let elapsed_micros =
        u64::try_from(Instant::now().saturating_duration_since(started).as_micros()).unwrap_or(u64::MAX);
    let waf = ctx.waf.as_deref_mut()?;
    waf.add_request_body_execution_micros(elapsed_micros);
    if matches!(&result, Ok(None)) {
        return None;
    }
    let total_micros = waf.take_request_body_execution_micros();
    crate::observe::metrics::record_waf_execution_micros("request_body", total_micros);
    match result {
        Ok(None) => unreachable!("non-terminal streamed WAF result returned above"),
        Ok(Some(outcome)) => {
            let intervention = outcome.intervention().map(|intervention| intervention.status());
            ctx.record_selected_waf_phase_outcome(WafLogPhase::RequestBody, total_micros, &outcome);
            intervention.map_or(Some(ChunkVerdict::Continue), |status| {
                Some(ChunkVerdict::Terminate {
                    status,
                    grpc_status: None,
                    body: None,
                    header_ops: Vec::new(),
                })
            })
        }
        Err(error) => {
            let status = crate::plugins::http::waf::WafPlugin::runtime_error_status(&error);
            ctx.record_selected_waf_terminal_error(
                WafLogPhase::RequestBody,
                crate::plugins::http::waf::WafPlugin::runtime_error_code(&error),
                status,
            );
            Some(ChunkVerdict::Terminate {
                status,
                grpc_status: None,
                body: None,
                header_ops: Vec::new(),
            })
        }
    }
}

/// Write a handler-authored local reply, mirroring the request-stage
/// `ErrResponse` writer: the gRPC-native error path on gRPC routes, and a
/// framing-corrected plain reply elsewhere. A write failure is not
/// propagated: the terminate path proceeds either way and the transport-side
/// hygiene is the proxy's job.
async fn write_terminate_reply(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    status: u16,
    exact_grpc_status: Option<u16>,
    reply: Option<Bytes>,
    header_ops: Vec<crate::PendingHeaderOp>,
) {
    // Contract obligation: when a FINAL response is already committed
    // downstream (an early upstream response raced the upload), observe it
    // and abandon — writing here would splice the terminate body into the
    // committed, possibly unfinished response. A relayed interim 1xx
    // (100-continue, 103 Early Hints) is NOT a committed response: the
    // transport permits (and expects) a final header after an informational
    // one; only 101 is terminal.
    if session
        .response_written()
        .is_some_and(|r| !r.status.is_informational() || r.status.as_u16() == 101)
    {
        return;
    }

    let is_grpc = is_canonical_grpc(ctx);
    let mut adapter = PingoraSessionAdapter::new(session, ctx);
    // Handler-authored reply headers (already protected-filtered) queue as
    // pending ops; both writer paths below drain them into the reply.
    for op in header_ops {
        let _ = adapter.queue_local_reply_header_op(op);
    }
    let written = if is_grpc {
        // gRPC route: every terminate status — including a processor's 200 —
        // must carry grpc-status trailers; a bare HTTP reply is protocol-
        // invalid for gRPC clients. Same mapping as the request-stage
        // `ErrResponse` choke point.
        let (grpc_status, message, invalid_utf8) = terminate_grpc_reply(status, exact_grpc_status, reply.as_deref());
        if invalid_utf8 {
            adapter.note_grpc_message_body_not_utf8();
        }
        adapter.send_grpc_error_direct(grpc_status, message).await.is_ok()
    } else if let Ok(status_code) = http::StatusCode::from_u16(status) {
        match ResponseHeader::build(status_code, Some(2)) {
            Ok(mut header) => {
                let _ = header.insert_header("content-type", "text/plain");
                let body = reply;
                let len = body.as_ref().map(|b| b.len()).unwrap_or(0);
                // Re-assert framing after draining queued ops, matching the
                // request-stage writer: a queued Content-Length must not
                // desync the wire from the actual reply length.
                adapter.drain_pending_response_header_ops(&mut header);
                let _ = header.remove_header(&http::header::TRANSFER_ENCODING);
                let _ = header.insert_header("content-length", len.to_string());
                let mut ok = adapter
                    .write_response_header(Box::new(header), body.is_none())
                    .await
                    .is_ok();
                if ok {
                    if let Some(bytes) = body {
                        ok = adapter.write_response_body(Some(bytes), true).await.is_ok();
                    }
                }
                ok
            }
            Err(_) => false,
        }
    } else {
        false
    };
    if written {
        ctx.mark_local_reply_written();
    }
}

/// Borrow text only at the gRPC trailer boundary. Exact statuses preserve an
/// absent message; HTTP-mapped statuses use the canonical default instead.
fn terminate_grpc_reply(
    status: u16,
    exact_grpc_status: Option<u16>,
    reply: Option<&[u8]>,
) -> (u16, Option<&str>, bool) {
    let (mapped_status, default_msg) = crate::runtime::server::http_to_grpc_status(status);
    let message = reply.and_then(|body| std::str::from_utf8(body).ok());
    let invalid_utf8 = reply.is_some() && message.is_none();
    (
        exact_grpc_status.unwrap_or(mapped_status),
        message.or_else(|| exact_grpc_status.is_none().then_some(default_msg)),
        invalid_utf8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::RequestDiagnostic;
    use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
    use edgion_resources::EdgionErrorCode;
    use edgion_waf::seclang::{RequestHead, RuleEngineMode, RuntimeLimits, Transaction};
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
            rules: "SecRule REQUEST_BODY \"@pm blocked\" \"id:1,phase:2,deny,status:409\"\n".to_string(),
            mode: WafMode::On,
            priority: 0,
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
        let mut transaction = Transaction::new_shared_with_modes(
            generation,
            request,
            RuntimeLimits::default(),
            edgion_waf::seclang::EvaluationMode::Accelerated,
            RuleEngineMode::On,
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
        assert!(waf.install(Arc::clone(&owner), transaction));
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
}

/// Push every pending handler log under the `request_body_filters` stage.
/// Called on terminate, on EOS, and as a leftover sweep from `pg_logging`
/// (client-abort / futile-read paths).
fn drain_handler_logs(handlers: &mut Handlers, ctx: &mut EdgionHttpContext) {
    let filters: Vec<_> = handlers.iter_mut().filter_map(|h| h.get_mut().take_log()).collect();
    if !filters.is_empty() {
        ctx.push_stage_log(StageLogs {
            stage: "request_body_filters",
            filters,
            edgion_plugins: Vec::new(),
        });
    }
}

/// Leftover-log sweep used by the logging phase for requests whose body pump
/// never reached EOS (client abort, early upstream response).
pub fn drain_leftover_stream_logs(ctx: &mut EdgionHttpContext) {
    if !ctx.has_request_body_stream_handlers() {
        return;
    }
    let mut handlers = ctx.take_request_body_stream_handlers();
    drain_handler_logs(&mut handlers, ctx);
    ctx.restore_request_body_stream_handlers(handlers);
}

/// Idempotent observer cleanup/log sweep for EOS, cancellation, early local
/// replies, and upstream aborts. Observer failures never affect the request.
pub fn drain_request_body_observer_logs(ctx: &mut EdgionHttpContext) {
    let Some(observers) = ctx.request_body_observers_mut() else {
        return;
    };
    let filters = observers.finalize_incomplete_and_take_logs();
    if !filters.is_empty() {
        ctx.push_stage_log(StageLogs {
            stage: "request_body_observers",
            filters,
            edgion_plugins: Vec::new(),
        });
    }
}
