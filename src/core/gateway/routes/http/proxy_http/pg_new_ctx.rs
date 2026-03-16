use super::EdgionHttpProxy;
use crate::core::gateway::observe::global_metrics;
use crate::types::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = edgion_http.listener.port as u16;
    global_metrics().ctx_created();
    ctx
}
