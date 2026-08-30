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
    ctx.request_body_config =
        crate::config::get_resolved_request_body_config(edgion_http.request_body_config_name.as_deref());
    ctx.remote_ip_header = crate::config::get_remote_ip_header(edgion_http.request_body_config_name.as_deref());
    global_metrics().ctx_created();
    ctx
}
