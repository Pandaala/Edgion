// Raw HTTP/2 (h2c prior-knowledge) transport backend
// (`--transport-h2-port`, default 30031).
//
// Built on the `h2` crate rather than hand-written HPACK: `h2::server` already
// gives frame-level control — RST_STREAM with any code at any point in the
// request body, GOAWAY, trailer observation, and (the single most important
// fact this backend exists to report) whether HEADERS carried END_STREAM,
// read before any DATA is polled.
//
// The precedent for a raw `h2::server::handshake` listener on its own port is
// `start_h2_trailer_malformed_server`; this one is parameterised instead of
// hardcoding one misbehaviour.

use anyhow::Result;
use bytes::Bytes;
use h2::server as h2s;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info};

use super::recorder::{self, TransportEvent, LISTENER_H2};
use super::script::{AfterResponse, Behavior, UpScript};

/// How long a `hang` script parks before releasing the stream.
const HANG_PARK: Duration = Duration::from_secs(120);

/// Upper bound on [`StreamHandles::wait_for_flush`].
///
/// The barrier is a round trip through the connection loop, so it normally
/// completes in microseconds. The bound exists only so a stream whose connection
/// has already gone cannot park forever; a lapsed barrier is reported as such
/// rather than being mistaken for a flush.
const FLUSH_BARRIER_TIMEOUT: Duration = Duration::from_secs(2);

/// Canonical HTTP/2 error-code names, in both directions: `x-up-reset-code`
/// parses through this table, and every recorded `code` renders through it, so
/// a case asserts on a name rather than on a `Debug` rendering.
const REASONS: &[(&str, h2::Reason)] = &[
    ("NO_ERROR", h2::Reason::NO_ERROR),
    ("PROTOCOL_ERROR", h2::Reason::PROTOCOL_ERROR),
    ("INTERNAL_ERROR", h2::Reason::INTERNAL_ERROR),
    ("FLOW_CONTROL_ERROR", h2::Reason::FLOW_CONTROL_ERROR),
    ("SETTINGS_TIMEOUT", h2::Reason::SETTINGS_TIMEOUT),
    ("STREAM_CLOSED", h2::Reason::STREAM_CLOSED),
    ("FRAME_SIZE_ERROR", h2::Reason::FRAME_SIZE_ERROR),
    ("REFUSED_STREAM", h2::Reason::REFUSED_STREAM),
    ("CANCEL", h2::Reason::CANCEL),
    ("COMPRESSION_ERROR", h2::Reason::COMPRESSION_ERROR),
    ("CONNECT_ERROR", h2::Reason::CONNECT_ERROR),
    ("ENHANCE_YOUR_CALM", h2::Reason::ENHANCE_YOUR_CALM),
    ("INADEQUATE_SECURITY", h2::Reason::INADEQUATE_SECURITY),
    ("HTTP_1_1_REQUIRED", h2::Reason::HTTP_1_1_REQUIRED),
];

/// Parse an `x-up-reset-code` value. An unknown name falls back to
/// `INTERNAL_ERROR`; the script's `warning` field is not the right place for
/// this because the code is free-form by design, so the fallback is recorded in
/// the event `detail` instead.
pub fn parse_reason(name: &str) -> h2::Reason {
    let want = name.trim().to_ascii_uppercase().replace('-', "_");
    REASONS
        .iter()
        .find(|(n, _)| *n == want)
        .map(|(_, r)| *r)
        .unwrap_or(h2::Reason::INTERNAL_ERROR)
}

/// Render an `h2::Reason` as its canonical name.
pub fn reason_name(reason: h2::Reason) -> String {
    REASONS
        .iter()
        .find(|(_, r)| *r == reason)
        .map(|(n, _)| (*n).to_string())
        .unwrap_or_else(|| format!("UNKNOWN({})", u32::from(reason)))
}

pub async fn start_transport_h2_server(port: u16) -> Result<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("✓ Transport H2 backend listening on 0.0.0.0:{}", port);

    loop {
        match listener.accept().await {
            Ok((tcp, peer)) => {
                let _ = tcp.set_nodelay(true);
                let conn = recorder::claim_conn(LISTENER_H2);
                debug!("transport-h2: accepted conn {} from {}", conn, peer);
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(tcp, conn).await {
                        debug!("transport-h2: conn {} finished: {}", conn, e);
                    }
                    recorder::mark_closed(LISTENER_H2, conn);
                });
            }
            Err(e) => error!("transport-h2: accept error: {}", e),
        }
    }
}

