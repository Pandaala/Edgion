use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_proxy::Session;

#[inline]
fn apply_client_keepalive_timeout(session: &mut Session, keepalive_timeout: u64, process_shutting_down: bool) {
    // Preserve transport-level refusals such as ambiguous H1 framing. Setting
    // a timeout here must only narrow an already reusable session, never turn
    // a core-disabled connection back on.
    let keepalive = (!process_shutting_down && session.get_keepalive().is_some()).then_some(keepalive_timeout);
    session.set_keepalive(keepalive);
}

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

    // During normal service, configure only sessions that Pingora still
    // considers reusable. During graceful shutdown, disable reuse so clients
    // drain away from this instance.
    let process_shutting_down = session.is_process_shutting_down();
    apply_client_keepalive_timeout(session, client_timeout.keepalive_timeout, process_shutting_down);

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

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::CONTENT_LENGTH;
    use tokio::io::AsyncWriteExt;

    async fn session_from_request(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(raw_request)
            .await
            .expect("write request into duplex pair");
        drop(client);
        assert!(
            session.read_request().await.expect("request headers should parse"),
            "client should send a request before closing"
        );
        session
    }

    #[tokio::test]
    async fn normal_http11_session_uses_configured_keepalive_timeout() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        assert!(session.get_keepalive().is_some());

        apply_client_keepalive_timeout(&mut session, 37, false);

        assert_eq!(session.get_keepalive(), Some(37));
    }

    #[tokio::test]
    async fn transfer_encoding_with_content_length_stays_non_reusable() {
        let mut session = session_from_request(
            b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n0\r\n\r\n",
        )
        .await;
        assert!(session.req_header().headers.get(CONTENT_LENGTH).is_none());
        assert!(
            session.get_keepalive().is_none(),
            "Pingora core must disable reuse for Transfer-Encoding plus Content-Length"
        );

        apply_client_keepalive_timeout(&mut session, 37, false);

        assert!(
            session.get_keepalive().is_none(),
            "Edgion must preserve the transport-level reuse refusal"
        );
    }

    #[tokio::test]
    async fn http10_keepalive_requires_explicit_client_opt_in() {
        let mut default_close = session_from_request(b"GET / HTTP/1.0\r\nHost: test\r\n\r\n").await;
        assert!(default_close.get_keepalive().is_none());

        apply_client_keepalive_timeout(&mut default_close, 37, false);

        assert!(default_close.get_keepalive().is_none());

        let mut explicit_keepalive =
            session_from_request(b"GET / HTTP/1.0\r\nHost: test\r\nConnection: keep-alive\r\n\r\n").await;
        assert!(explicit_keepalive.get_keepalive().is_some());

        apply_client_keepalive_timeout(&mut explicit_keepalive, 37, false);

        assert_eq!(explicit_keepalive.get_keepalive(), Some(37));
    }

    #[tokio::test]
    async fn graceful_shutdown_disables_an_otherwise_reusable_session() {
        let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        assert!(session.get_keepalive().is_some());

        apply_client_keepalive_timeout(&mut session, 37, true);

        assert!(session.get_keepalive().is_none());
    }
}
