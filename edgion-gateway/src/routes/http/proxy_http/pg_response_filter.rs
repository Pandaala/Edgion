use super::content_type::classify_response_stream_kind;
use super::pg_init_downstream_modules::DEFAULT_COMPRESSION_LEVEL;
use super::pg_upstream_response_body_filter::enforce_stream_processor_framing;
use super::pg_upstream_response_filter::{is_interim_informational, response_denial_error, response_stage_settled};
use super::EdgionHttpProxy;
use crate::ctx::ResponseStreamKind;
use crate::plugins::http::common::{connection_listed_hop_by_hop, HOP_BY_HOP_HEADERS};
use crate::EdgionHttpContext;
use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use http::HeaderName;
use pingora_core::modules::http::compression::ResponseCompression;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

#[inline]
fn is_connection_option_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

/// Remove connection-scoped metadata before forwarding a response header.
///
/// Every repeated `Connection` field is inspected before the header map is
/// mutated. This keeps an invalid field fail-closed and atomic while the
/// shared parser remains the sole owner of comma splitting and case folding.
/// A 101 response is an upgrade handshake and deliberately retains its
/// connection metadata.
#[inline]
fn sanitize_forwarded_response_headers(response: &mut ResponseHeader) -> pingora_core::Result<bool> {
    if response.status == http::StatusCode::SWITCHING_PROTOCOLS {
        return Ok(false);
    }

    let had_transfer_encoding = response.headers.contains_key(http::header::TRANSFER_ENCODING);
    let mut dynamic_names = Vec::<HeaderName>::new();
    let mut invalid_connection_value = false;

    for value in response.headers.get_all(http::header::CONNECTION) {
        let Ok(value) = value.to_str() else {
            invalid_connection_value = true;
            continue;
        };
        for option in connection_listed_hop_by_hop(value) {
            if !option.bytes().all(is_connection_option_tchar) {
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(option.as_bytes()) else {
                continue;
            };
            if !dynamic_names.contains(&name) {
                dynamic_names.push(name);
            }
        }
    }

    if invalid_connection_value {
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InvalidHTTPHeader,
            "invalid upstream Connection header",
        ));
    }

    for name in &dynamic_names {
        response.remove_header(name);
    }
    for name in HOP_BY_HOP_HEADERS {
        response.remove_header(*name);
    }
    // Legacy proxy metadata is response-hop scoped too. Keep the canonical
    // request header policy unchanged.
    response.remove_header("proxy-connection");

    Ok(had_transfer_encoding)
}

/// Select compression only for a final response whose complete size is known.
///
/// Pingora's gzip encoder does not flush after each input chunk, so enabling it
/// for an unknown-length response can hold a low-rate stream indefinitely.
/// Multiple or combined `Content-Length` values are rejected conservatively;
/// any `Transfer-Encoding`, including one removed by hop-by-hop sanitization,
/// also means the response is not known-length. The compression module removes
/// an accepted length before forwarding.
#[inline]
fn response_compression_level(
    enabled: bool,
    stream_kind: ResponseStreamKind,
    had_transfer_encoding: bool,
    response: &ResponseHeader,
) -> u32 {
    if !enabled || stream_kind == ResponseStreamKind::Sse {
        return 0;
    }

    let mut content_lengths = response.headers.get_all(http::header::CONTENT_LENGTH).iter();
    let has_one_valid_content_length = content_lengths
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
        && content_lengths.next().is_none();

    let has_transfer_encoding = had_transfer_encoding || response.headers.contains_key(http::header::TRANSFER_ENCODING);

    if has_one_valid_content_length && !has_transfer_encoding {
        DEFAULT_COMPRESSION_LEVEL
    } else {
        0
    }
}

#[inline]
fn complete_local_reply_delivery(
    ctx: &mut EdgionHttpContext,
    body_result: pingora_core::Result<()>,
) -> pingora_core::Result<()> {
    match body_result {
        Ok(()) => {
            ctx.record_info(EdgionInfoCode::PluginResponseReplaced);
            Ok(())
        }
        Err(error) => {
            ctx.record_error(EdgionErrorCode::ClientClosed);
            Err(error)
        }
    }
}

