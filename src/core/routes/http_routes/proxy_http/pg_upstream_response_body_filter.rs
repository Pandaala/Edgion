use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_proxy::Session;

#[inline]
pub fn upstream_response_body_filter(
    _edgion_http: &EdgionHttp,
    _session: &mut Session,
    _body: &mut Option<bytes::Bytes>,
    _end_of_stream: bool,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Record body time (time from start_time to receiving first body chunk)
    // Only set once when bt is None (first chunk)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        if upstream.bt.is_none() {
            let bt = upstream.start_time.elapsed().as_millis() as u64;
            upstream.bt = Some(bt);
        }
    }
    Ok(())
}
