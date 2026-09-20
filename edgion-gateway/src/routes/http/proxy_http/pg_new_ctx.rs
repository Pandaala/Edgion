use super::EdgionHttpProxy;
use crate::observe::global_metrics;
use crate::EdgionHttpContext;

#[inline]
pub fn new_ctx(edgion_http: &EdgionHttpProxy) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = edgion_http.listener_port;
    ctx.allow_loopback_upstream = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|security| security.allow_loopback_upstream)
        .unwrap_or(false);
    let request_config =
        crate::config::get_resolved_http_request_config(edgion_http.request_body_config_name.as_deref());
    ctx.request_body_config = request_config.request_body;
    ctx.remote_ip_header = request_config.remote_ip_header;
    global_metrics().ctx_created();
    ctx
}