/// Claim a staged response-phase reply before touching the downstream.
///
/// A plugin can surface a status outside the ordinary final-response range
/// through a lower-level runtime such as native WAF. Falling through after a
/// rejected claim would silently forward the upstream response, so every
/// rejected claim becomes non-retryable local control flow instead.
#[inline]
fn claim_response_local_reply(ctx: &mut EdgionHttpContext, status: u16) -> pingora_core::Result<()> {
    if ctx.claim_final_response(status) {
        return Ok(());
    }

    let mut error = pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(500),
        "invalid or duplicate edgion response local reply",
    );
    error.set_retry(false);
    Err(error)
}

#[inline]
pub async fn response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Mirror of the sync entry point's guard: an interim 1xx is not the response
    // the plugins are meant to see. Without it a plugin denial on a `100`/`103`
    // arms the response-stage abort — `apply_response_local_reply` refuses an
    // informational response, so `check_response_stage_terminate` takes the
    // refused branch — and kills an exchange whose real response has not arrived
    // yet. The final header runs the chains, so nothing is skipped, only
    // deferred.
    //
    // Deliberately before gateway/plugin mutation and, like the sync side,
    // without stamping `ctx.request_info.status`: only the mandatory
    // hop-by-hop sanitizer runs before the early return. The access-log status
    // must describe the final response, not a 100 Continue.
    if is_interim_informational(upstream_response.status) {
        sanitize_forwarded_response_headers(upstream_response)?;
        return Ok(());
    }

    let switching_protocols = upstream_response.status == http::StatusCode::SWITCHING_PROTOCOLS;
    let mut had_transfer_encoding = false;
    if !switching_protocols && !response_stage_settled(ctx) {
        had_transfer_encoding = sanitize_forwarded_response_headers(upstream_response)?;
    }

    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // RFC 9110 §7.6.1 cleanup happened above and is repeated after async
    // plugins. Injecting `Connection: close` during graceful shutdown is
    // redundant: `set_keepalive(None)` in pg_early_request_filter makes
    // Pingora's H1 codec emit it on the downstream wire.

    // A sync Phase-6 plugin may have already stashed a local reply. Do NOT
    // return here: fall through so the write+abort block below actually writes
    // the stashed reply. The group guards below (`is_some()` -> skip) prevent
    // Phase-7 plugins from running on top of a Phase-6 reply.

    if !response_stage_settled(ctx) {
        if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
            grpc_route_unit
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        } else if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    if !response_stage_settled(ctx) {
        if let Some(runtime) = ctx
            .selected_grpc_backend
            .as_ref()
            .map(crate::routes::grpc::SelectedGrpcBackend::plugin_runtime)
        {
            runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        } else if let Some(runtime) = ctx
            .selected_backend
            .as_ref()
            .map(crate::backends::SelectedHttpBackend::plugin_runtime)
        {
            runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_gateway_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_async_plugins_count() == 0 {
                    continue;
                }
                runtime
                    .run_upstream_response_plugins_async(session, ctx, upstream_response)
                    .await;
            }
        }
    }

    // Run global plugins last (onion: response stages go inner → outer).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_global_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_async_plugins_count() == 0 {
                    continue;
                }
                runtime
                    .run_upstream_response_plugins_async(session, ctx, upstream_response)
                    .await;
            }
        }
    }

    // Async response plugins run after H1 made its framing choice. Remove a
    // stale length once more so they cannot produce CL+TE or incorrect H2
    // length metadata for a body-mutating processor.
    if !response_stage_settled(ctx) {
        enforce_stream_processor_framing(upstream_response, ctx);
        if !switching_protocols {
            had_transfer_encoding |= sanitize_forwarded_response_headers(upstream_response)?;
        }
    }

    // A denial no local reply could carry aborts before the forward path below.
    // Nothing has been written downstream yet, so `fail_to_proxy` still gets to
    // deliver the plugin's status.
    if let Some(denial) = ctx.response_denial() {
        return Err(response_denial_error(denial));
    }

    // filter-04b: a response-phase plugin stashed a local reply. Write it
    // directly to the downstream and abort the pump so the upstream response is
    // not forwarded. This is the single write+abort point; it handles both
    // body-less and body-present upstreams (Phase 8 is never reached).
    let pending_local_reply = ctx.take_pending_response_local_reply();
    if let Some((mut header, body)) = pending_local_reply {
        // Stamp the gateway Server identity onto the freshly-built reply header.
        edgion_http.server_header_opts.apply_to_response(&mut header);
        let body_empty = body.is_empty();
        let status = header.status.as_u16();
        claim_response_local_reply(ctx, status)?;
        // Writing this reply and returning the abort `Err` below tears down the
        // downstream connection. Disable keepalive first so Pingora's H1 codec
        // emits `Connection: close` on the wire (no-op semantics for H2): without
        // it the client sees a keepalive-eligible response, pools the connection,
        // and may race to reuse the already-closed socket on a later request,
        // surfacing as an intermittent transport-level send error.
        session.set_keepalive(None);
        ctx.request_info.trace_id.reconcile_response(&mut *header);
        session.write_response_header(header, body_empty).await?;
        // Mark immediately after the header commits and before awaiting a body.
        // Pingora may cancel this hook future if the duplex pump's other side
        // fails; the sentinel must already suppress a second response and
        // upstream-failure accounting in that cancellation window.
        ctx.mark_local_reply_written();
        let body_result = if body_empty {
            // Session::write_response_header does not propagate its end flag
            // to the H2 codec. Finish explicitly before the sentinel abort
            // drops the session, otherwise an empty reply ends with CANCEL.
            session.write_response_body(None, true).await
        } else {
            session.write_response_body(Some(body), true).await
        };
        complete_local_reply_delivery(ctx, body_result)?;
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "edgion response local reply",
        ));
    }

    // `response_filter` is the one response-header hook shared by origin and
    // cache-hit paths. Configure compression only here, after every Edgion
    // response plugin and framing repair, and immediately before Pingora runs
    // the downstream modules' response-header filters.
    ctx.response_stream_kind = classify_response_stream_kind(upstream_response);
    let eligibility = final_response_eligibility(
        &session.req_header().method,
        upstream_response.status,
        ctx.response_stream_kind,
    );
    if let Some(state) = ctx.ai_state_mut() {
        state.final_response_eligibility = Some(eligibility);
    }
    if let Some(compression_ctx) = session.downstream_modules_ctx.get_mut::<ResponseCompression>() {
        let level = response_compression_level(
            ctx.egc.enable_compression,
            ctx.response_stream_kind,
            had_transfer_encoding,
            upstream_response,
        );
        compression_ctx.adjust_level(level);
    }
    ctx.request_info.trace_id.reconcile_response(upstream_response);
    crate::ai::conversion::chat_to_anthropic::head::compact_final_header(upstream_response, ctx)?;
    ctx.select_final_downstream_status(upstream_response.status.as_u16());
    // The response onion and both framing-repair passes are complete. From
    // this point through body/trailer callbacks and logging, one request-local
    // driver owns the fixed processor set.
    ctx.freeze_response_processor_driver();
    Ok(())
}

fn final_response_eligibility(
    method: &http::Method,
    status: http::StatusCode,
    stream_kind: crate::ctx::ResponseStreamKind,
) -> crate::response::terminal::FinalResponseEligibility {
    crate::response::terminal::FinalResponseEligibility {
        method_allows_body: method != http::Method::HEAD,
        status_allows_body: !status.is_informational() && !matches!(status.as_u16(), 204 | 304),
        stream_kind,
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_response_filter_test.rs"]
mod tests;
