use std::time::Duration;

use super::EdgionHttp;
use crate::core::gateway::plugins::http::get_global_plugin_store;
use crate::types::EdgionHttpContext;
use pingora_proxy::Session;

/// Process upstream response body chunks
///
/// Returns:
/// - `Ok(None)` - continue processing normally
/// - `Ok(Some(duration))` - rate limit for the given duration (for bandwidth throttling)
#[inline]
pub fn upstream_response_body_filter(
    _edgion_http: &EdgionHttp,
    session: &mut Session,
    body: &mut Option<bytes::Bytes>,
    end_of_stream: bool,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Option<Duration>> {
    // Record body time (time from start_time to receiving first body chunk)
    // Only set once when bt is None (first chunk)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        if upstream.bt.is_none() {
            let bt = upstream.start_time.elapsed().as_millis() as u64;
            upstream.bt = Some(bt);
        }
    }

    // Run upstream_response_body_filter plugins for bandwidth throttling
    let mut max_delay: Option<Duration> = None;

    // Run route-level body filter plugins
    if let Some(route_unit) = ctx.route_unit.clone() {
        if route_unit.rule.plugin_runtime.upstream_response_body_plugins_count() > 0 {
            if let Some(delay) =
                route_unit
                    .rule
                    .plugin_runtime
                    .run_upstream_response_body_plugins(session, ctx, body, end_of_stream)
            {
                max_delay = Some(delay);
            }
        }
    }

    // Run global plugins body filter
    if let Some(ref global_plugins_ref) = _edgion_http.edgion_gateway_config.spec.global_plugins_ref {
        for plugin_ref in global_plugins_ref {
            let plugin_key = format!(
                "{}/{}",
                plugin_ref.namespace.as_deref().unwrap_or("default"),
                &plugin_ref.name
            );
            if let Some(edgion_plugin) = get_global_plugin_store().get(&plugin_key) {
                if edgion_plugin.spec.plugin_runtime.upstream_response_body_plugins_count() > 0 {
                    if let Some(delay) = edgion_plugin.spec.plugin_runtime.run_upstream_response_body_plugins(
                        session,
                        ctx,
                        body,
                        end_of_stream,
                    ) {
                        max_delay = Some(match max_delay {
                            Some(current) => current.max(delay),
                            None => delay,
                        });
                    }
                }
            }
        }
    }

    Ok(max_delay)
}
