// Raw HTTP/1.1 transport backend (`--transport-h1-port`, default 30030).
//
// Deliberately NOT an axum route. `axum::serve` builds a hyper auto-negotiating
// connection, which cannot express half-close, a TCP RST at a chosen request
// byte offset, connection identity claimed at `accept()`, or a response whose
// framing is a deliberate lie. Putting that machinery on the shared listener
// would also put it on the path of the ~40 endpoints five other suites depend
// on, so it lives on its own port.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

use super::h1_wire::{
    parse_request_head, read_head, render_malformed, render_response, BodyItem, BodyPump, RenderedResponse, RequestHead,
};
use super::recorder::{self, TransportEvent, LISTENER_H1};
use super::script::{AfterResponse, Behavior, RetryFailure, UpScript};

/// How long a `hang` script parks before giving the connection back.
const HANG_PARK: Duration = Duration::from_secs(120);

pub async fn start_transport_h1_server(port: u16) -> Result<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("✓ Transport H1 backend listening on 0.0.0.0:{}", port);

    loop {
        match listener.accept().await {
            Ok((tcp, peer)) => {
                let _ = tcp.set_nodelay(true);
                // The ordinal is claimed here, not at the first request: pool
                // reuse is decided before any request is written, so a counter
                // that only moves on request arrival cannot observe it.
                let conn = recorder::claim_conn(LISTENER_H1);
                debug!("transport-h1: accepted conn {} from {}", conn, peer);
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(tcp, conn).await {
                        debug!("transport-h1: conn {} finished: {}", conn, e);
                    }
                    recorder::mark_closed(LISTENER_H1, conn);
                });
            }
            Err(e) => error!("transport-h1: accept error: {}", e),
        }
    }
}

/// What the request script wants done with the connection afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnOutcome {
    /// Keep the connection open and read the next request (HTTP/1 keep-alive).
    KeepAlive,
    /// Close gracefully (FIN).
    Close,
    /// Close abortively: `SO_LINGER(0)` then drop, which puts a real TCP RST on
    /// the wire rather than a graceful shutdown.
    ResetTcp,
}

/// Identity of the request currently being served, for the recorder.
struct ReqCtx {
    conn: u32,
    stream: u32,
    trace: Option<String>,
    trace_request_ordinal: Option<u32>,
}

impl ReqCtx {
    fn event(&self, kind: &str) -> TransportEvent {
        TransportEvent::new(LISTENER_H1, self.conn, self.stream, kind)
            .trace(self.trace.as_deref())
            .trace_request_ordinal(self.trace_request_ordinal)
    }
}

