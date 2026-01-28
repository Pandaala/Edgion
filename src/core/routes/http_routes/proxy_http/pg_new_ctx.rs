use super::EdgionHttp;
use crate::core::observe::global_metrics;
use crate::types::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttp) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();

    // Copy gateway info for access log and metrics
    ctx.gateway_info = edgion_http.gateway_info.clone();

    global_metrics().ctx_created();
    ctx
}
