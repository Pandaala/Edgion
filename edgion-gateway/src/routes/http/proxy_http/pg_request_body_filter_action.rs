//! Per-chunk request-body hook: drives installed streamed handlers, then the
//! request-mirror tee.
//!
//! Native byte admission precedes the nullable-state test shared by streamed
//! handlers, native WAF, observers, and the mirror tee.
//!
//! Handler chunks are post-mutation for the mirror: the tee runs after the
//! handlers so the mirror backend receives exactly what the primary upstream
//! receives.

use super::{pg_request_body_filter, EdgionHttpProxy};
use crate::ctx::{MirrorAbandonReason, WafErrorCode, WafLogPhase};
use crate::plugins::runtime::{ChunkVerdict, PingoraSessionAdapter, PluginSession, StageLogs};
use crate::EdgionHttpContext;
use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use pingora_http::ResponseHeader;
use pingora_proxy::{RequestBodyAction, RequestBodyEvent, Session};
use std::time::Instant;

type Handlers = crate::ctx::RequestBodyStreamHandlers;

#[inline]
fn is_canonical_grpc(ctx: &EdgionHttpContext) -> bool {
    ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched
}

fn request_body_exceeds_hard_limit(session: &Session, ctx: &EdgionHttpContext) -> bool {
    (ctx.enforce_request_body_limit || session.request_body_prefix_active() || ctx.waf_body_hard_limit().is_some())
        && session.body_bytes_read() as u64 > ctx.request_body_hard_limit()
        && !session.was_upgraded()
}

pub async fn request_body_filter_action(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<Bytes>,
    event: RequestBodyEvent,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    // Native transport counts original bytes exactly once, including captured
    // bytes; replay and streaming mutations cannot inflate this admission count.
    if request_body_exceeds_hard_limit(session, ctx) {
        return reject_request_body_limit(session, ctx).await;
    }
    // Every per-chunk consumer activates the same optional state at install
    // time. Requests without a handler, WAF stream, observer, or mirror pay one
    // nullable-pointer branch and no allocation.
    if !ctx.has_request_body_plugins() {
        return Ok(RequestBodyAction::Continue);
    }

    if event != RequestBodyEvent::Abandoned
        && ctx.request_body_handlers_selected()
        && !ctx.admit_streamed_request_body_bytes(body.as_ref().map_or(0, Bytes::len), false)
    {
        return reject_streamed_request(session, ctx, 413, EdgionErrorCode::BodyReadTooLarge).await;
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
                    abandon_handlers(&mut handlers).await;
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

    if event != RequestBodyEvent::Abandoned
        && ctx.request_body_handlers_selected()
        && !ctx.admit_streamed_request_body_bytes(body.as_ref().map_or(0, Bytes::len), true)
    {
        return reject_streamed_request(session, ctx, 413, EdgionErrorCode::BodyReadTooLarge).await;
    }

    // Native WAF observes the post-mutation bytes that are about to be sent
    // upstream. A terminating body handler has already returned above, so an
    // unforwarded chunk is never charged as inspected by the WAF.
    if let Some(ChunkVerdict::Terminate {
        status,
        grpc_status,
        body: reply,
        header_ops,
    }) = run_waf_body_stream(body.as_deref().unwrap_or_default(), event, ctx)
    {
        pg_request_body_filter::abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
        if request_body_should_write_terminate_reply(session.response_head_selected_by_pipeline()) {
            write_terminate_reply(session, ctx, status, grpc_status, reply, header_ops).await;
        }
        return Ok(RequestBodyAction::Terminate);
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

/// Pingora calls this only for actual trailers, before a synthetic clean EOF.
pub async fn request_trailer_filter(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    if !ctx.request_body_handlers_selected() {
        return Ok(RequestBodyAction::Continue);
    }
    reject_streamed_request(session, ctx, 400, EdgionErrorCode::BodyStreamTrailersUnsupported).await
}

async fn abandon_handlers(handlers: &mut Handlers) {
    for handler in handlers.iter_mut() {
        // Abandonment releases incomplete exchanges without sending protocol EOS.
        // Completed handlers preserve their already-selected terminal outcome.
        let _ = handler.get_mut().on_chunk(&mut None, RequestBodyEvent::Abandoned).await;
    }
}

/// Shared early and per-chunk hard admission, with one response owner.
pub(super) async fn reject_request_body_limit(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    ctx.record_selected_waf_terminal_error(WafLogPhase::RequestBody, WafErrorCode::BodyLimit, 413);
    reject_streamed_request(session, ctx, 413, EdgionErrorCode::BodyReadTooLarge).await
}

async fn reject_streamed_request(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    status: u16,
    code: EdgionErrorCode,
) -> pingora_core::Result<RequestBodyAction> {
    ctx.record_error(code);
    session.set_keepalive(None);
    let mut handlers = ctx.take_request_body_stream_handlers();
    abandon_handlers(&mut handlers).await;
    drain_handler_logs(&mut handlers, ctx);
    ctx.restore_request_body_stream_handlers(handlers);
    pg_request_body_filter::abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
    if request_body_should_write_terminate_reply(session.response_head_selected_by_pipeline()) {
        write_terminate_reply(session, ctx, status, None, None, Vec::new()).await;
    }
    Ok(RequestBodyAction::Terminate)
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
    let (status, reply) = if edgion_resources::http_status::is_valid_final_http_status(status) {
        (status, reply)
    } else {
        (500, None)
    };
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

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_body_filter_action_test.rs"]
mod tests;