/// What one turn of the connection loop produced.
type Accepted = Option<Result<(http::Request<h2::RecvStream>, h2s::SendResponse<Bytes>), h2::Error>>;

enum ConnStep {
    /// Boxed only to keep the variants a similar size — an accepted stream
    /// carries a whole `Request` and a `SendResponse`.
    Accepted(Box<Accepted>),
    Goaway(h2::Reason),
    /// A stream task asked to be told when the connection had been driven —
    /// see [`StreamHandles::wait_for_flush`].
    Flushed(oneshot::Sender<()>),
}

/// A stream task's handles on the connection loop.
#[derive(Clone)]
struct StreamHandles {
    goaway: mpsc::Sender<h2::Reason>,
    flush: mpsc::Sender<oneshot::Sender<()>>,
}

impl StreamHandles {
    /// Park until everything this stream has queued is on the wire, and report
    /// whether that is what happened.
    ///
    /// This exists because `h2` has no flush API. `send_response` and
    /// `send_data` only *queue* frames on the stream; they are moved into the
    /// codec and written by `Streams::poll_complete`, which runs in the
    /// connection task. Until that has happened the frames are still cancellable
    /// — `Send::send_reset` opens by calling `Prioritize::clear_queue`, which
    /// drops every frame still queued — so a reset issued too early destroys the
    /// very response it was supposed to follow.
    ///
    /// The barrier is a round trip through the connection loop rather than a
    /// sleep, and that is what makes it a guarantee instead of a hope: the loop
    /// polls `poll_accept` **first** on every turn, and only serves this request
    /// on a turn where `poll_accept` answered `Pending` — which is exactly the
    /// path on which `Connection::poll` runs `Streams::poll_complete` and
    /// flushes the codec. It is also the *fast* barrier, which matters: a real
    /// gateway answers a complete response by cancelling the upload it no longer
    /// needs, measured here at ~2 ms, and a timer-based barrier loses that race.
    async fn wait_for_flush(&self) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.flush.send(tx).await.is_err() {
            return false;
        }
        matches!(tokio::time::timeout(FLUSH_BARRIER_TIMEOUT, rx).await, Ok(Ok(())))
    }
}

async fn handle_conn(tcp: TcpStream, conn: u32) -> Result<(), String> {
    let mut h2conn = h2s::handshake(tcp).await.map_err(|e| format!("h2 handshake: {e}"))?;
    // Streams are served in their own tasks so that resetting one stream never
    // tears the connection down — a case asserting "the pooled H2 connection
    // stayed usable" needs the backend not to close it.
    let (goaway_tx, mut goaway_rx) = mpsc::channel::<h2::Reason>(8);
    let (flush_tx, mut flush_rx) = mpsc::channel::<oneshot::Sender<()>>(8);
    let handles = StreamHandles {
        goaway: goaway_tx,
        flush: flush_tx,
    };
    let mut streams = Vec::new();

    loop {
        // Written as a `poll_fn` rather than a `select!` on purpose. `select!`
        // polls its branches in a random order, so it cannot promise that the
        // connection was driven *before* a stream task is told its frames are
        // out. Here `poll_accept` is polled first, unconditionally, and the
        // other branches are only reached once it has answered `Pending` — the
        // path on which `Connection::poll` runs `Streams::poll_complete` and
        // flushes the codec.
        let step = std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(accepted) = h2conn.poll_accept(cx) {
                return std::task::Poll::Ready(ConnStep::Accepted(Box::new(accepted)));
            }
            if let std::task::Poll::Ready(Some(reason)) = goaway_rx.poll_recv(cx) {
                return std::task::Poll::Ready(ConnStep::Goaway(reason));
            }
            if let std::task::Poll::Ready(Some(ack)) = flush_rx.poll_recv(cx) {
                return std::task::Poll::Ready(ConnStep::Flushed(ack));
            }
            std::task::Poll::Pending
        })
        .await;

        match step {
            ConnStep::Flushed(ack) => {
                let _ = ack.send(());
            }
            ConnStep::Goaway(reason) => {
                recorder::record(
                    TransportEvent::new(LISTENER_H2, conn, 0, recorder::KIND_RESP_DONE)
                        .code(&reason_name(reason))
                        .detail("backend sent GOAWAY"),
                );
                h2conn.abrupt_shutdown(reason);
            }
            ConnStep::Accepted(accepted) => match *accepted {
                None => break,
                Some(Ok((req, respond))) => {
                    let handles = handles.clone();
                    streams.push(tokio::spawn(handle_stream(conn, req, respond, handles)));
                }
                Some(Err(e)) => {
                    // A connection-level error here is the gateway acting on us:
                    // GOAWAY, or a transport failure. Both are facts a case wants.
                    let kind = if e.is_go_away() {
                        recorder::KIND_PEER_GOAWAY
                    } else {
                        recorder::KIND_PEER_RESET
                    };
                    let code = e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string());
                    recorder::record(
                        TransportEvent::new(LISTENER_H2, conn, 0, kind)
                            .code(&code)
                            .detail(&e.to_string()),
                    );
                    break;
                }
            },
        }
    }

    for task in streams {
        let _ = task.await;
    }
    Ok(())
}

