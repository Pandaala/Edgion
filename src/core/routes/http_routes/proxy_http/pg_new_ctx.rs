use super::EdgionHttp;
use crate::core::observe::global_metrics;
use crate::types::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttp) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();

    // Use first gateway_info as default (will be overwritten upon successful route match)
    if let Some(gi) = edgion_http.gateway_infos.first() {
        ctx.gateway_info = gi.clone();
    }

    global_metrics().ctx_created();
    ctx
}
