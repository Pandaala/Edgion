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
fn classify_and_initialize_grpc_web(session: &mut Session, ctx: &mut EdgionHttpContext) {
    super::pg_request_filter::detect_request_protocol(session.req_header(), ctx);
    if ctx.request_info.discover_protocol.as_deref() == Some("grpc-web") {
        if let Some(bridge) = session.downstream_modules_ctx.get_mut::<GrpcWebBridge>() {
            bridge.init();
        }
    }
}

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
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

    // Freeze protocol classification from the raw downstream headers before
    // Pingora's built-in request_header_filter rewrites gRPC-Web to gRPC. Only
    // the canonical gRPC-Web classification activates the bridge, preserving
    // WebSocket precedence and leaving native gRPC and ordinary HTTP inert.
    classify_and_initialize_grpc_web(session, ctx);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::CONTENT_LENGTH;
    use pingora_core::modules::http::grpc_web::GrpcWeb;
    use pingora_core::modules::http::HttpModules;
    use pingora_core::protocols::http::bridge::grpc_web::GrpcWebCtx;
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

    async fn session_from_request_with_grpc_web(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut modules = HttpModules::new();
        modules.add_module(Box::new(GrpcWeb));
        let mut session = Session::new_h1_with_modules(stream, &modules);
        client
            .write_all(raw_request)
            .await
            .expect("write request into duplex pair");
        drop(client);
        assert!(session.read_request().await.expect("request headers should parse"));
        session
    }

    async fn run_module_request_header_filter(session: &mut Session) {
        let req = session.downstream_session.req_header_mut();
        session
            .downstream_modules_ctx
            .request_header_filter(req)
            .await
            .expect("module request header filter");
    }

    #[tokio::test]
    async fn raw_grpc_web_classification_survives_module_rewrite() {
        let mut session = session_from_request_with_grpc_web(
            b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Type: application/grpc-web+proto\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();

        classify_and_initialize_grpc_web(&mut session, &mut ctx);
        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc-web"));
        assert_eq!(
            **session
                .downstream_modules_ctx
                .get::<GrpcWebBridge>()
                .expect("gRPC-Web bridge module"),
            GrpcWebCtx::Init
        );

        run_module_request_header_filter(&mut session).await;
        assert_eq!(
            session.req_header().headers.get(http::header::CONTENT_TYPE).unwrap(),
            "application/grpc+proto"
        );
        assert_eq!(
            **session
                .downstream_modules_ctx
                .get::<GrpcWebBridge>()
                .expect("gRPC-Web bridge module"),
            GrpcWebCtx::Upgrade
        );
        super::super::pg_request_filter::detect_request_protocol(session.req_header(), &mut ctx);

        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc-web"));
        assert!(ctx.request_info.is_grpc_request);
        assert!(!super::super::pg_request_filter::native_grpc_requires_frontend_http2(
            ctx.request_info.discover_protocol.as_deref(),
            false,
        ));
    }

    #[tokio::test]
    async fn websocket_precedence_keeps_grpc_web_bridge_disabled() {
        let mut session = session_from_request_with_grpc_web(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nContent-Type: application/grpc-web+proto\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();

        classify_and_initialize_grpc_web(&mut session, &mut ctx);
        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
        assert_eq!(
            **session
                .downstream_modules_ctx
                .get::<GrpcWebBridge>()
                .expect("gRPC-Web bridge module"),
            GrpcWebCtx::Disabled
        );

        run_module_request_header_filter(&mut session).await;
        assert_eq!(
            session.req_header().headers.get(http::header::CONTENT_TYPE).unwrap(),
            "application/grpc-web+proto"
        );
        assert!(!super::super::pg_request_filter::native_grpc_requires_frontend_http2(
            ctx.request_info.discover_protocol.as_deref(),
            false,
        ));
    }

    #[tokio::test]
    async fn native_grpc_stays_inert_and_requires_frontend_http2() {
        let mut session = session_from_request_with_grpc_web(
            b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Type: application/grpc\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();

        classify_and_initialize_grpc_web(&mut session, &mut ctx);

        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc"));
        assert_eq!(
            **session
                .downstream_modules_ctx
                .get::<GrpcWebBridge>()
                .expect("gRPC-Web bridge module"),
            GrpcWebCtx::Disabled
        );
        assert!(super::super::pg_request_filter::native_grpc_requires_frontend_http2(
            ctx.request_info.discover_protocol.as_deref(),
            false,
        ));
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