struct ReqCtx {
    conn: u32,
    stream: u32,
    trace: Option<String>,
}

impl ReqCtx {
    fn event(&self, kind: &str) -> TransportEvent {
        TransportEvent::new(LISTENER_H2, self.conn, self.stream, kind).trace(self.trace.as_deref())
    }
}

#[derive(Default)]
struct BodyState {
    total: usize,
    end_emitted: bool,
    trailers: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PumpStop {
    Reached,
    Ended,
    PeerReset,
}

async fn handle_stream(
    conn: u32,
    req: http::Request<h2::RecvStream>,
    mut respond: h2s::SendResponse<Bytes>,
    handles: StreamHandles,
) {
    let stream = u32::from(respond.stream_id());
    let script = UpScript::from_headers(|name| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string())
    });
    if let Some(trace) = script.trace_id.as_deref() {
        recorder::bind_trace(LISTENER_H2, conn, trace);
    }
    recorder::note_request(LISTENER_H2, conn);

    let ctx = ReqCtx {
        conn,
        stream,
        trace: script.trace_id.clone(),
    };

    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let content_length = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let transfer_encoding = req
        .headers()
        .get("transfer-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    // Read BEFORE any `data()` poll. This is the fact that distinguishes the
    // Ordinary framing (END_STREAM follows the declaration) from the Streamed
    // framing (END_STREAM on HEADERS is always false).
    let headers_eos = req.body().is_end_stream();

    recorder::record(
        ctx.event(recorder::KIND_REQ_HEADERS)
            .headers_eos(headers_eos)
            .content_length(content_length)
            .transfer_encoding(transfer_encoding)
            .method(&method)
            .path(&path)
            .behavior(script.behavior.as_str()),
    );

    let mut body = req.into_body();
    let mut state = BodyState::default();

    match script.behavior {
        Behavior::Echo | Behavior::Malformed(_) => {
            if pump_body(&mut body, &ctx, &mut state, None).await == PumpStop::PeerReset {
                return;
            }
            respond_h2(
                &mut respond,
                &mut body,
                &ctx,
                &script,
                &mut state,
                headers_eos,
                &handles,
            )
            .await;
        }
        Behavior::EarlyResponse => {
            let stop = pump_body(&mut body, &ctx, &mut state, Some(script.at_bytes)).await;
            if stop == PumpStop::PeerReset {
                return;
            }
            respond_h2(
                &mut respond,
                &mut body,
                &ctx,
                &script,
                &mut state,
                headers_eos,
                &handles,
            )
            .await;
            if script.drain && stop == PumpStop::Reached {
                let _ = pump_body(&mut body, &ctx, &mut state, None).await;
            }
        }
        Behavior::Hang => {
            if script.drain {
                let _ = pump_body(&mut body, &ctx, &mut state, None).await;
            }
            park_until_peer_resets(&mut respond, &ctx).await;
        }
        Behavior::HalfClose => {
            let _ = pump_body(&mut body, &ctx, &mut state, Some(script.at_bytes)).await;
            respond_h2(
                &mut respond,
                &mut body,
                &ctx,
                &script,
                &mut state,
                headers_eos,
                &handles,
            )
            .await;
            // Drop the receive stream while the request body is still open: the
            // H2 analogue of a half-closed connection.
            drop(body);
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .detail("half-close: receive stream dropped with the request body open"),
            );
        }
        Behavior::Reset => {
            let _ = pump_body(&mut body, &ctx, &mut state, Some(script.at_bytes)).await;
            let reason = parse_reason(&script.reset_code);
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .code(&reason_name(reason))
                    .detail(&format!(
                        "reset: RST_STREAM at {} request bytes (requested code {})",
                        state.total, script.reset_code
                    )),
            );
            respond.send_reset(reason);
        }
        Behavior::Close => {
            let _ = pump_body(&mut body, &ctx, &mut state, Some(script.at_bytes)).await;
            // HTTP/2 has no per-stream connection close; GOAWAY is the closest
            // analogue of "the upstream closed the connection".
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .detail("close: HTTP/2 has no connection close per stream, sending GOAWAY(NO_ERROR)"),
            );
            let _ = handles.goaway.send(h2::Reason::NO_ERROR).await;
        }
    }
}

