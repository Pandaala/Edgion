use super::EdgionHttpProxy;
use crate::observe::global_metrics;
use crate::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let config = edgion_http.egc.load();
    let mut ctx = EdgionHttpContext::with_gateway_config(config);
    ctx.request_info.listener_port = edgion_http.listener_port;
    ctx.allow_loopback_upstream = ctx.egc.allow_loopback_upstream;
    ctx.request_body_config = ctx.egc.request_body;
    ctx.preflight_policy = ctx.egc.preflight_policy;
    global_metrics().ctx_created();
    ctx
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_new_ctx_test.rs"]
mod tests;
