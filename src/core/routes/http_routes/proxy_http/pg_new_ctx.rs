use super::EdgionHttp;
use crate::core::observe::global_metrics;
use crate::types::EdgionHttpContext;

#[inline]
pub fn new_ctx(_edgion_http: &EdgionHttp) -> EdgionHttpContext {
    let ctx = EdgionHttpContext::new();
    global_metrics().ctx_created();
    ctx
}