async fn pump_body(body: &mut h2::RecvStream, ctx: &ReqCtx, state: &mut BodyState, target: Option<usize>) -> PumpStop {
    loop {
        if let Some(target) = target {
            if state.total >= target {
                return PumpStop::Reached;
            }
        }
        if state.end_emitted {
            return PumpStop::Ended;
        }
        match body.data().await {
            Some(Ok(chunk)) => {
                let len = chunk.len();
                let _ = body.flow_control().release_capacity(len);
                state.total += len;
                let end = body.is_end_stream();
                if end {
                    state.end_emitted = true;
                }
                recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(len).end_stream(end));
            }
            Some(Err(e)) => {
                // The gateway reset the stream. This is the whole reason
                // `PeerReset` exists in the vocabulary.
                let code = e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string());
                recorder::record(
                    ctx.event(recorder::KIND_PEER_RESET)
                        .code(&code)
                        .len(state.total)
                        .detail(&e.to_string()),
                );
                return PumpStop::PeerReset;
            }
            None => {
                match body.trailers().await {
                    Ok(Some(map)) if !map.is_empty() => {
                        state.trailers = true;
                        let names = map.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(",");
                        recorder::record(ctx.event(recorder::KIND_REQ_TRAILERS).len(map.len()).detail(&names));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let code = e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string());
                        recorder::record(
                            ctx.event(recorder::KIND_PEER_RESET)
                                .code(&code)
                                .len(state.total)
                                .detail(&format!("while polling trailers: {e}")),
                        );
                        return PumpStop::PeerReset;
                    }
                }
                if !state.end_emitted {
                    state.end_emitted = true;
                    recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(0).end_stream(true));
                }
                return PumpStop::Ended;
            }
        }
    }
}

/// Consume whatever request-body data has already arrived, **without waiting**,
/// and report whether the peer has ended the request stream.
///
/// The non-blocking drain is what makes that answer trustworthy.
/// `RecvStream::is_end_stream` is `recv_end_stream && pending_recv.is_empty()`,
/// so a stream the peer already ended still answers `false` while unread chunks
/// sit in the buffer — while `h2`'s own `Send::send_reset` looks at the stream
/// *state*, which is already `Closed`. Emptying the buffer first removes the
/// disagreement, and since every chunk it consumes is recorded as `ReqData`,
/// nothing is hidden by doing so.
fn drain_ready(body: &mut h2::RecvStream, ctx: &ReqCtx, state: &mut BodyState) -> bool {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        if state.end_emitted {
            return true;
        }
        match body.poll_data(&mut cx) {
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                let len = chunk.len();
                let _ = body.flow_control().release_capacity(len);
                state.total += len;
                let end = body.is_end_stream();
                if end {
                    state.end_emitted = true;
                }
                recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(len).end_stream(end));
            }
            std::task::Poll::Ready(Some(Err(e))) => {
                // The peer reset or the connection failed: either way the
                // receive half is closed, so `h2` will not emit our reset.
                let code = e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string());
                recorder::record(
                    ctx.event(recorder::KIND_PEER_RESET)
                        .code(&code)
                        .len(state.total)
                        .detail(&format!("while draining before the post-response reset: {e}")),
                );
                return true;
            }
            std::task::Poll::Ready(None) => {
                state.end_emitted = true;
                recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(0).end_stream(true));
                return true;
            }
            // Nothing buffered and the peer has not ended: the receive half is
            // genuinely still open.
            std::task::Poll::Pending => return false,
        }
    }
}

