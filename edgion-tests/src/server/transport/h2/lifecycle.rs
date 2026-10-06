//! Deterministic, bounded upstream lifecycle probes for the shared test backend.

use super::*;

const LIMIT: Duration = Duration::from_secs(25);
pub(super) const UPLOAD_WINDOW: usize = 65_535;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    mode: &str,
    release_trace: Option<&str>,
    respond: &mut h2s::SendResponse<Bytes>,
    body: &mut h2::RecvStream,
    ctx: &ReqCtx,
    script: &UpScript,
    state: &mut BodyState,
    headers_eos: bool,
    handles: &StreamHandles,
) {
    let result = tokio::time::timeout(LIMIT, async {
        match mode {
            "stall" => {
                // The server handshake leaves SETTINGS_INITIAL_WINDOW_SIZE at
                // its RFC default. Consume exactly that credit, never return it.
                while state.total < UPLOAD_WINDOW {
                    match body.data().await {
                        Some(Ok(chunk)) => {
                            state.total += chunk.len();
                            state.digest.update(&chunk);
                            recorder::record(
                                ctx.event(recorder::KIND_REQ_DATA)
                                    .len(chunk.len())
                                    .end_stream(body.is_end_stream()),
                            );
                        }
                        other => return Err(format!("lifecycle-stall ended before window exhaustion: {other:?}")),
                    }
                }
                if state.total != UPLOAD_WINDOW || body.is_end_stream() {
                    return Err("lifecycle-stall requires exactly one open upload window".to_string());
                }
                recorder::record(
                    ctx.event(recorder::KIND_REQ_DATA)
                        .len(0)
                        .end_stream(false)
                        .detail("lifecycle-stall-window-exhausted"),
                );
                reply(respond, body, ctx, script, state, headers_eos, handles).await;
                // Retain the receive half and its outstanding credit until the
                // Gateway cancels its blocked upload after delivering EOS.
                match body.data().await {
                    Some(Err(error)) => {
                        let code = error.reason().map(reason_name).unwrap_or_else(|| "NONE".to_string());
                        recorder::record(
                            ctx.event(recorder::KIND_PEER_RESET)
                                .code(&code)
                                .len(state.total)
                                .detail("lifecycle-stall-upload-released"),
                        );
                        Ok(())
                    }
                    other => Err(format!("lifecycle-stall expected upload reset, got {other:?}")),
                }
            }
            "held" => {
                let trace = release_trace.ok_or_else(|| "lifecycle-held requires x-up-release-trace".to_string())?;
                if pump_body(body, ctx, state, None).await == PumpStop::PeerReset {
                    return Ok(());
                }
                recorder::record(ctx.event(recorder::KIND_REQ_DATA).len(0).detail("lifecycle-held-ready"));
                tokio::select! {
                    _ = recorder::wait_for_request_headers(trace) => {
                        reply(respond, body, ctx, script, state, headers_eos, handles).await;
                    }
                    reset = std::future::poll_fn(|cx| respond.poll_reset(cx)) => {
                        match reset {
                            Ok(reason) => recorder::record(ctx.event(recorder::KIND_PEER_RESET)
                                .code(&reason_name(reason)).detail("lifecycle-held-peer-reset")),
                            Err(error) => return Err(format!("lifecycle-held reset poll: {error}")),
                        }
                    }
                }
                Ok(())
            }
            mode if mode.starts_with("goaway-") => {
                let (frames, close, valid) =
                    frames(mode, ctx.stream).ok_or_else(|| format!("unknown lifecycle mode: {mode}"))?;
                let ordinal = ctx.trace.as_deref().map(recorder::note_trace_request).unwrap_or(1);
                if pump_body(body, ctx, state, None).await == PumpStop::PeerReset {
                    return Ok(());
                }
                if ordinal > 1 {
                    let mut followup = script.clone();
                    followup.status = 201;
                    reply(respond, body, ctx, &followup, state, headers_eos, handles).await;
                    recorder::record(
                        ctx.event(recorder::KIND_RESP_DONE)
                            .trace_request_ordinal(Some(ordinal))
                            .status(201)
                            .detail("lifecycle-goaway-followup"),
                    );
                    return Ok(());
                }
                let len = frames.len();
                if !handles.wait_for_flush().await
                    || !handles.fault(retry_wire::Fault::Frames { bytes: frames, close }).await
                {
                    return Err(format!("lifecycle-{mode} injection flush failed"));
                }
                recorder::record(
                    ctx.event(recorder::KIND_RESP_DONE)
                        .trace_request_ordinal(Some(ordinal))
                        .len(len)
                        .end_stream(false)
                        .code(if valid { "GOAWAY_NO_ERROR" } else { "GOAWAY_INVALID" })
                        .detail(&format!("lifecycle-{mode}")),
                );
                if valid {
                    reply(respond, body, ctx, script, state, headers_eos, handles).await;
                }
                Ok(())
            }
            _ => Err(format!("unknown lifecycle mode: {mode}")),
        }
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            recorder::record(ctx.event(recorder::KIND_RESP_DONE).detail(&error));
            respond.send_reset(h2::Reason::INTERNAL_ERROR);
        }
        Err(_) => {
            recorder::record(
                ctx.event(recorder::KIND_RESP_DONE)
                    .detail(&format!("lifecycle-{mode} deadline exceeded")),
            );
            respond.send_reset(h2::Reason::CANCEL);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reply(
    respond: &mut h2s::SendResponse<Bytes>,
    body: &mut h2::RecvStream,
    ctx: &ReqCtx,
    script: &UpScript,
    state: &mut BodyState,
    headers_eos: bool,
    handles: &StreamHandles,
) {
    let mut reply = script.clone();
    reply.resp_len = 64;
    reply.after_response = AfterResponse::Linger;
    reply.delay_ms = 0;
    reply.response_trailers = false;
    respond_h2(respond, body, ctx, &reply, state, headers_eos, handles).await;
}

pub(super) fn frames(mode: &str, stream: u32) -> Option<(Vec<u8>, bool, bool)> {
    fn goaway(stream: u32, last: u32) -> Vec<u8> {
        let mut out = vec![0, 0, 8, 7, 0];
        out.extend_from_slice(&stream.to_be_bytes());
        out.extend_from_slice(&last.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out
    }
    let (out, close, valid) = match mode {
        "goaway-descending" => {
            let mut out = goaway(0, 0x7fff_ffff);
            out.extend(goaway(0, stream));
            (out, false, true)
        }
        "goaway-excluding" => {
            let mut out = goaway(0, 0x7fff_ffff);
            out.extend(goaway(0, stream.saturating_sub(2)));
            (out, false, true)
        }
        "goaway-raising" => {
            let mut out = goaway(0, stream.saturating_sub(2));
            out.extend(goaway(0, stream));
            (out, false, true)
        }
        "goaway-malformed" => {
            let mut out = goaway(0, stream);
            out[2] = 4;
            out.truncate(13);
            (out, false, false)
        }
        "goaway-nonzero" => (goaway(stream, stream), false, false),
        "goaway-truncated" => {
            let mut out = goaway(0, stream);
            out.truncate(15);
            (out, true, false)
        }
        _ => return None,
    };
    Some((out, close, valid))
}
