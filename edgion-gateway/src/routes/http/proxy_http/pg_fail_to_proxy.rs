use super::EdgionHttpProxy;
use crate::request_body::BodyReadError;
use crate::runtime::server::send_grpc_status_trailers_only;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::send_grpc_trailers_only;
use crate::{EdgionHttpContext, PeerTimeoutSource, ResponseStageDenial};
use edgion_resources::EdgionErrorCode;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};

#[inline]
fn is_downstream_read_timeout(e: &Error) -> bool {
    matches!(e.etype(), ErrorType::ReadTimedout) && matches!(e.esource(), ErrorSource::Downstream)
}

#[inline]
fn is_terminal_upstream_io_timeout(e: &Error) -> bool {
    matches!(e.etype(), ErrorType::ReadTimedout | ErrorType::WriteTimedout)
        && matches!(e.esource(), ErrorSource::Upstream)
}

fn is_body_capacity_error(e: &Error) -> bool {
    if !matches!(e.etype(), HTTPStatus(503)) {
        return false;
    }
    let mut cause = e
        .cause
        .as_deref()
        .map(|cause| cause as &(dyn std::error::Error + 'static));
    while let Some(error) = cause {
        if error.downcast_ref::<BodyReadError>() == Some(&BodyReadError::CapacityExceeded) {
            return true;
        }
        cause = error.source();
    }
    false
}

#[inline]
fn classify_proxy_status(e: &Error) -> u16 {
    match e.etype() {
        HTTPStatus(code) => *code,
        ErrorType::ReadTimedout if is_downstream_read_timeout(e) => 408,
        ErrorType::ReadTimedout | ErrorType::WriteTimedout => match e.esource() {
            ErrorSource::Downstream => 499,
            ErrorSource::Upstream => 504,
            _ => 504,
        },
        ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
        _ => match e.esource() {
            ErrorSource::Upstream => 502,
            ErrorSource::Downstream => match e.etype() {
                WriteError | ReadError | ConnectionClosed => 499,
                _ => 400,
            },
            ErrorSource::Internal | ErrorSource::Unset => 500,
        },
    }
}

#[inline]
fn proxy_error_diagnostic(e: &Error, code: u16, timeout_source: Option<PeerTimeoutSource>) -> Option<EdgionErrorCode> {
    if is_body_capacity_error(e) {
        Some(EdgionErrorCode::BodyCaptureCapacityExceeded)
    } else if is_downstream_read_timeout(e) {
        Some(EdgionErrorCode::BodyReadTimeout)
    } else if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        Some(EdgionErrorCode::ClientClosed)
    } else if is_terminal_upstream_io_timeout(e) {
        match timeout_source {
            Some(PeerTimeoutSource::RequestDeadline) => Some(EdgionErrorCode::RequestTimeoutExceeded),
            Some(PeerTimeoutSource::GrpcDeadline) => Some(EdgionErrorCode::GrpcClientDeadlineExceeded),
            Some(PeerTimeoutSource::BackendRequest) | None => None,
        }
    } else {
        None
    }
}

#[inline]
fn should_append_proxy_error_to_upstream(e: &Error, code: u16) -> bool {
    is_terminal_upstream_io_timeout(e) || (code != 504 && code != 499 && !is_downstream_read_timeout(e))
}

#[inline]
fn append_proxy_error_to_upstream(errors: &mut Vec<std::borrow::Cow<'static, str>>, e: &Error, code: u16) {
    if !should_append_proxy_error_to_upstream(e, code) {
        return;
    }

    let kind = e.etype().as_str();
    if is_terminal_upstream_io_timeout(e) && errors.iter().any(|existing| existing == kind) {
        return;
    }
    errors.push(kind.into());
}

#[inline]
fn record_proxy_error_attribution(ctx: &mut EdgionHttpContext, e: &Error, code: u16) {
    if let Some(diagnostic) = proxy_error_diagnostic(e, code, ctx.peer_timeout_source) {
        ctx.record_error(diagnostic);
    }
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        append_proxy_error_to_upstream(&mut upstream.err, e, code);
    }
}

#[inline]
fn can_reuse_downstream_after_proxy_error(code: u16) -> bool {
    matches!(code, 400 | 403 | 404 | 429)
}

#[inline]
fn select_proxy_outcome_status(ctx: &mut EdgionHttpContext, e: &Error, code: u16) {
    if code == 0 {
        return;
    }
    if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        ctx.select_terminal_downstream_status(499);
    } else {
        ctx.select_final_downstream_status(code);
    }
}

fn unary_transport_error(e: &Error, code: u16) -> bool {
    if !matches!(code, 502 | 504) {
        return false;
    }
    if !matches!(e.etype(), HTTPStatus(_)) {
        return matches!(e.esource(), ErrorSource::Upstream);
    }
    // The connect deadline handler wraps the upstream failure in HTTP 504.
    // Pingora stores its immediate cause as BError (Box<Error>).
    matches!(e.etype(), HTTPStatus(504))
        && e.cause
            .as_deref()
            .and_then(|cause| cause.downcast_ref::<Box<Error>>())
            .is_some_and(|cause| matches!(cause.esource(), ErrorSource::Upstream))
}