async fn respond_h2(
    respond: &mut h2s::SendResponse<Bytes>,
    recv: &mut h2::RecvStream,
    ctx: &ReqCtx,
    script: &UpScript,
    state: &mut BodyState,
    headers_eos: bool,
    handles: &StreamHandles,
) {
    if script.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(script.delay_ms)).await;
    }

    let body = script.response_body();
    let mut detail = format!("after-response={}", script.after_response.as_str());
    if let Behavior::Malformed(kind) = script.behavior {
        // Deliberately malformed framing is an HTTP/1 capability: HTTP/2 has no
        // chunked framing, and the `h2` crate refuses to put a self-contradictory
        // message on the wire. Degrade visibly rather than silently.
        detail.push_str(&format!("; {} is HTTP/1-only, served as echo on HTTP/2", kind.as_str()));
    }
    if let Some(warning) = script.warning.as_deref() {
        detail.push_str("; ");
        detail.push_str(warning);
    }

    let mut builder = http::Response::builder().status(script.status);
    for (name, value) in framing_report(script, state, ctx, headers_eos) {
        builder = builder.header(name, value);
    }
    let response = match builder.body(()) {
        Ok(r) => r,
        Err(e) => {
            recorder::record(
                ctx.event(recorder::KIND_RESP_HEAD)
                    .detail(&format!("build failed: {e}")),
            );
            respond.send_reset(h2::Reason::INTERNAL_ERROR);
            return;
        }
    };

    recorder::record(
        ctx.event(recorder::KIND_RESP_HEAD)
            .status(script.status)
            .framing("h2")
            .len(body.len())
            .behavior(script.behavior.as_str())
            .detail(&detail),
    );

    let mut send = match respond.send_response(response, false) {
        Ok(send) => send,
        Err(e) => {
            recorder::record(
                ctx.event(recorder::KIND_PEER_RESET)
                    .code(&e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string()))
                    .detail(&format!("send_response failed: {e}")),
            );
            return;
        }
    };
    send.reserve_capacity(body.len());
    if let Err(e) = send.send_data(Bytes::from(body.clone()), true) {
        recorder::record(
            ctx.event(recorder::KIND_PEER_RESET)
                .code(&e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string()))
                .detail(&format!("send_data failed: {e}")),
        );
        return;
    }
    recorder::record(ctx.event(recorder::KIND_RESP_DONE).len(body.len()));

    match script.after_response {
        AfterResponse::Linger => {}
        AfterResponse::Close => {
            let _ = handles.goaway.send(h2::Reason::NO_ERROR).await;
        }
        AfterResponse::Reset => {
            // A complete response, THEN RST_STREAM with the requested code.
            // With NO_ERROR this is the benign post-EOF reset the fork's §6 fix
            // exists for: it must not destroy a healthy pooled connection.
            //
            // Two `h2` facts decide whether that shape can reach the wire at
            // all, and both are handled here rather than hoped for:
            //
            //  1. `send_reset` clears the stream's pending send queue, so the
            //     response frames must be on the wire first — see
            //     `StreamHandles::wait_for_flush`.
            //  2. `send_reset` emits *nothing* on a stream that is already
            //     closed (`Send::send_reset`, `is_closed && is_empty`). The
            //     stream closes as soon as the peer's END_STREAM has been
            //     received, since this backend has just sent its own — so the
            //     reset is only expressible while the request body is still
            //     arriving, which is exactly RFC 9113 §8.1's premise.
            //
            // Neither is asserted by hope: the barrier is a round trip through
            // the connection loop, and what is *recorded* below is derived from
            // `h2`'s own state at the moment `send_reset` is called.
            let reason = parse_reason(&script.reset_code);
            let flushed = handles.wait_for_flush().await;
            let request_ended = drain_ready(recv, ctx, state);
            send.send_reset(reason);
            match (flushed, request_ended) {
                (true, false) => recorder::record(
                    ctx.event(recorder::KIND_RESP_DONE)
                        .code(&reason_name(reason))
                        .end_stream(false)
                        .detail(
                            "after-response: RST_STREAM emitted after the complete response reached the \
                             wire, with the request body still arriving",
                        ),
                ),
                // Both of the remaining shapes are runs on which no assertion
                // about a reset that was *sent* may pass, so neither records a
                // `code`. That is the whole point: the old backend recorded the
                // code before calling `send_reset`, so the log claimed a frame
                // on every run, including the runs that carried none.
                (_, true) => recorder::record(ctx.event(recorder::KIND_RESP_DONE).end_stream(true).detail(
                    "after-response: RST_STREAM was requested but h2 emitted none — the peer had already \
                     ended the request, so the stream was closed and `send_reset` returns without \
                     queueing a frame. Nothing went on the wire.",
                )),
                (false, false) => recorder::record(ctx.event(recorder::KIND_RESP_DONE).end_stream(false).detail(
                    "after-response: the flush barrier lapsed before the reset, so the response frames may \
                     have been cleared from the send queue by `send_reset`. No reset is claimed.",
                )),
            }
        }
        AfterResponse::Goaway => {
            let _ = handles.goaway.send(parse_reason(&script.reset_code)).await;
        }
    }
}

