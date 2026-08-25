use super::content_type::classify_response_stream_kind;
use super::pg_init_downstream_modules::DEFAULT_COMPRESSION_LEVEL;
use super::pg_upstream_response_body_filter::enforce_stream_processor_framing;
use super::pg_upstream_response_filter::{is_interim_informational, response_denial_error, response_stage_settled};
use super::EdgionHttpProxy;
use crate::ctx::ResponseStreamKind;
use crate::EdgionHttpContext;
use edgion_resources::EdgionInfoCode;
use pingora_core::modules::http::compression::ResponseCompression;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// Select compression only for a final response whose complete size is known.
///
/// Pingora's gzip encoder does not flush after each input chunk, so enabling it
/// for an unknown-length response can hold a low-rate stream indefinitely.
/// Multiple or combined `Content-Length` values are rejected conservatively;
/// any `Transfer-Encoding` also means the response is not known-length. The
/// compression module removes an accepted length before forwarding.
#[inline]
fn response_compression_level(enabled: bool, stream_kind: ResponseStreamKind, response: &ResponseHeader) -> u32 {
    let mut content_lengths = response.headers.get_all(http::header::CONTENT_LENGTH).iter();
    let has_one_valid_content_length = content_lengths
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
        && content_lengths.next().is_none();

    let has_transfer_encoding = response.headers.contains_key(http::header::TRANSFER_ENCODING);

    if enabled && stream_kind != ResponseStreamKind::Sse && has_one_valid_content_length && !has_transfer_encoding {
        DEFAULT_COMPRESSION_LEVEL
    } else {
        0
    }
}

