//! Per-chunk request-body hook: drives installed streamed handlers, then the
//! request-mirror tee.
//!
//! The fast path is one emptiness test — a request with no streamed handler
//! pays a single branch before the mirror tee's own fast path.
//!
//! Handler chunks are post-mutation for the mirror: the tee runs after the
//! handlers so the mirror backend receives exactly what the primary upstream
//! receives.

use super::{pg_request_body_filter, EdgionHttpProxy};
use crate::plugins::runtime::{ChunkVerdict, PingoraSessionAdapter, PluginSession, StageLogs};
use crate::EdgionHttpContext;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::{RequestBodyAction, Session};

type Handlers = Vec<sync_wrapper::SyncWrapper<Box<dyn crate::plugins::runtime::RequestBodyStreamHandler>>>;

pub async fn request_body_filter_action(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<Bytes>,
    end_of_stream: bool,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    if !ctx.request_body_stream.is_empty() {
        // Handlers are moved out for the duration of the loop so the session
        // adapter (which borrows the whole ctx) can be built on the terminate
        // path. They are always restored: the vector must never become empty
        // once populated (retry-predicate monotonicity).
        let mut handlers = std::mem::take(&mut ctx.request_body_stream);
        for handler in handlers.iter_mut() {
            let handler = handler.get_mut();
            let verdict = handler.on_chunk(body, end_of_stream).await;
            if let Some((outcome, cause)) = handler.take_pending_outcome() {
                // Record against the installing entry's dye plan, mirroring
                // the request-stage fail_mode choke point. At body phase only
                // response-direction dye rules can still take effect: the
                // upstream request headers are already committed, so
                // request-direction rule writes are inert here (and a
                // response-direction append is lost if the upstream response
                // header was also already committed).
                ctx.current_dye_plan = handler.dye_plan();
                PingoraSessionAdapter::new(session, ctx).record_outcome(outcome, &cause);
            }
            match verdict {
                ChunkVerdict::Continue => {}
                ChunkVerdict::Terminate {
                    status,
                    body: reply,
                    headers,
                } => {
                    // Contract obligation: finish the downstream response
                    // before returning Terminate — the proxy never writes one.
                    write_terminate_reply(session, ctx, status, reply, headers).await;
                    drain_handler_logs(&mut handlers, ctx);
                    ctx.request_body_stream = handlers;
                    return Ok(RequestBodyAction::Terminate);
                }
            }
        }
        if end_of_stream {
            drain_handler_logs(&mut handlers, ctx);
        }
        ctx.request_body_stream = handlers;
    }

    pg_request_body_filter::request_body_filter(edgion_http, session, body, end_of_stream, ctx).await?;
    Ok(RequestBodyAction::Continue)
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
    reply: Option<String>,
    headers: Vec<(String, String)>,
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

    let is_grpc = ctx.request_info.is_grpc_request;
    let mut adapter = PingoraSessionAdapter::new(session, ctx);
    // Handler-authored reply headers (already protected-filtered) queue as
    // pending ops; both writer paths below drain them into the reply.
    for (name, value) in &headers {
        let _ = adapter.set_response_header(name, value);
    }
    let written = if is_grpc {
        // gRPC route: every terminate status — including a processor's 200 —
        // must carry grpc-status trailers; a bare HTTP reply is protocol-
        // invalid for gRPC clients. Same mapping as the request-stage
        // `ErrResponse` choke point.
        let (grpc_status, default_msg) = crate::runtime::server::http_to_grpc_status(status);
        let message = reply.as_deref().or(Some(default_msg));
        adapter.send_grpc_error_direct(grpc_status, message).await.is_ok()
    } else if let Ok(status_code) = http::StatusCode::from_u16(status) {
        match ResponseHeader::build(status_code, Some(2)) {
            Ok(mut header) => {
                let _ = header.insert_header("content-type", "text/plain");
                let body = reply.map(Bytes::from);
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
        ctx.local_reply_written = true;
    }
}

/// Push every pending handler log under the `request_body_filters` stage.
/// Called on terminate, on EOS, and as a leftover sweep from `pg_logging`
/// (client-abort / futile-read paths).
fn drain_handler_logs(handlers: &mut Handlers, ctx: &mut EdgionHttpContext) {
    let filters: Vec<_> = handlers.iter_mut().filter_map(|h| h.get_mut().take_log()).collect();
    if !filters.is_empty() {
        ctx.stage_logs.push(StageLogs {
            stage: "request_body_filters",
            filters,
            edgion_plugins: Vec::new(),
        });
    }
}

/// Leftover-log sweep used by the logging phase for requests whose body pump
/// never reached EOS (client abort, early upstream response).
pub fn drain_leftover_stream_logs(ctx: &mut EdgionHttpContext) {
    if ctx.request_body_stream.is_empty() {
        return;
    }
    let mut handlers = std::mem::take(&mut ctx.request_body_stream);
    drain_handler_logs(&mut handlers, ctx);
    ctx.request_body_stream = handlers;
}