fn framing_report(script: &UpScript, state: &BodyState, ctx: &ReqCtx, headers_eos: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        ("x-up-headers-eos".to_string(), headers_eos.to_string()),
        ("x-up-req-body-len".to_string(), state.total.to_string()),
        ("x-up-req-trailers".to_string(), state.trailers.to_string()),
        ("x-up-http-version".to_string(), "2".to_string()),
        ("x-up-conn".to_string(), ctx.conn.to_string()),
        ("x-up-conn-req-seq".to_string(), ctx.stream.to_string()),
        (
            "x-up-behavior-applied".to_string(),
            script.behavior.as_str().to_string(),
        ),
    ];
    if let Some(warning) = script.warning.as_deref() {
        headers.push(("x-up-script-warning".to_string(), warning.replace(['\r', '\n'], " ")));
    }
    headers
}

/// Park without responding, but stay awake for the gateway's RST_STREAM so the
/// reset is observed rather than lost to a sleeping task.
async fn park_until_peer_resets(respond: &mut h2s::SendResponse<Bytes>, ctx: &ReqCtx) {
    let reset = std::future::poll_fn(|cx| respond.poll_reset(cx));
    tokio::select! {
        _ = tokio::time::sleep(HANG_PARK) => {
            recorder::record(ctx.event(recorder::KIND_RESP_DONE).detail("park window elapsed"));
        }
        outcome = reset => match outcome {
            Ok(reason) => recorder::record(
                ctx.event(recorder::KIND_PEER_RESET)
                    .code(&reason_name(reason))
                    .detail("peer reset the stream while the backend was parked"),
            ),
            Err(e) => recorder::record(
                ctx.event(recorder::KIND_PEER_RESET)
                    .code(&e.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string()))
                    .detail(&format!("poll_reset failed while parked: {e}")),
            ),
        },
    }
}

#[cfg(test)]
mod wire_proof;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_codes_round_trip_through_their_canonical_names() {
        for (name, reason) in REASONS {
            assert_eq!(parse_reason(name), *reason);
            assert_eq!(reason_name(*reason), *name);
        }
    }

    #[test]
    fn reset_code_parsing_is_case_and_separator_insensitive() {
        assert_eq!(parse_reason("no_error"), h2::Reason::NO_ERROR);
        assert_eq!(parse_reason("no-error"), h2::Reason::NO_ERROR);
        assert_eq!(parse_reason("  Cancel  "), h2::Reason::CANCEL);
    }

    #[test]
    fn an_unknown_reset_code_falls_back_to_internal_error() {
        assert_eq!(parse_reason("NOT_A_CODE"), h2::Reason::INTERNAL_ERROR);
    }

    #[test]
    fn the_benign_post_response_reset_is_expressible() {
        // The S3 shape: x-up-behavior: echo + x-up-after-response: reset +
        // x-up-reset-code: NO_ERROR.
        let script = UpScript::from_headers(|name| {
            Some(match name {
                "x-up-behavior" => "echo",
                "x-up-after-response" => "reset",
                "x-up-reset-code" => "NO_ERROR",
                _ => return None,
            })
            .map(|v: &str| v.to_string())
        });
        assert_eq!(script.behavior, Behavior::Echo);
        assert_eq!(script.after_response, AfterResponse::Reset);
        assert_eq!(parse_reason(&script.reset_code), h2::Reason::NO_ERROR);
    }
}