#[derive(Default)]
struct BodyState {
    total: usize,
    digest: Sha256,
    end_emitted: bool,
    trailers: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PumpStop {
    /// The `x-up-at-bytes` offset was reached; the body is still open.
    Reached,
    /// The request body ended normally.
    Ended,
    /// The peer closed or reset before the body was complete.
    PeerClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InformationalAttempt {
    MissingTrace,
    First,
    Echo,
}

fn informational_attempt(trace_request_ordinal: Option<u32>) -> InformationalAttempt {
    match trace_request_ordinal {
        None => InformationalAttempt::MissingTrace,
        Some(1) => InformationalAttempt::First,
        Some(_) => InformationalAttempt::Echo,
    }
}

async fn handle_conn(tcp: TcpStream, conn: u32) -> Result<(), String> {
    let mut io = BufReader::new(tcp);

    loop {
        // A clean FIN at a message boundary. For an upstream backend this is
        // the gateway declining to reuse the connection, which is exactly what
        // a pool-poisoning assertion looks for.
        let Some(raw) = read_head(&mut io).await? else {
            recorder::record(TransportEvent::new(
                LISTENER_H1,
                conn,
                0,
                recorder::KIND_PEER_HALF_CLOSE,
            ));
            return Ok(());
        };
        let head = parse_request_head(&raw).ok_or_else(|| "unparsable request head".to_string())?;
        let script = UpScript::from_headers(|name| head.get(name).map(|v| v.to_string()));
        let stream = recorder::note_request(LISTENER_H1, conn);
        if let Some(trace) = script.trace_id.as_deref() {
            recorder::bind_trace(LISTENER_H1, conn, trace);
        }
        let trace_request_ordinal = if matches!(
            script.behavior,
            Behavior::InformationalThenHangFirst | Behavior::RetryProbe(_)
        ) {
            script.trace_id.as_deref().map(recorder::note_trace_request)
        } else {
            None
        };
        let ctx = ReqCtx {
            conn,
            stream,
            trace: script.trace_id.clone(),
            trace_request_ordinal,
        };

        recorder::record(
            ctx.event(recorder::KIND_REQ_HEADERS)
                .headers_eos(head.headers_eos())
                .content_length(head.content_length())
                .transfer_encoding(head.transfer_encoding().map(|v| v.to_string()))
                .method(&head.method)
                .path(&head.path)
                .target(&head.path)
                .authority(head.get("host").unwrap_or(""))
                .behavior(script.behavior.as_str()),
        );

        match run_request(&mut io, &ctx, &head, &script).await? {
            ConnOutcome::KeepAlive => continue,
            ConnOutcome::Close => return Ok(()),
            ConnOutcome::ResetTcp => {
                // SO_LINGER(0) turns the imminent drop into a TCP RST rather
                // than a graceful FIN, which is the point of the `reset`
                // script on HTTP/1. tokio deprecates `set_linger` because a
                // non-zero timeout blocks the thread on drop; a zero timeout
                // discards the send buffer and closes immediately, so that
                // caveat does not apply here and there is no replacement API.
                #[allow(deprecated)]
                let _ = io.get_ref().set_linger(Some(Duration::ZERO));
                return Ok(());
            }
        }
    }
}

async fn run_request(
    io: &mut BufReader<TcpStream>,
    ctx: &ReqCtx,
    head: &RequestHead,
    script: &UpScript,
) -> Result<ConnOutcome, String> {
    let mut pump = BodyPump::new(head.framing());
    let mut state = BodyState::default();

    match script.behavior {
        Behavior::RetryProbe(failure) => {
            let ordinal = ctx.trace_request_ordinal.ok_or("retry probe requires a trace id")?;
            if matches!(failure, RetryFailure::Refused | RetryFailure::Goaway) {
                return Err("H2-only retry probe used on an H1 listener".into());
            }
            if pump_body(io, &mut pump, ctx, &mut state, None).await? == PumpStop::PeerClosed {
                return Ok(ConnOutcome::Close);
            }
            recorder::record(
                ctx.event(recorder::KIND_EFFECT_APPLIED)
                    .len(state.total)
                    .detail(&format!("{:x}", state.digest.clone().finalize())),
            );
            if !failure.fails(ordinal, script.reset_first) {
                return respond(io, ctx, head, script, &state).await;
            }
            match failure {
                RetryFailure::TcpReset => {
                    recorder::record(ctx.event(recorder::KIND_RESP_DONE).code("TCP_RST"));
                    Ok(ConnOutcome::ResetTcp)
                }
                RetryFailure::Status => {
                    let mut failed = script.clone();
                    failed.status = 503;
                    respond(io, ctx, head, &failed, &state).await
                }
                RetryFailure::Timeout => {
                    recorder::record(ctx.event(recorder::KIND_RESP_DONE).code("HANG"));
                    park_until_peer_closes(io, ctx).await;
                    Ok(ConnOutcome::Close)
                }
                RetryFailure::Refused | RetryFailure::Goaway => unreachable!("checked above"),
            }
        }
        Behavior::Echo | Behavior::Malformed(_) => {
            if pump_body(io, &mut pump, ctx, &mut state, None).await? == PumpStop::PeerClosed {
                return Ok(ConnOutcome::Close);
            }
            respond(io, ctx, head, script, &state).await
        }
        Behavior::EarlyResponse => {
            let stop = pump_body(io, &mut pump, ctx, &mut state, Some(script.at_bytes)).await?;
            if stop == PumpStop::PeerClosed {
                return Ok(ConnOutcome::Close);
            }
            let outcome = respond(io, ctx, head, script, &state).await?;
            if script.drain && stop == PumpStop::Reached {
                // Keep reading after the response so the pump on the other side
                // is not blocked by our receive window.
                let _ = pump_body(io, &mut pump, ctx, &mut state, None).await?;
            }
            Ok(outcome)
        }
        Behavior::Hang => {
            if script.drain {
                let _ = pump_body(io, &mut pump, ctx, &mut state, None).await?;
            }
            park_until_peer_closes(io, ctx).await;
            Ok(ConnOutcome::Close)
        }
        Behavior::HalfClose => {
            let _ = pump_body(io, &mut pump, ctx, &mut state, Some(script.at_bytes)).await?;
            // FIN on our write half only: the backend is done sending but keeps
            // reading, so the gateway observes a half-open connection.
            let _ = io.get_mut().shutdown().await;
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .detail("half-close: write half shut down"),
            );
            if script.drain {
                let _ = pump_body(io, &mut pump, ctx, &mut state, None).await?;
            }
            park_until_peer_closes(io, ctx).await;
            Ok(ConnOutcome::Close)
        }
        Behavior::Reset => {
            let _ = pump_body(io, &mut pump, ctx, &mut state, Some(script.at_bytes)).await?;
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .code("TCP_RST")
                    .detail("reset: SO_LINGER(0) + close"),
            );
            Ok(ConnOutcome::ResetTcp)
        }
        Behavior::Close => {
            let _ = pump_body(io, &mut pump, ctx, &mut state, Some(script.at_bytes)).await?;
            recorder::record(ctx.event(recorder::KIND_RESP_DONE).detail("close: no response written"));
            Ok(ConnOutcome::Close)
        }
        Behavior::GrpcSayHello => {
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .detail("grpc-say-hello is available only on the H2 listener"),
            );
            Ok(ConnOutcome::Close)
        }
        Behavior::InformationalThenHangFirst => {
            if pump_body(io, &mut pump, ctx, &mut state, None).await? == PumpStop::PeerClosed {
                return Ok(ConnOutcome::Close);
            }
            match informational_attempt(ctx.trace_request_ordinal) {
                InformationalAttempt::MissingTrace => {
                    recorder::record(
                        ctx.event(recorder::KIND_RESP_DONE)
                            .detail("informational-then-hang-first requires a nonempty x-trace-id"),
                    );
                    Ok(ConnOutcome::Close)
                }
                InformationalAttempt::First => {
                    recorder::record(
                        ctx.event(recorder::KIND_RESP_HEAD)
                            .status(100)
                            .framing("informational;no-body")
                            .behavior(script.behavior.as_str())
                            .detail("first trace request; hanging after flushed informational head"),
                    );
                    io.get_mut()
                        .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                        .await
                        .map_err(|e| format!("write informational response: {e}"))?;
                    io.get_mut()
                        .flush()
                        .await
                        .map_err(|e| format!("flush informational response: {e}"))?;
                    park_until_peer_closes(io, ctx).await;
                    Ok(ConnOutcome::Close)
                }
                InformationalAttempt::Echo => respond(io, ctx, head, script, &state).await,
            }
        }
    }
}

