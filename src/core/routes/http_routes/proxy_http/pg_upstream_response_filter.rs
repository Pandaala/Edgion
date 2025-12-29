use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use crate::types::EdgionHttpContext;
use super::EdgionHttp;

#[inline]
pub fn upstream_response_filter(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Record status code
    let status_code = upstream_response.status.as_u16();
    ctx.request_info.status = Some(status_code);
    
    // Apply custom server headers (including Server header)
    edgion_http.server_header_opts.apply_to_response(upstream_response);
    
    // Record header time (time from this upstream's start_time to receiving response header)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ht = upstream.start_time.elapsed().as_millis() as u64;
        upstream.ht = Some(ht);
        upstream.status = Some(status_code);
    }
    
    // Run rule-level upstream_response edgion_plugins (sync)
    if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit.rule.plugin_runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    // Run backend-level upstream_response edgion_plugins (sync)
    if let Some(backend) = ctx.selected_backend.clone() {
        backend.plugin_runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    Ok(())
}

