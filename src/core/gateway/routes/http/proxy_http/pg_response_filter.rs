use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

#[inline]
pub async fn response_filter(
    edgion_http: &EdgionHttp,
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

    // Handle graceful shutdown:
    // If the process is shutting down, add Connection: close header to notify the client/LB
    // that this connection should not be reused.
    // This helps in draining connections gracefully during rolling updates.
    if session.is_process_shutting_down() {
        let _ = upstream_response.insert_header("Connection", "close");
        tracing::debug!("Process shutting down, adding Connection: close header");
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
    if let Some(backend) = ctx.selected_backend.clone() {
        backend
            .plugin_runtime
            .run_upstream_response_plugins_async(session, ctx, upstream_response)
            .await;
    }

    Ok(())
}