/// Read request-body bytes, recording one `ReqData` per read.
///
/// Exactly one `ReqData` per completed request body carries `end_stream: true`,
/// including a body that was empty by declaration — so a case can assert "the
/// whole body reached the upstream" with a single predicate regardless of
/// framing.
async fn pump_body(
    io: &mut BufReader<TcpStream>,
    pump: &mut BodyPump,
    ctx: &ReqCtx,
    state: &mut BodyState,
    target: Option<usize>,
) -> Result<PumpStop, String> {
    loop {
        if let Some(target) = target {
            if state.total >= target {
                return Ok(PumpStop::Reached);
            }
        }
        if state.end_emitted {
            return Ok(PumpStop::Ended);
        }
        match pump.next(io).await? {
            BodyItem::Data(data) => {
                state.total += data.len();
                state.digest.update(&data);
                let end = pump.is_finished();
                if end {
                    state.end_emitted = true;
                }
                recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(data.len()).end_stream(end));
            }
            BodyItem::Trailers(fields) => {
                state.trailers = true;
                recorder::record(
                    ctx.event(recorder::KIND_REQ_TRAILERS)
                        .len(fields.len())
                        .detail(&render_trailer_names(&fields)),
                );
            }
            BodyItem::End => {
                if !state.end_emitted {
                    state.end_emitted = true;
                    recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(0).end_stream(true));
                }
                return Ok(PumpStop::Ended);
            }
            BodyItem::PeerClosed => {
                recorder::record(
                    ctx.event(recorder::KIND_PEER_HALF_CLOSE)
                        .len(state.total)
                        .detail("peer closed before the request body was complete"),
                );
                return Ok(PumpStop::PeerClosed);
            }
        }
    }
}

fn render_trailer_names(fields: &[(String, String)]) -> String {
    fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(",")
}

