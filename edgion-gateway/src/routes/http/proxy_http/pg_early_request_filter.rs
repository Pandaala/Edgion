use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_proxy::Session;

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    _ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Set client timeouts from pre-parsed config (no runtime overhead)
    let client_timeout = &edgion_http.parsed_timeouts.client;
    session.set_read_timeout(Some(client_timeout.read_timeout));
    session.set_write_timeout(Some(client_timeout.write_timeout));

    // Handle graceful shutdown:
    // If the process is shutting down, disable keepalive for this connection.
    // This prevents the client from trying to reuse this connection for subsequent requests,
    // which helps in draining traffic away from this instance.
    if session.is_process_shutting_down() {
        session.set_keepalive(None);
    } else {
        session.set_keepalive(Some(client_timeout.keepalive_timeout));
    }

    // gRPC-web bridge must be initialized here (step 1) — before Pingora's built-in
    // request_header_filter (step 2) — so it can transition Init -> Upgrade and convert
    // grpc-web -> grpc. Gate strictly on the grpc-web content-type prefix: native grpc
    // (application/grpc / application/grpc+proto) never matches, so its bridge stays
    // Disabled and fully inert. The module is only present when enable_http2 (get_mut None
    // otherwise), so this is a no-op on non-h2 listeners.
    let is_grpc_web = session
        .req_header()
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(super::is_grpc_web_content_type);
    if is_grpc_web {
        if let Some(bridge) = session.downstream_modules_ctx.get_mut::<GrpcWebBridge>() {
            bridge.init();
        }
    }

    Ok(())
}
