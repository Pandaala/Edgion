use super::content_type::classify_response_stream_kind;
use super::pg_init_downstream_modules::DEFAULT_COMPRESSION_LEVEL;
use super::pg_upstream_response_body_filter::enforce_stream_processor_framing;
use super::pg_upstream_response_filter::{is_interim_informational, response_denial_error, response_stage_settled};
use super::EdgionHttpProxy;
use crate::ctx::ResponseStreamKind;
use crate::EdgionHttpContext;
use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use pingora_core::modules::http::compression::ResponseCompression;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// Select compression only for a final response whose complete size is known.
///
/// Pingora's gzip encoder does not flush after each input chunk, so enabling it
/// for an unknown-length response can hold a low-rate stream indefinitely.
/// Multiple or combined `Content-Length` values are rejected conservatively;
/// any `Transfer-Encoding` also means the response is not known-length. The
/// compression module removes an accepted length before forwarding.
#[inline]
fn response_compression_level(enabled: bool, stream_kind: ResponseStreamKind, response: &ResponseHeader) -> u32 {
    if !enabled || stream_kind == ResponseStreamKind::Sse {
        return 0;
    }

    let mut content_lengths = response.headers.get_all(http::header::CONTENT_LENGTH).iter();
    let has_one_valid_content_length = content_lengths
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
        && content_lengths.next().is_none();

    let has_transfer_encoding = response.headers.contains_key(http::header::TRANSFER_ENCODING);

    if has_one_valid_content_length && !has_transfer_encoding {
        DEFAULT_COMPRESSION_LEVEL
    } else {
        0
    }
}

#[inline]
fn enable_compression(edgion_http: &EdgionHttpProxy) -> bool {
    edgion_http
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|server| server.enable_compression)
        .unwrap_or(false)
}

#[inline]
fn complete_local_reply_delivery(
    ctx: &mut EdgionHttpContext,
    body_result: pingora_core::Result<()>,
) -> pingora_core::Result<()> {
    match body_result {
        Ok(()) => {
            ctx.record_info(EdgionInfoCode::PluginResponseReplaced);
            Ok(())
        }
        Err(error) => {
            ctx.record_error(EdgionErrorCode::ClientClosed);
            Err(error)
        }
    }
}

/// Claim a staged response-phase reply before touching the downstream.
///
/// A plugin can surface a status outside the ordinary final-response range
/// through a lower-level runtime such as native WAF. Falling through after a
/// rejected claim would silently forward the upstream response, so every
/// rejected claim becomes non-retryable local control flow instead.
#[inline]
fn claim_response_local_reply(ctx: &mut EdgionHttpContext, status: u16) -> pingora_core::Result<()> {
    if ctx.claim_final_response(status) {
        return Ok(());
    }

    let mut error = pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(500),
        "invalid or duplicate edgion response local reply",
    );
    error.set_retry(false);
    Err(error)
}