async fn write_proxy_error_reply(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    code: u16,
    error: &Error,
) -> bool {
    if ctx.grpc_transcoding.is_some() {
        let body = bytes::Bytes::from_static(b"{\"error\":\"gRPC upstream exchange failed\"}");
        let Ok(mut header) = pingora_http::ResponseHeader::build(code, None) else {
            return true;
        };
        let _ = header.insert_header("content-type", "application/json");
        let _ = header.insert_header("content-length", body.len().to_string());
        server_header_opts.apply_to_response(&mut header);
        ctx.request_info.trace_id.reconcile_response(&mut header);
        if ctx.claim_final_response(code) {
            session.write_error_response(header, body).await.is_err()
        } else {
            false
        }
    } else if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        send_grpc_trailers_only(session, ctx, server_header_opts, code)
            .await
            .is_err()
    } else if let Some(owner) = ctx
        .ai_state()
        .and_then(|state| state.unary_owner.as_ref())
        .filter(|owner| ctx.response_denial().is_none() && (owner.raw_success() || unary_transport_error(error, code)))
    {
        let reply = owner.error_reply(code, ctx);
        match reply {
            Ok((resp, body)) if ctx.claim_final_response(code) => {
                session.write_error_response(resp, body).await.is_err()
            }
            _ => false,
        }
    } else {
        let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
        server_header_opts.apply_to_response(&mut resp);
        ctx.request_info.trace_id.reconcile_response(&mut resp);
        if ctx.claim_final_response(code) {
            session
                .write_error_response(resp, bytes::Bytes::default())
                .await
                .is_err()
        } else {
            false
        }
    }
}

#[inline]
pub async fn fail_to_proxy(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    e: &Error,
    ctx: &mut EdgionHttpContext,
) -> FailToProxy {
    // filter-04b: the local reply was already written downstream by
    // response_filter. Do not write an error page or pollute upstream.err.
    // NOTE (filter-04c): can_reuse_downstream is reported true to express intent,
    // but the post-proxy abort path (the sentinel Err returned by response_filter)
    // does not consult this field and tears the H1 downstream connection down.
    // This is safe because response_filter disables keepalive before writing;
    // preserving reuse is a separate Pingora lifecycle feature. See
    // skills/04-review/http-body/intentional-design/response-local-reply-h1-close-is-feature.md.
    if ctx.local_reply_written() {
        return FailToProxy {
            error_code: ctx.final_downstream_status().unwrap_or(500),
            can_reuse_downstream: true,
        };
    }

    // A response-stage `ErrGrpcError` the stage itself could not write (the sync
    // runner cannot await). Deliver the plugin's exact gRPC status here, ahead
    // of the generic status handling below, which would otherwise re-derive a
    // code from the synthetic HTTP 200 the abort carries. Marks the response
    // committed, so that generic block then skips this request.
    let grpc_denial = if matches!(ctx.response_denial(), Some(ResponseStageDenial::Grpc { .. })) {
        ctx.take_response_denial()
    } else {
        None
    };
    if let Some(ResponseStageDenial::Grpc { status, message }) = grpc_denial {
        if send_grpc_status_trailers_only(
            session,
            ctx,
            &edgion_http.server_header_opts,
            status,
            message.as_deref(),
        )
        .await
        .is_err()
        {
            // Delivery failed: the downstream connection is dead.
            ctx.record_error(EdgionErrorCode::ClientClosed);
        }
    }

    let code = classify_proxy_status(e);

    // A stalled request-body upload is a client timeout, not a disconnect.
    // Other downstream transport failures keep the ClientClosed diagnostic.
    record_proxy_error_attribution(ctx, e, code);
    crate::ai::conversion::chat_to_anthropic::head::record_timeout_diagnostic(ctx, e);
    super::pg_response_head::record_guardrail_timeout(ctx, e);

    // A gateway-internal error that surfaced while the captured request body
    // was being replayed to the upstream is a body-replay failure (buffer
    // invariant violation or spill-storage read error). `upstream.err` alone
    // would show only the indistinguishable "InternalError", so record a
    // dedicated status; the error-log line (join on the standard trace ID) carries the
    // precise cause text. Upstream-attributed errors mid-replay (e.g. a write
    // timeout while pushing the replayed body) are deliberately excluded —
    // those are upstream failures, not buffer failures.
    if crate::request_body::request_body_replay_active(session)
        && !is_body_capacity_error(e)
        && (matches!(e.esource(), ErrorSource::Internal) || matches!(e.etype(), ErrorType::InternalError))
    {
        ctx.record_error(EdgionErrorCode::BodyReplayFailed);
    }

    if code > 0 {
        select_proxy_outcome_status(ctx, e, code);

        if let Some(upstream) = ctx.get_current_upstream_mut() {
            // Preserve a real response status stamped by upstream_response_filter;
            // this code is only the gateway's final outcome for the request.
            upstream.status.get_or_insert(code);
        }
    }

    if code > 0 && code != 499 && !ctx.response_write_started() {
        // gRPC classification uses both the media-type and route-derived facts;
        // a GRPCRoute match must never fall through to Pingora's HTML writer.
        if write_proxy_error_reply(session, ctx, &edgion_http.server_header_opts, code, e).await {
            // Delivery failed: the downstream connection is dead.
            ctx.record_error(EdgionErrorCode::ClientClosed);
        }
    }

    FailToProxy {
        error_code: code,
        can_reuse_downstream: can_reuse_downstream_after_proxy_error(code),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_fail_to_proxy_test.rs"]
mod tests;
