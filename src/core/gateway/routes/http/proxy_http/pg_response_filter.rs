use super::EdgionHttpProxy;
use crate::types::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

#[inline]
pub async fn response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Apply custom server headers (including Server header)
    // This catches all responses, including framework-generated error responses (e.g., 502)
    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // Add x-trace-id to response headers
    if let Some(trace_id) = &ctx.request_info.x_trace_id {
        let _ = upstream_response.insert_header("x-trace-id", trace_id.as_str());
    }

    // RFC 9110 §7.6.1 forbids forwarding hop-by-hop `Connection` headers in a
    // proxied response. `set_keepalive(None)` in pg_early_request_filter already
    // makes Pingora's H1 codec emit `Connection: close` on the downstream wire
    // during graceful shutdown, so the manual injection below was redundant and
    // a MUST violation. Kept commented out as a rollback marker.
    // if session.is_process_shutting_down() {
    //     let _ = upstream_response.insert_header("Connection", "close");
    // }

    // A sync Phase-6 plugin may have already produced a local reply (header
    // rebuilt, body stashed). The server-header + x-trace-id application above
    // re-stamped the gateway identity onto the rebuilt header; skip the async
    // Phase-7 plugin groups entirely.
    if ctx.local_reply_active {
        return Ok(());
    }

    // Run rule-level response edgion_plugins (async)
    if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit
            .rule
            .plugin_runtime
            .run_upstream_response_plugins_async(session, ctx, upstream_response)
            .await;
    }

    // Run backend-level response edgion_plugins (async)
    if !ctx.local_reply_active {
        if let Some(backend) = ctx.selected_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    // Run global plugins (D3-A gap fill from unified snapshot Phase 2).
    // Order: route → backend → global (onion: response stages go inner → outer).
    if !ctx.local_reply_active {
        if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(std::sync::Arc::clone) {
            for runtime in plugins.iter() {
                if ctx.local_reply_active {
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

    // A Phase-7 plugin that produced a local reply cleared the upstream headers
    // (including the Server header applied at the top of this hook). Re-stamp
    // the gateway Server header; the rebuild already re-added x-trace-id.
    if ctx.local_reply_active {
        edgion_http.server_header_opts.apply_to_response(upstream_response);
    }

    Ok(())
}