#[inline]
pub async fn response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Mirror of the sync entry point's guard: an interim 1xx is not the response
    // the plugins are meant to see. Without it a plugin denial on a `100`/`103`
    // arms the response-stage abort — `apply_response_local_reply` refuses an
    // informational response, so `check_response_stage_terminate` takes the
    // refused branch — and kills an exchange whose real response has not arrived
    // yet. The final header runs the chains, so nothing is skipped, only
    // deferred.
    //
    // Deliberately BEFORE any header mutation and, like the sync side, without
    // stamping `ctx.request_info.status`: the access-log status must describe the
    // final response, not a 100 Continue.
    if is_interim_informational(upstream_response.status) {
        return Ok(());
    }

    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // RFC 9110 §7.6.1 forbids forwarding hop-by-hop `Connection` headers. Injecting
    // `Connection: close` during graceful shutdown is redundant — `set_keepalive(None)` in
    // pg_early_request_filter already makes Pingora's H1 codec emit it on the downstream wire.

    // A sync Phase-6 plugin may have already stashed a local reply. Do NOT
    // return here: fall through so the write+abort block below actually writes
    // the stashed reply. The group guards below (`is_some()` -> skip) prevent
    // Phase-7 plugins from running on top of a Phase-6 reply.

    if !response_stage_settled(ctx) {
        if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
            grpc_route_unit
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        } else if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit
                .plugin_runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    if !response_stage_settled(ctx) {
        if let Some(runtime) = ctx
            .selected_grpc_backend
            .as_ref()
            .map(crate::routes::grpc::SelectedGrpcBackend::plugin_runtime)
        {
            runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        } else if let Some(runtime) = ctx
            .selected_backend
            .as_ref()
            .map(crate::backends::SelectedHttpBackend::plugin_runtime)
        {
            runtime
                .run_upstream_response_plugins_async(session, ctx, upstream_response)
                .await;
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_gateway_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_async_plugins_count() == 0 {
                    continue;
                }
                runtime
                    .run_upstream_response_plugins_async(session, ctx, upstream_response)
                    .await;
            }
        }
    }

    // Run global plugins last (onion: response stages go inner → outer).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_global_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_async_plugins_count() == 0 {
                    continue;
                }
                runtime
                    .run_upstream_response_plugins_async(session, ctx, upstream_response)
                    .await;
            }
        }
    }

    // Async response plugins run after H1 made its framing choice. Remove a
    // stale length once more so they cannot produce CL+TE or incorrect H2
    // length metadata for a body-mutating processor.
    if !response_stage_settled(ctx) {
        enforce_stream_processor_framing(upstream_response, ctx);
    }

    // A denial no local reply could carry aborts before the forward path below.
    // Nothing has been written downstream yet, so `fail_to_proxy` still gets to
    // deliver the plugin's status.
    if let Some(denial) = ctx.response_denial() {
        return Err(response_denial_error(denial));
    }

    // filter-04b: a response-phase plugin stashed a local reply. Write it
    // directly to the downstream and abort the pump so the upstream response is
    // not forwarded. This is the single write+abort point; it handles both
    // body-less and body-present upstreams (Phase 8 is never reached).
    let pending_local_reply = ctx.take_pending_response_local_reply();
    if let Some((mut header, body)) = pending_local_reply {
        // Stamp the gateway Server identity onto the freshly-built reply header.
        edgion_http.server_header_opts.apply_to_response(&mut header);
        let body_empty = body.is_empty();
        let status = header.status.as_u16();
        claim_response_local_reply(ctx, status)?;
        // Writing this reply and returning the abort `Err` below tears down the
        // downstream connection. Disable keepalive first so Pingora's H1 codec
        // emits `Connection: close` on the wire (no-op semantics for H2): without
        // it the client sees a keepalive-eligible response, pools the connection,
        // and may race to reuse the already-closed socket on a later request,
        // surfacing as an intermittent transport-level send error.
        session.set_keepalive(None);
        ctx.request_info.trace_id.reconcile_response(&mut *header);
        session.write_response_header(header, body_empty).await?;
        // Mark immediately after the header commits and before awaiting a body.
        // Pingora may cancel this hook future if the duplex pump's other side
        // fails; the sentinel must already suppress a second response and
        // upstream-failure accounting in that cancellation window.
        ctx.mark_local_reply_written();
        let body_result = if body_empty {
            // Session::write_response_header does not propagate its end flag
            // to the H2 codec. Finish explicitly before the sentinel abort
            // drops the session, otherwise an empty reply ends with CANCEL.
            session.write_response_body(None, true).await
        } else {
            session.write_response_body(Some(body), true).await
        };
        complete_local_reply_delivery(ctx, body_result)?;
        return Err(pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "edgion response local reply",
        ));
    }

    // `response_filter` is the one response-header hook shared by origin and
    // cache-hit paths. Configure compression only here, after every Edgion
    // response plugin and framing repair, and immediately before Pingora runs
    // the downstream modules' response-header filters.
    ctx.response_stream_kind = classify_response_stream_kind(upstream_response);
    let eligibility = final_response_eligibility(
        &session.req_header().method,
        upstream_response.status,
        ctx.response_stream_kind,
    );
    if let Some(state) = ctx.ai_state_mut() {
        state.final_response_eligibility = Some(eligibility);
    }
    if let Some(compression_ctx) = session.downstream_modules_ctx.get_mut::<ResponseCompression>() {
        let level = response_compression_level(
            enable_compression(edgion_http),
            ctx.response_stream_kind,
            upstream_response,
        );
        compression_ctx.adjust_level(level);
    }
    ctx.request_info.trace_id.reconcile_response(upstream_response);
    ctx.select_final_downstream_status(upstream_response.status.as_u16());
    // The response onion and both framing-repair passes are complete. From
    // this point through body/trailer callbacks and logging, one request-local
    // driver owns the fixed processor set.
    ctx.freeze_response_processor_driver();
    Ok(())
}

