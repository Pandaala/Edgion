use super::pg_upstream_response_filter::{is_interim_informational, response_denial_error, response_stage_settled};
use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

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

    // A denial no local reply could carry aborts before the forward path below.
    // Nothing has been written downstream yet, so `fail_to_proxy` still gets to
    // deliver the plugin's status.
    if let Some(denial) = ctx.response_denial.as_ref() {
        return Err(response_denial_error(denial));
    }

    // filter-04b: a response-phase plugin stashed a local reply. Write it
    // directly to the downstream and abort the pump so the upstream response is
    // not forwarded. This is the single write+abort point; it handles both
    // body-less and body-present upstreams (Phase 8 is never reached).
    if ctx.local_reply_header.is_some() && !ctx.request_info.response_committed {
        let mut header = ctx.local_reply_header.take().expect("checked is_some");
        // Stamp the gateway Server identity onto the freshly-built reply header.
        edgion_http.server_header_opts.apply_to_response(&mut header);
        let body = ctx.local_reply_body.take().unwrap_or_default();
        let body_empty = body.is_empty();
        ctx.request_info.status = Some(header.status.as_u16());
        ctx.request_info.response_committed = true;
        ctx.local_reply_written = true;
        // Writing this reply and returning the abort `Err` below tears down the
        // downstream connection. Disable keepalive first so Pingora's H1 codec
        // emits `Connection: close` on the wire (no-op semantics for H2): without
        // it the client sees a keepalive-eligible response, pools the connection,
        // and may race to reuse the already-closed socket on a later request,
        // surfacing as an intermittent transport-level send error.
        session.set_keepalive(None);
        session.write_response_header(header, body_empty).await?;
        if !body_empty {
            session.write_response_body(Some(body), true).await?;
        }
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "edgion response local reply",
        ));
    }

    Ok(())
}
