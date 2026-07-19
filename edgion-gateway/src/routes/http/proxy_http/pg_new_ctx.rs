use super::EdgionHttpProxy;
use crate::observe::global_metrics;
use crate::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = edgion_http.listener.port as u16;
    ctx.allow_loopback_upstream = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|security| security.allow_loopback_upstream)
        .unwrap_or(false);
    ctx.max_body_size_bytes = edgion_http.edgion_gateway_config.spec.max_body_size_bytes();
    global_metrics().ctx_created();
    ctx
}