fn final_response_eligibility(
    method: &http::Method,
    status: http::StatusCode,
    stream_kind: crate::ctx::ResponseStreamKind,
) -> crate::ai::terminal::FinalResponseEligibility {
    crate::ai::terminal::FinalResponseEligibility {
        method_allows_body: method != http::Method::HEAD,
        status_allows_body: !status.is_informational() && !matches!(status.as_u16(), 204 | 304),
        stream_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        claim_response_local_reply, complete_local_reply_delivery, final_response_eligibility,
        response_compression_level, response_filter, EdgionHttpProxy, DEFAULT_COMPRESSION_LEVEL,
    };
    use crate::ctx::ResponseStreamKind;
    use crate::observe::AccessLogger;
    use crate::observe::RequestDiagnostic;
    use crate::routes::grpc::GrpcPortRouteManager;
    use crate::routes::http::routes_mgr::HttpPortRouteManager;
    use crate::EdgionHttpContext;
    use edgion_resources::{EdgionErrorCode, EdgionGatewayConfig, EdgionInfoCode, ListenerKey};
    use pingora_core::protocols::http::compression::ResponseCompressionCtx;
    use pingora_core::{Error, ErrorType};
    use pingora_http::{RequestHeader, ResponseHeader};
    use pingora_proxy::Session;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    fn proxy() -> EdgionHttpProxy {
        EdgionHttpProxy {
            listener_port: 80,
            is_https_listener: false,
            listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
            server_header_opts: Default::default(),
            access_logger: Arc::new(AccessLogger::default()),
            edgion_gateway_config: Arc::new(EdgionGatewayConfig::new("test", Default::default())),
            request_body_config_name: None,
            parsed_timeouts: Default::default(),
            enable_http2: false,
            real_ip_extractor: None,
            preflight_handler: super::super::PreflightHandler::new(None),
            http_route_manager: Arc::new(HttpPortRouteManager::default()),
            grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
        }
    }

    async fn h1_session() -> (tokio::io::DuplexStream, Session) {
        let (mut client, server) = tokio::io::duplex(4096);
        client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        session.read_request().await.unwrap();
        (client, session)
    }

    #[tokio::test]
    async fn empty_local_reply_h1_preserves_close_and_framing() {
        use std::time::Duration;
        use tokio::io::AsyncReadExt;

        for method in [http::Method::GET, http::Method::HEAD] {
            let (mut client, mut session) = h1_session().await;
            session.req_header_mut().set_method(method);
            let mut ctx = EdgionHttpContext::default();
            let mut reply = ResponseHeader::build(502, None).unwrap();
            reply.insert_header("content-length", "0").unwrap();
            ctx.stage_response_local_reply(Box::new(reply), bytes::Bytes::new());
            let mut upstream = ResponseHeader::build(200, None).unwrap();
            upstream.insert_header("x-upstream-private", "discard").unwrap();
            response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
                .await
                .expect_err("the local response stops upstream forwarding");
            assert_eq!(ctx.final_downstream_status(), Some(502));
            assert_eq!(
                ctx.diagnostics.entries(),
                &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
            );
            drop(session);
            let mut wire = String::new();
            tokio::time::timeout(Duration::from_secs(2), client.read_to_string(&mut wire))
                .await
                .unwrap()
                .unwrap();
            let (headers, body) = wire.split_once("\r\n\r\n").unwrap();
            let headers = headers.to_ascii_lowercase();
            assert!(headers.starts_with("http/1.1 502"));
            assert!(headers.contains("connection: close\r\n") || headers.ends_with("connection: close"));
            assert!(headers.contains("content-length: 0"));
            assert!(!headers.contains("transfer-encoding"));
            assert!(!headers.contains("x-upstream-private"));
            assert!(body.is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dsl_terminate_h2_local_reply_finishes_before_session_drop() {
        use crate::plugins::runtime::PluginRuntime;
        use crate::PendingHeaderOp;
        use edgion_dsl::validator::{compile_dsl_source, ValidationLimits};
        use edgion_resources::{DslConfig, EdgionPlugin, UpstreamResponseFilterEntry};
        use pingora_core::protocols::http::v2::server::{handshake, H2Accept, HttpSession as H2Session};
        use pingora_core::protocols::Digest;
        use std::time::Duration;

        for bytecode in [false, true] {
            for method in [http::Method::GET, http::Method::HEAD] {
                tokio::time::timeout(Duration::from_secs(5), async {
                    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
                    let client = tokio::spawn(async move {
                        let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
                        let mut drivers = tokio::task::JoinSet::new();
                        drivers.spawn(connection);
                        sender = sender.ready().await.unwrap();
                        let request = http::Request::builder()
                            .method(method)
                            .uri("https://example.test/terminate")
                            .body(())
                            .unwrap();
                        let (response, _) = sender.send_request(request, true).unwrap();
                        let response = response.await.expect("local reply headers arrive without reset");
                        assert_eq!(response.status(), http::StatusCode::BAD_GATEWAY);
                        assert_eq!(response.headers()["x-queued"], "retained");
                        assert!(!response.headers().contains_key("x-upstream-private"));
                        assert!(!response.headers().contains_key("x-bound"));
                        assert!(!response.headers().contains_key("x-after-terminate"));
                        let mut body = response.into_body();
                        while let Some(chunk) = body.data().await {
                            assert!(chunk.expect("empty reply must end cleanly, not reset").is_empty());
                        }
                        assert!(body.trailers().await.unwrap().is_none());
                        assert!(body.is_end_stream());
                        drop(body);
                        drop(sender);
                        // Only stop the client driver after observing the actual EOS.
                        drivers.shutdown().await;
                    });

                    let mut connection = handshake(Box::new(server_io), None).await.unwrap();
                    let digest = Arc::new(Digest::default());
                    let Some(H2Accept::Session(h2_session)) =
                        H2Session::from_h2_conn(&mut connection, Arc::clone(&digest))
                            .await
                            .unwrap()
                    else {
                        panic!("H2 request reaches the response-stage fixture");
                    };
                    // The proxy exposes only an H1 test constructor. Replace its
                    // public downstream with the real in-memory H2 session.
                    let (_unused_client, placeholder) = tokio::io::duplex(1);
                    let mut session = Session::new_h1(Box::new(placeholder));
                    session.downstream_session =
                        Box::new(pingora_core::protocols::http::server::Session::new_http2(h2_session));
                    let source = "resp.set_header(\"x-bound\", \"discard\")\nreturn terminate()";
                    let config = DslConfig {
                        name: "terminate-contract".to_string(),
                        source: (!bytecode).then(|| source.to_string()),
                        bytecode: bytecode.then(|| compile_dsl_source(source, &ValidationLimits::default()).unwrap()),
                        ..Default::default()
                    };
                    let mut runtime = PluginRuntime::new();
                    let entries = [
                        UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(config)),
                        UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                            name: "after-terminate".to_string(),
                            source: Some("resp.set_header(\"x-after-terminate\", \"unexpected\")".to_string()),
                            ..Default::default()
                        })),
                    ];
                    let errors = runtime.add_from_upstream_response_filters(&entries, "test");
                    assert!(errors.is_empty(), "{errors:?}");
                    let mut ctx = EdgionHttpContext::default();
                    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                        name: "x-queued".to_string(),
                        value: http::HeaderValue::from_static("retained"),
                    });
                    let mut upstream = ResponseHeader::build(200, None).unwrap();
                    upstream.insert_header("x-upstream-private", "discard").unwrap();
                    runtime.run_upstream_response_plugins_sync(&mut session, &mut ctx, &mut upstream);
                    assert_eq!(ctx.stage_logs()[0].filters.len(), 1);
                    let proxy = proxy();
                    let error = response_filter(&proxy, &mut session, &mut upstream, &mut ctx)
                        .await
                        .expect_err("local replacement stops the upstream pump");
                    assert!(ctx.local_reply_written());
                    assert_eq!(ctx.final_downstream_status(), Some(502));
                    assert_eq!(
                        ctx.diagnostics.entries(),
                        &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
                    );
                    let outcome =
                        super::super::pg_fail_to_proxy::fail_to_proxy(&proxy, &mut session, &error, &mut ctx).await;
                    assert_eq!(outcome.error_code, 502);
                    // Match the post-pump abort path: no downstream.finish() here.
                    drop(session);
                    let (client_result, connection_result) =
                        tokio::join!(client, H2Session::from_h2_conn(&mut connection, digest));
                    client_result.unwrap();
                    assert!(connection_result.unwrap().is_none());
                })
                .await
                .expect("H2 local reply reaches EOS within the test deadline");
            }
        }
    }

    fn response(content_length: Option<&str>) -> ResponseHeader {
        let mut response = ResponseHeader::build(200, None).unwrap();
        response.insert_header("content-type", "application/json").unwrap();
        if let Some(content_length) = content_length {
            response.insert_header("content-length", content_length).unwrap();
        }
        response
    }

    #[test]
    fn local_reply_body_failure_records_client_closed_and_preserves_status() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.claim_final_response(403));
        ctx.mark_local_reply_written();

        let result = complete_local_reply_delivery(&mut ctx, Err(Error::new(ErrorType::WriteError)));

        assert!(result.is_err());
        assert!(ctx.local_reply_written());
        assert_eq!(ctx.final_downstream_status(), Some(403));
        assert_eq!(
            ctx.diagnostics.entries(),
            &[RequestDiagnostic::Error(EdgionErrorCode::ClientClosed)]
        );
    }

    #[test]
    fn successful_local_reply_records_replacement_once() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.claim_final_response(403));
        ctx.mark_local_reply_written();

        complete_local_reply_delivery(&mut ctx, Ok(())).unwrap();

        assert!(ctx.local_reply_written());
        assert_eq!(ctx.final_downstream_status(), Some(403));
        assert_eq!(
            ctx.diagnostics.entries(),
            &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
        );
    }

    #[test]
    fn valid_response_local_reply_statuses_claim_delivery() {
        for status in [200, 599] {
            let mut ctx = EdgionHttpContext::default();

            claim_response_local_reply(&mut ctx, status).unwrap();

            assert!(ctx.response_write_started(), "status {status}");
            assert_eq!(ctx.final_downstream_status(), Some(status), "status {status}");
            assert!(!ctx.local_reply_written(), "status {status}");
            assert!(ctx.diagnostics.entries().is_empty(), "status {status}");
        }
    }

    #[test]
    fn invalid_response_local_reply_statuses_fail_closed_without_delivery_state() {
        for status in [100, 101, 199, 600] {
            let mut ctx = EdgionHttpContext::default();

            let error = claim_response_local_reply(&mut ctx, status).unwrap_err();

            assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)), "status {status}");
            assert!(!error.retry(), "status {status}");
            assert!(!ctx.response_write_started(), "status {status}");
            assert_eq!(ctx.final_downstream_status(), None, "status {status}");
            assert!(!ctx.local_reply_written(), "status {status}");
            assert!(ctx.diagnostics.entries().is_empty(), "status {status}");
        }
    }

    #[test]
    fn already_claimed_response_rejects_local_reply_and_preserves_first_owner() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.claim_final_response(204));

        let error = claim_response_local_reply(&mut ctx, 403).unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), Some(204));
        assert!(!ctx.local_reply_written());
        assert!(ctx.diagnostics.entries().is_empty());
    }

    #[tokio::test]
    async fn response_filter_rejects_informational_local_reply_before_downstream_write() {
        let (_client, mut session) = h1_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.stage_response_local_reply(Box::new(ResponseHeader::build(101, None).unwrap()), bytes::Bytes::new());
        let mut upstream = ResponseHeader::build(200, None).unwrap();

        let error = response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
            .await
            .unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(session.response_written().is_none());
        assert!(!ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), None);
        assert!(!ctx.local_reply_written());
        assert!(ctx.diagnostics.entries().is_empty());
    }

    #[tokio::test]
    async fn response_filter_rejects_pending_reply_after_an_existing_writer() {
        let (_client, mut session) = h1_session().await;
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.claim_final_response(204));
        session
            .write_response_header(Box::new(ResponseHeader::build(204, None).unwrap()), true)
            .await
            .unwrap();
        ctx.stage_response_local_reply(
            Box::new(ResponseHeader::build(403, None).unwrap()),
            bytes::Bytes::from_static(b"denied"),
        );
        let mut upstream = ResponseHeader::build(200, None).unwrap();

        let error = response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
            .await
            .unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert_eq!(
            session.response_written().map(|header| header.status.as_u16()),
            Some(204)
        );
        assert!(ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), Some(204));
        assert!(!ctx.local_reply_written());
        assert!(ctx.diagnostics.entries().is_empty());
    }

    #[test]
    fn final_response_eligibility_uses_live_method_status_and_stream_kind() {
        for status in [
            http::StatusCode::CONTINUE,
            http::StatusCode::NO_CONTENT,
            http::StatusCode::NOT_MODIFIED,
        ] {
            assert!(!final_response_eligibility(&http::Method::GET, status, ResponseStreamKind::Sse).allows_sse());
        }
        assert!(
            !final_response_eligibility(&http::Method::HEAD, http::StatusCode::OK, ResponseStreamKind::Sse)
                .allows_sse()
        );
        assert!(
            final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::Sse).allows_sse()
        );
        assert!(
            !final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::None)
                .allows_sse()
        );
    }

    #[test]
    fn compression_requires_one_valid_content_length() {
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(Some("128"))),
            DEFAULT_COMPRESSION_LEVEL
        );
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(None)),
            0
        );
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response(Some("invalid"))),
            0
        );

        let mut duplicate = response(Some("128"));
        duplicate.append_header("content-length", "128").unwrap();
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &duplicate),
            0
        );

        let mut conflicting_framing = response(Some("128"));
        conflicting_framing
            .insert_header("transfer-encoding", "chunked")
            .unwrap();
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &conflicting_framing),
            0
        );
    }

    #[test]
    fn final_header_mutation_can_disable_compression() {
        let mut response = response(Some("128"));
        assert_eq!(
            response_compression_level(true, ResponseStreamKind::None, &response),
            DEFAULT_COMPRESSION_LEVEL
        );

        response.remove_header("content-length");
        assert_eq!(response_compression_level(true, ResponseStreamKind::None, &response), 0);
    }

    #[test]
    fn compression_respects_config_and_sse_exemption() {
        let mut response = response(Some("128"));
        response.append_header("content-length", "invalid").unwrap();
        response.insert_header("transfer-encoding", "chunked").unwrap();
        assert_eq!(
            response_compression_level(false, ResponseStreamKind::None, &response),
            0
        );
        assert_eq!(response_compression_level(true, ResponseStreamKind::Sse, &response), 0);
    }

    #[test]
    fn enabled_request_negotiation_survives_final_level_selection() {
        let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header("accept-encoding", "gzip").unwrap();
        compression.request_filter(&request);

        let mut response = response(Some("128"));
        compression.adjust_level(response_compression_level(true, ResponseStreamKind::None, &response));
        compression.response_header_filter(&mut response, false);

        assert_eq!(response.headers.get("content-encoding").unwrap(), "gzip");
        assert!(response.headers.get("content-length").is_none());
    }

    #[test]
    fn final_unknown_length_selection_disables_negotiated_compression() {
        let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header("accept-encoding", "gzip").unwrap();
        compression.request_filter(&request);

        let mut response = response(None);
        compression.adjust_level(response_compression_level(true, ResponseStreamKind::None, &response));
        compression.response_header_filter(&mut response, false);

        assert!(response.headers.get("content-encoding").is_none());
    }
}