#[inline]
fn enable_compression(edgion_http: &EdgionHttpProxy) -> bool {
    edgion_http
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|server| server.enable_compression)
        .unwrap_or(false)
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
    // Deliberately BEFORE any header mutation and, like the sync side, without
    // stamping `ctx.request_info.status`: the access-log status must describe the
    // final response, not a 100 Continue.
    if is_interim_informational(upstream_response.status) {
        return Ok(());
    }

    edgion_http.server_header_opts.apply_to_response(upstream_response);

    if let Some(trace_id) = &ctx.request_info.x_trace_id {
        let _ = upstream_response.insert_header("x-trace-id", trace_id.as_str());
    }

    // RFC 9110 §7.6.1 forbids forwarding hop-by-hop `Connection` headers. Injecting
    // `Connection: close` during graceful shutdown is redundant — `set_keepalive(None)` in
    // pg_early_request_filter already makes Pingora's H1 codec emit it on the downstream wire.

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
        if let Some(backend) = ctx.selected_grpc_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        } else if let Some(backend) = ctx.selected_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_gateway_plugins.as_ref().map(std::sync::Arc::clone) {
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
        if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(std::sync::Arc::clone) {
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
    let pending_local_reply = if ctx.response_write_started() {
        None
    } else {
        ctx.take_pending_response_local_reply()
    };
    if let Some((mut header, body)) = pending_local_reply {
        // Stamp the gateway Server identity onto the freshly-built reply header.
        edgion_http.server_header_opts.apply_to_response(&mut header);
        let body_empty = body.is_empty();
        let status = header.status.as_u16();
        if !ctx.claim_final_response(status) {
            return Ok(());
        }
        // Writing this reply and returning the abort `Err` below tears down the
        // downstream connection. Disable keepalive first so Pingora's H1 codec
        // emits `Connection: close` on the wire (no-op semantics for H2): without
        // it the client sees a keepalive-eligible response, pools the connection,
        // and may race to reuse the already-closed socket on a later request,
        // surfacing as an intermittent transport-level send error.
        session.set_keepalive(None);
        session.write_response_header(header, body_empty).await?;
        ctx.mark_local_reply_written();
        if !body_empty {
            session.write_response_body(Some(body), true).await?;
        }
        ctx.record_info(EdgionInfoCode::PluginResponseReplaced);
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
            enable_compression(edgion_http),
            ctx.response_stream_kind,
            upstream_response,
        );
        compression_ctx.adjust_level(level);
    }
    ctx.claim_final_response(upstream_response.status.as_u16());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{final_response_eligibility, response_compression_level, DEFAULT_COMPRESSION_LEVEL};
    use crate::ctx::ResponseStreamKind;
    use pingora_core::protocols::http::compression::ResponseCompressionCtx;
    use pingora_http::{RequestHeader, ResponseHeader};

    fn response(content_length: Option<&str>) -> ResponseHeader {
        let mut response = ResponseHeader::build(200, None).unwrap();
        response.insert_header("content-type", "application/json").unwrap();
        if let Some(content_length) = content_length {
            response.insert_header("content-length", content_length).unwrap();
        }
        response
    }

    #[test]
    fn final_response_eligibility_uses_live_method_status_and_stream_kind() {
        for status in [
            http::StatusCode::CONTINUE,
            http::StatusCode::NO_CONTENT,
            http::StatusCode::NOT_MODIFIED,
        ] {
            assert!(!final_response_eligibility(&http::Method::GET, status, ResponseStreamKind::Sse).allows_sse());
        }
        assert!(
            !final_response_eligibility(&http::Method::HEAD, http::StatusCode::OK, ResponseStreamKind::Sse)
                .allows_sse()
        );
        assert!(
            final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::Sse).allows_sse()
        );
        assert!(
            !final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::None)
                .allows_sse()
        );
    }

    #[test]
    fn compression_requires_one_valid_content_length() {
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(Some("128"))),
            DEFAULT_COMPRESSION_LEVEL
        );
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(None)),
            0
        );
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(Some("invalid"))),
            0
        );

        let mut duplicate = response(Some("128"));
        duplicate.append_header("content-length", "128").unwrap();
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &duplicate),
            0
        );

        let mut conflicting_framing = response(Some("128"));
        conflicting_framing
            .insert_header("transfer-encoding", "chunked")
            .unwrap();
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &conflicting_framing),
            0
        );
    }

    #[test]
    fn final_header_mutation_can_disable_compression() {
        let mut response = response(Some("128"));
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response),
            DEFAULT_COMPRESSION_LEVEL
        );

        response.remove_header("content-length");
        assert_eq!(response_compression_level(true, ResponseStreamKind::None, &response), 0);
    }

    #[test]
    fn compression_respects_config_and_sse_exemption() {
        let response = response(Some("128"));
        assert_eq!(
            response_compression_level(false, ResponseStreamKind::None, &response),
            0
        );
        assert_eq!(response_compression_level(true, ResponseStreamKind::Sse, &response), 0);
    }

    #[test]
    fn enabled_request_negotiation_survives_final_level_selection() {
        let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header("accept-encoding", "gzip").unwrap();
        compression.request_filter(&request);

        let mut response = response(Some("128"));
        compression.adjust_level(response_compression_level(true, ResponseStreamKind::None, &response));
        compression.response_header_filter(&mut response, false);

        assert_eq!(response.headers.get("content-encoding").unwrap(), "gzip");
        assert!(response.headers.get("content-length").is_none());
    }

    #[test]
    fn final_unknown_length_selection_disables_negotiated_compression() {
        let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header("accept-encoding", "gzip").unwrap();
        compression.request_filter(&request);

        let mut response = response(None);
        compression.adjust_level(response_compression_level(true, ResponseStreamKind::None, &response));
        compression.response_header_filter(&mut response, false);

        assert!(response.headers.get("content-encoding").is_none());
    }
}
fn final_response_eligibility(
    method: &http::Method,
    status: http::StatusCode,
    stream_kind: crate::ctx::ResponseStreamKind,
) -> crate::ai::terminal::FinalResponseEligibility {
    crate::ai::terminal::FinalResponseEligibility {
        method_allows_body: method != http::Method::HEAD,
        status_allows_body: !status.is_informational() && !matches!(status.as_u16(), 204 | 304),
        stream_kind,
    }
}