/// Build and write the response, then apply `x-up-after-response`.
async fn respond(
    io: &mut BufReader<TcpStream>,
    ctx: &ReqCtx,
    head: &RequestHead,
    script: &UpScript,
    state: &BodyState,
) -> Result<ConnOutcome, String> {
    if script.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(script.delay_ms)).await;
    }

    let headers = framing_report(head, script, state, ctx);
    let body = script.response_body();
    let after = script.after_response;
    let keep_alive = after == AfterResponse::Linger;

    let rendered: RenderedResponse = match script.behavior {
        Behavior::Malformed(kind) => render_malformed(kind, script.status, &headers, &body),
        _ => render_response(script.status, &headers, &body, keep_alive),
    };

    let mut detail = format!("after-response={}", after.as_str());
    if after == AfterResponse::Goaway {
        // HTTP/1 has no GOAWAY frame; degrade rather than pretend.
        detail.push_str("; HTTP/1 has no GOAWAY, degraded to close");
    }
    if let Some(warning) = script.warning.as_deref() {
        detail.push_str("; ");
        detail.push_str(warning);
    }

    recorder::record(
        ctx.event(recorder::KIND_RESP_HEAD)
            .status(rendered.status)
            .framing(&rendered.framing)
            .len(rendered.body_len)
            .behavior(script.behavior.as_str())
            .detail(&detail),
    );

    io.get_mut()
        .write_all(&rendered.bytes)
        .await
        .map_err(|e| format!("write response: {e}"))?;
    io.get_mut().flush().await.map_err(|e| format!("flush: {e}"))?;

    recorder::record(ctx.event(recorder::KIND_RESP_DONE).len(rendered.body_len));

    if rendered.must_close {
        return Ok(ConnOutcome::Close);
    }
    Ok(match after {
        AfterResponse::Linger => ConnOutcome::KeepAlive,
        AfterResponse::Close | AfterResponse::Goaway => ConnOutcome::Close,
        AfterResponse::Reset => ConnOutcome::ResetTcp,
    })
}

/// The framing report: what the backend application observed about the request
/// the gateway wrote. A convenience for the happy path only — every claim about
/// the gateway's wire behaviour goes through the recorder, because on the
/// failure paths no response is ever written and an echoed header cannot exist.
fn framing_report(head: &RequestHead, script: &UpScript, state: &BodyState, ctx: &ReqCtx) -> Vec<(String, String)> {
    let mut headers = vec![
        ("x-up-headers-eos".to_string(), head.headers_eos().to_string()),
        (
            "x-up-req-content-length".to_string(),
            head.content_length()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "absent".to_string()),
        ),
        (
            "x-up-req-transfer-encoding".to_string(),
            head.transfer_encoding().unwrap_or("absent").to_string(),
        ),
        ("x-up-req-body-len".to_string(), state.total.to_string()),
        (
            "x-up-req-body-sha256".to_string(),
            format!("{:x}", state.digest.clone().finalize()),
        ),
        ("x-up-req-trailers".to_string(), state.trailers.to_string()),
        ("x-up-http-version".to_string(), head.version.clone()),
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

/// Read (and discard) until the peer FINs or the park window elapses, so the
/// peer's own close is observed rather than raced by ours.
async fn park_until_peer_closes(io: &mut BufReader<TcpStream>, ctx: &ReqCtx) {
    let mut scratch = [0u8; 1024];
    let deadline = tokio::time::sleep(HANG_PARK);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => {
                recorder::record(ctx.event(recorder::KIND_RESP_DONE).detail("park window elapsed"));
                return;
            }
            read = io.read(&mut scratch) => match read {
                Ok(0) => {
                    recorder::record(
                        ctx.event(recorder::KIND_PEER_HALF_CLOSE)
                            .detail("peer FIN observed while parked"),
                    );
                    return;
                }
                Ok(_) => continue,
                Err(e) => {
                    recorder::record(
                        ctx.event(recorder::KIND_PEER_RESET)
                            .code("TCP_RST")
                            .detail(&format!("read error while parked: {e}")),
                    );
                    return;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{informational_attempt, InformationalAttempt};

    #[test]
    fn informational_retry_requires_a_trace_and_only_hangs_once() {
        assert_eq!(informational_attempt(None), InformationalAttempt::MissingTrace);
        assert_eq!(informational_attempt(Some(1)), InformationalAttempt::First);
        assert_eq!(informational_attempt(Some(2)), InformationalAttempt::Echo);
        assert_eq!(informational_attempt(Some(9)), InformationalAttempt::Echo);
    }
}
