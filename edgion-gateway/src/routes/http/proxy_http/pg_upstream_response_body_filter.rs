use std::sync::Arc;
use std::time::{Duration, Instant};

use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_proxy::Session;

/// Process upstream response body chunks
///
/// Returns:
/// - `Ok(None)` - continue processing normally
/// - `Ok(Some(duration))` - rate limit for the given duration (for bandwidth throttling)
#[inline]
pub fn upstream_response_body_filter(
    _edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    body: &mut Option<bytes::Bytes>,
    end_of_stream: bool,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Option<Duration>> {
    // Record body time (time from start_time to receiving first body chunk)
    // Only set once when bt is None (first chunk)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        if upstream.bt.is_none() {
            let bt = Instant::now()
                .saturating_duration_since(upstream.start_time)
                .as_millis() as u64;
            upstream.bt = Some(bt);
        }
    }

    // Run upstream_response_body_filter plugins for bandwidth throttling
    let mut max_delay: Option<Duration> = None;

    // Run route-level body filter plugins
    if let Some(route_unit) = ctx.route_unit.clone() {
        if route_unit.plugin_runtime.upstream_response_body_plugins_count() > 0 {
            if let Some(delay) =
                route_unit
                    .plugin_runtime
                    .run_upstream_response_body_plugins(session, ctx, body, end_of_stream)
            {
                max_delay = Some(delay);
            }
        }
    }

    // Run global plugins body filter (read from per-request snapshot; zero
    // format/store cost on the per-chunk hot path).
    if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.upstream_response_body_plugins_count() == 0 {
                continue;
            }
            if let Some(delay) = runtime.run_upstream_response_body_plugins(session, ctx, body, end_of_stream) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(delay),
                    None => delay,
                });
            }
        }
    }

    Ok(max_delay)
}
