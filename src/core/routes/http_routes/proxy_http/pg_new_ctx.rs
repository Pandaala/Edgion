use crate::types::EdgionHttpContext;
use crate::core::observe::global_metrics;
use super::EdgionHttp;

#[inline]
pub fn new_ctx(_edgion_http: &EdgionHttp) -> EdgionHttpContext {
    let ctx = EdgionHttpContext::new();
    global_metrics().ctx_created();
    ctx
}

