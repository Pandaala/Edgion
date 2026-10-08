use super::*;
use crate::observe::RequestDiagnostic;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::DslConfig;
use edgion_resources::resources::{EdgionPlugin, KeyAuthConfig, RequestFilterEntry};
use edgion_resources::EdgionErrorCode;
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;

fn compile_scoped_request_entries(
    entries: Vec<RequestFilterEntry>,
    deny: serde_json::Value,
) -> Arc<crate::plugins::http::CompiledPlugins> {
    use crate::plugins::http::plugin_generation::PluginGeneration;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec};
    use edgion_resources::resources::plugin_policy::PluginPolicyConfig;
    let policy: PluginPolicyConfig = serde_json::from_value(serde_json::json!({"deny": deny})).unwrap();
    PluginGeneration::new(EdgionPlugins::new(
        "scoped-test",
        EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(entries),
            ..Default::default()
        },
    ))
    .compile(Some(&policy.compile().unwrap()))
}

async fn session_from_request(raw_request: &[u8]) -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client.write_all(raw_request).await.unwrap();
    drop(client);
    assert!(session.read_request().await.unwrap());
    session
}

#[test]
fn elapsed_micros_saturates_a_future_start_to_zero() {
    let started = Instant::now()
        .checked_add(Duration::from_secs(60))
        .expect("adding one minute to the current instant must remain representable");

    assert_eq!(elapsed_micros(started), 0);
}

struct FixedResultFilter(PluginRunningResult);

struct GraphqlLocalErrorFilter;

struct QueuedPlainHeaderFilter;

#[async_trait]
impl RequestFilter for QueuedPlainHeaderFilter {
    fn name(&self) -> &str {
        "QueuedPlainHeader"
    }
    async fn run_request(&self, session: &mut dyn PluginSession, _plugin_log: &mut PluginLog) -> PluginRunningResult {
        session.set_response_header("Content-Type", "text/plain").unwrap();
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl RequestFilter for GraphqlLocalErrorFilter {
    fn name(&self) -> &str {
        "GraphqlLocalError"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, _plugin_log: &mut PluginLog) -> PluginRunningResult {
        assert!(session.set_response_header("Content-Length", "999").is_err());
        assert!(session.set_response_header("Transfer-Encoding", "chunked").is_err());
        crate::plugins::http::common::graphql_request::reject(session, 400)
    }
}

#[async_trait]
impl RequestFilter for FixedResultFilter {
    fn name(&self) -> &str {
        "FixedResult"
    }

    async fn run_request(&self, _session: &mut dyn PluginSession, _plugin_log: &mut PluginLog) -> PluginRunningResult {
        self.0.clone()
    }
}

struct SelfWritingFilter {
    client: Mutex<Option<tokio::io::DuplexStream>>,
}

#[async_trait]
impl RequestFilter for SelfWritingFilter {
    fn name(&self) -> &str {
        "SelfWriting"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, _plugin_log: &mut PluginLog) -> PluginRunningResult {
        let mut response = ResponseHeader::build(401, None).unwrap();
        response.insert_header("content-length", "6").unwrap();
        session
            .write_response_header(Box::new(response), false)
            .await
            .expect("header should be delivered before the test closes the client");
        drop(self.client.lock().take());
        let _ = session
            .write_response_body(Some(bytes::Bytes::from_static(b"denied")), true)
            .await;
        PluginRunningResult::ErrTerminateRequest
    }
}

async fn plugin_test_session() -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"GET /plugin-test HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    (session, client)
}

fn client_closed_count(ctx: &EdgionHttpContext) -> usize {
    ctx.diagnostics
        .entries()
        .iter()
        .filter(|entry| **entry == RequestDiagnostic::Error(EdgionErrorCode::ClientClosed))
        .count()
}

fn terminate_dsl_config(bytecode: bool) -> DslConfig {
    use edgion_dsl::validator::{compile_dsl_source, ValidationLimits};
    let source = r#"
            resp.set_header("x-terminate", "selected")
            return terminate()
        "#;
    DslConfig {
        name: "terminate-contract".to_string(),
        source: (!bytecode).then(|| source.to_string()),
        bytecode: bytecode.then(|| compile_dsl_source(source, &ValidationLimits::default()).unwrap()),
        // The default Ignore policy must not turn an explicit rejection into a pass.
        ..Default::default()
    }
}

fn after_terminate_dsl_config() -> DslConfig {
    DslConfig {
        name: "after-terminate".to_string(),
        source: Some(r#"resp.set_header("x-after-terminate", "unexpected")"#.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn dsl_terminate_request_writes_empty_502_without_fallback() {
    use crate::runtime::server::{terminate_with_fallback, ServerHeaderOpts};
    use crate::PendingHeaderOp;
    for bytecode in [false, true] {
        for method in [http::Method::GET, http::Method::HEAD] {
            let (mut session, mut client) = plugin_test_session().await;
            session.req_header_mut().set_method(method);
            let mut ctx = EdgionHttpContext::default();
            for (name, value) in [("content-length", "999"), ("transfer-encoding", "chunked")] {
                ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                    name: name.to_string(),
                    value: http::HeaderValue::from_static(value),
                });
            }
            let mut runtime = PluginRuntime::new();
            runtime.add_request_filter(DslPlugin::from_config(&terminate_dsl_config(bytecode)).unwrap());
            runtime.add_request_filter(DslPlugin::from_config(&after_terminate_dsl_config()).unwrap());
            runtime.run_request_plugins(&mut session, &mut ctx).await;
            assert!(ctx.plugin_terminated());
            assert_eq!(ctx.final_downstream_status(), Some(502));
            // The proxy invokes this after a stopped request chain. A legitimate
            // local response already owns the reply, so this must be a no-op.
            terminate_with_fallback(&mut session, &mut ctx, &ServerHeaderOpts::default())
                .await
                .unwrap();
            assert!(!ctx
                .diagnostics
                .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
            let stages = ctx.stage_logs();
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].filters.len(), 1);
            assert_eq!(stages[0].filters[0].render_for_test(), "Deny 502");
            drop(session);
            let raw = String::from_utf8(read_client_response(&mut client).await).unwrap();
            let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
            let headers = headers.to_ascii_lowercase();
            assert!(headers.starts_with("http/1.1 502"), "{raw}");
            assert!(headers.contains("content-length: 0"), "{raw}");
            assert!(!headers.contains("transfer-encoding"), "{raw}");
            assert!(headers.contains("content-type: text/plain"), "{raw}");
            assert!(headers.contains("x-terminate: selected"), "{raw}");
            assert!(!headers.contains("x-after-terminate"), "{raw}");
            assert!(body.is_empty(), "{raw}");
        }
    }
}

/// The in-memory H1 session exercises the gRPC writer's status mapping;
/// actual H2 trailer interoperability is tracked by the E2E follow-up.
#[tokio::test]
async fn dsl_terminate_request_maps_grpc_without_fallback() {
    for bytecode in [false, true] {
        for route_only in [false, true] {
            let (mut session, mut client) = plugin_test_session().await;
            let mut ctx = EdgionHttpContext::default();
            ctx.request_info.is_grpc_request = !route_only;
            ctx.is_grpc_route_matched = route_only;
            let mut runtime = PluginRuntime::new();
            runtime.add_request_filter(DslPlugin::from_config(&terminate_dsl_config(bytecode)).unwrap());
            runtime.add_request_filter(DslPlugin::from_config(&after_terminate_dsl_config()).unwrap());
            runtime.run_request_plugins(&mut session, &mut ctx).await;
            assert!(ctx.plugin_terminated());
            assert_eq!(ctx.final_downstream_status(), Some(200));
            assert_eq!(ctx.request_info.grpc_status(), Some(13));
            assert!(!ctx
                .diagnostics
                .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
            assert_eq!(ctx.stage_logs()[0].filters.len(), 1);
            drop(session);
            let raw = String::from_utf8(read_client_response(&mut client).await).unwrap();
            let raw = raw.to_ascii_lowercase();
            assert!(raw.starts_with("http/1.1 200"), "{raw}");
            assert!(raw.contains("grpc-status: 13"), "{raw}");
            assert!(raw.contains("grpc-message: internal"), "{raw}");
            assert!(raw.contains("x-terminate: selected"), "{raw}");
            assert!(!raw.contains("x-after-terminate"), "{raw}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dsl_terminate_response_stages_empty_local_reply() {
    use crate::PendingHeaderOp;
    for bytecode in [false, true] {
        for (method, upstream_status) in [
            (http::Method::GET, 200),
            (http::Method::HEAD, 200),
            (http::Method::GET, 204),
            (http::Method::CONNECT, 403),
        ] {
            let (mut session, _client) = plugin_test_session().await;
            session.req_header_mut().set_method(method.clone());
            // A request-side upgrade intent alone must not prevent replacement.
            session.req_header_mut().insert_header("upgrade", "websocket").unwrap();
            let mut ctx = EdgionHttpContext::default();
            ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                name: "x-queued".to_string(),
                value: http::HeaderValue::from_static("retained"),
            });
            if method != http::Method::HEAD {
                for (name, value) in [("content-length", "999"), ("transfer-encoding", "chunked")] {
                    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                        name: name.to_string(),
                        value: http::HeaderValue::from_static(value),
                    });
                }
            }
            let mut upstream = ResponseHeader::build(upstream_status, None).unwrap();
            upstream.insert_header("x-upstream-private", "discard").unwrap();
            let mut runtime = PluginRuntime::new();
            runtime.add_upstream_response_filter(
                DslPlugin::from_config_response(&terminate_dsl_config(bytecode)).unwrap(),
            );
            runtime
                .add_upstream_response_filter(DslPlugin::from_config_response(&after_terminate_dsl_config()).unwrap());
            runtime.run_upstream_response_plugins_sync(&mut session, &mut ctx, &mut upstream);
            assert!(ctx.plugin_terminated());
            assert_eq!(ctx.response_denial(), None);
            assert!(!ctx
                .diagnostics
                .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
            assert_eq!(ctx.stage_logs()[0].filters.len(), 1);
            let log = ctx.stage_logs()[0].filters[0].render_for_test();
            assert_eq!(log.matches("Deny 502").count(), 1);
            assert!(log.contains("replace response 502"));
            let (reply, body) = ctx.take_pending_response_local_reply().expect("local reply staged");
            assert_eq!(reply.status.as_u16(), 502);
            assert!(body.is_empty());
            // Response-phase resp.set_header edits the bound upstream header.
            // Only queued request-phase operations survive local replacement.
            assert_eq!(upstream.headers["x-terminate"], "selected");
            assert!(!upstream.headers.contains_key("x-after-terminate"));
            assert!(!reply.headers.contains_key("x-terminate"));
            assert_eq!(reply.headers["x-queued"], "retained");
            assert!(!reply.headers.contains_key("x-upstream-private"));
            assert!(!reply.headers.contains_key("x-after-terminate"));
            if method != http::Method::HEAD {
                assert_eq!(reply.headers["content-length"], "0");
                assert!(!reply.headers.contains_key("transfer-encoding"));
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dsl_terminate_response_refusal_arms_denial_without_plugin_bug() {
    use crate::ResponseStageDenial;
    for (method, upstream_status, grpc_request, grpc_route) in [
        (http::Method::GET, 101, false, false),
        (http::Method::CONNECT, 200, false, false),
        (http::Method::POST, 200, true, false),
        (http::Method::POST, 200, false, true),
    ] {
        let (mut session, _client) = plugin_test_session().await;
        session.req_header_mut().set_method(method);
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = grpc_request;
        ctx.is_grpc_route_matched = grpc_route;
        let mut upstream = ResponseHeader::build(upstream_status, None).unwrap();
        let mut runtime = PluginRuntime::new();
        runtime.add_upstream_response_filter(DslPlugin::from_config_response(&terminate_dsl_config(false)).unwrap());
        runtime.add_upstream_response_filter(DslPlugin::from_config_response(&after_terminate_dsl_config()).unwrap());
        runtime.run_upstream_response_plugins_sync(&mut session, &mut ctx, &mut upstream);
        assert!(ctx.plugin_terminated());
        assert!(ctx.take_pending_response_local_reply().is_none());
        assert_eq!(ctx.response_denial(), Some(&ResponseStageDenial::Http(502)));
        assert!(!ctx
            .diagnostics
            .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::PluginResponseDenyAborted));
        assert_eq!(
            ctx.diagnostics
                .contains_error(EdgionErrorCode::PluginResponseReplaceUnsupportedGrpc),
            grpc_request || grpc_route
        );
        assert_eq!(ctx.stage_logs()[0].filters.len(), 1);
    }
}

#[tokio::test]
async fn err_response_writer_failures_record_client_closed_once() {
    let (mut session, client) = plugin_test_session().await;
    drop(client);
    let mut ctx = EdgionHttpContext::default();
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
        status: 403,
        body: Some(Bytes::from_static(b"denied")),
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert_eq!(ctx.final_downstream_status(), Some(403));
    assert_eq!(client_closed_count(&ctx), 1);
}

#[tokio::test]
async fn graphql_local_error_uses_http_json_framing_and_preserves_native_grpc_owner() {
    for grpc in [false, true] {
        let (mut session, mut client) = plugin_test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = grpc;
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(GraphqlLocalErrorFilter));
        runtime.run_request_plugins(&mut session, &mut ctx).await;
        let bytes = read_client_response(&mut client).await;
        let raw = String::from_utf8(bytes).unwrap();
        let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
        let headers = headers.to_ascii_lowercase();
        assert!(!headers.contains("content-length: 999"));
        assert!(!headers.contains("transfer-encoding:"));
        if grpc {
            assert!(headers.starts_with("http/1.1 200"));
            assert!(headers.contains("content-type: application/grpc"));
            assert!(!headers.contains("content-type: application/json"));
            assert!(headers.contains("grpc-status:"));
            assert!(body.is_empty());
        } else {
            assert!(headers.starts_with("http/1.1 400"));
            assert!(headers.contains("content-type: application/json"));
            assert!(headers.contains(&format!("content-length: {}", body.len())));
            assert_eq!(
                body.as_bytes(),
                crate::plugins::http::common::graphql_request::error_body(400).as_ref()
            );
        }
    }
}

#[tokio::test]
async fn graphql_limiter_json_rejection_overrides_earlier_queued_type_without_changing_ordinary_limiter() {
    use crate::plugins::http::common::graphql_request;
    use crate::plugins::http::rate_limit_redis::RateLimitRedis;
    use edgion_resources::plugin_configs::RateLimitRedisConfig;
    let config: RateLimitRedisConfig = serde_json::from_value(serde_json::json!({
        "redisRef":"default/unused-graphql-header-test",
        "key":[{"type":"header","name":"x-missing-identity"}],
        "policies":[{"rate":100,"interval":"1s"}],
        "onKeyMiss":"Deny"
    }))
    .unwrap();
    for graphql in [false, true] {
        let (mut session, mut client) = plugin_test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut runtime = PluginRuntime::new();
        runtime.add_request_filter(Box::new(QueuedPlainHeaderFilter));
        let limiter = RateLimitRedis::new(&config, "default/header-test/#0");
        let limiter = if graphql {
            limiter.with_json_rejection_body(graphql_request::error_body(429))
        } else {
            limiter
        };
        runtime.add_request_filter(Box::new(limiter));
        runtime.run_request_plugins(&mut session, &mut ctx).await;
        let raw = String::from_utf8(read_client_response(&mut client).await).unwrap();
        let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
        let headers = headers.to_ascii_lowercase();
        assert!(headers.starts_with("http/1.1 429"));
        assert!(!headers.contains("retry-after:"));
        if graphql {
            assert!(headers.contains("content-type: application/json"));
            assert!(body.contains(std::str::from_utf8(&graphql_request::error_body(429)).unwrap()));
        } else {
            assert!(headers.contains("content-type: text/plain"));
            assert!(body.contains("Rate limit exceeded"));
        }
    }
}

#[tokio::test]
async fn invalid_request_plugin_reply_uses_a_final_500() {
    let (mut session, mut client) = plugin_test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
        status: 100,
        body: None,
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert_eq!(ctx.final_downstream_status(), Some(500));
    assert!(String::from_utf8_lossy(&read_client_response(&mut client).await).starts_with("HTTP/1.1 500"));
}

#[tokio::test]
async fn failed_request_reply_claim_does_not_write_a_body() {
    let (mut session, _client) = plugin_test_session().await;
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(204));
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
        status: 403,
        body: Some(Bytes::from_static(b"denied")),
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert!(session.response_written().is_none());
    assert_eq!(ctx.final_downstream_status(), Some(204));
}

/// Read whatever the gateway wrote to the duplex client end. The gRPC
/// trailers-only reply is a single HTTP/1 header block, so the raw bytes
/// are enough to assert what reached the wire.
async fn read_client_response(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(1), client.read(&mut buf))
        .await
        .expect("gateway wrote a response")
        .expect("read succeeds");
    buf.truncate(n);
    buf
}

/// A UTF-8 `ErrResponse` body on a gRPC route is carried verbatim as
/// `grpc-message`, exactly as before the body became raw `Bytes`.
#[tokio::test]
async fn err_response_on_grpc_route_carries_utf8_body_as_message() {
    let (mut session, mut client) = plugin_test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
        status: 403,
        body: Some(Bytes::from_static(b"tenant quota exhausted")),
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;
    let raw = String::from_utf8_lossy(&read_client_response(&mut client).await).into_owned();

    assert_eq!(ctx.request_info.grpc_status(), Some(http_to_grpc_status(403).0));
    assert!(
        raw.contains("grpc-message: tenant quota exhausted"),
        "plugin body must reach grpc-message verbatim; got: {raw}"
    );
    assert!(
        !ctx.diagnostics
            .entries()
            .contains(&RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8)),
        "a valid UTF-8 body must not raise the not-utf8 marker"
    );
}

/// A non-UTF-8 body cannot be a `grpc-message` header value. It must fall
/// back to the status-mapped default rather than emitting raw bytes or a
/// U+FFFD-mangled decode, and the drop must be visible in the access log.
#[tokio::test]
async fn err_response_on_grpc_route_replaces_non_utf8_body_and_marks_it() {
    let (mut session, mut client) = plugin_test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrResponse {
        status: 403,
        body: Some(Bytes::from_static(&[0x89, 0x50, 0x4e, 0x47, 0xff, 0x00])),
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;
    let bytes = read_client_response(&mut client).await;
    let raw = String::from_utf8_lossy(&bytes).into_owned();

    let (expected_status, default_msg) = http_to_grpc_status(403);
    assert_eq!(ctx.request_info.grpc_status(), Some(expected_status));
    assert!(
        raw.contains(&format!("grpc-message: {default_msg}")),
        "non-utf8 body must fall back to the mapped default; got: {raw}"
    );
    // Neither the raw bytes nor a lossy decode may reach the wire.
    assert!(
        !bytes.windows(4).any(|w| w == [0x89, 0x50, 0x4e, 0x47]),
        "raw body bytes must not reach the trailer"
    );
    assert!(!raw.contains('\u{FFFD}'), "no lossy replacement characters on the wire");
    assert_eq!(
        ctx.diagnostics
            .entries()
            .iter()
            .filter(|e| **e == RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8))
            .count(),
        1,
        "the dropped body must be recorded exactly once"
    );
}

#[tokio::test]
async fn err_grpc_error_writer_failure_preserves_both_statuses() {
    let (mut session, client) = plugin_test_session().await;
    drop(client);
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(FixedResultFilter(PluginRunningResult::ErrGrpcError {
        grpc_status: 7,
        message: Some("denied".to_string()),
    })));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert_eq!(ctx.request_info.grpc_status(), Some(7));
    assert_eq!(client_closed_count(&ctx), 1);
}

#[tokio::test]
async fn self_written_writer_failure_records_client_closed() {
    let (mut session, client) = plugin_test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut runtime = PluginRuntime::new();
    runtime.add_request_filter(Box::new(SelfWritingFilter {
        client: Mutex::new(Some(client)),
    }));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert_eq!(ctx.final_downstream_status(), Some(401));
    assert_eq!(client_closed_count(&ctx), 1);
}

#[tokio::test]
async fn waf_request_runner_emits_only_the_dedicated_summary() {
    let (mut session, _client) = plugin_test_session().await;
    let mut runtime = PluginRuntime::new();
    runtime
        .add_waf(&WafConfig {
            mode: edgion_resources::resources::edgion_plugins::WafMode::On,
            priority: 12,
            ..crate::plugins::http::waf::fixture::config("SecAction \"id:1,phase:1,pass,log\"\n")
        })
        .unwrap();
    let candidate = runtime.waf_candidate().unwrap();
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_waf(Some(crate::ctx::WafSelection::new(
        candidate.owner(),
        crate::ctx::WafLogSource::Route,
        candidate.priority(),
        candidate.mode(),
    ))));

    runtime.run_request_plugins(&mut session, &mut ctx).await;

    assert!(ctx.stage_logs().is_empty());
    let waf = serde_json::to_value(ctx.waf_access_log().unwrap()).unwrap();
    assert_eq!(waf["source"], "route");
    assert_eq!(waf["priority"], 12);
    assert_eq!(waf["phases"]["request_headers"]["findings"], 1);
    assert_eq!(waf["findings"][0]["rule_id"], 1);
}

#[test]
fn invalid_update_policy_uses_plugin_variants_and_strictest_policy_wins() {
    let dsl = EdgionPlugin::Dsl(DslConfig::default());
    let key_auth = EdgionPlugin::KeyAuth(KeyAuthConfig::default());
    let mut invalid = InvalidPlugins::default();

    invalid.record(&dsl);
    invalid.record(&dsl);
    assert_eq!(invalid.update_policy(), Some(InvalidUpdatePolicy::RetainPrevious));

    invalid.record(&key_auth);
    invalid.record(&dsl);
    assert_eq!(invalid.update_policy(), Some(InvalidUpdatePolicy::ReplaceFailClosed));
}

fn json_schema_entry(
    body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
    enabled: bool,
) -> RequestFilterEntry {
    use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
    use serde_json::json;

    let mut entry = RequestFilterEntry::with_enable(
        EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: json!({"type": "object"}),
        }),
        enabled,
    );
    entry.body = body;
    entry
}

fn graphql_entry(
    body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
    enabled: bool,
) -> RequestFilterEntry {
    let config = serde_json::from_value(serde_json::json!({
        "schema":"type Query { name: String }",
        "redisRef":"unit/redis",
        "policies":[{"rate":100,"interval":"1s"}]
    }))
    .unwrap();
    let mut entry = RequestFilterEntry::with_enable(EdgionPlugin::GraphqlRateLimit(config), enabled);
    entry.body = body;
    entry
}

#[test]
fn graphql_body_validation_is_identical_on_controller_and_gateway() {
    use edgion_resources::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement, EdgionPlugins};
    for body in [
        BodyRequirement {
            max_body_size: Some("65k".into()),
            ..Default::default()
        },
        BodyRequirement {
            max_body_size: Some("0".into()),
            ..Default::default()
        },
        BodyRequirement {
            conditions: Some(Default::default()),
            ..Default::default()
        },
        BodyRequirement {
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        },
    ] {
        let entry = graphql_entry(Some(body), true);
        let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
            "metadata":{"name":"test-plugins", "namespace":"default"},
            "spec":{"requestPlugins":[entry.clone()]}
        }))
        .unwrap();
        let expected = resource.validate_plugins();
        let mut runtime = PluginRuntime::new();
        let actual = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
        assert!(!expected.is_empty());
        assert_eq!(actual, expected);
        assert_sealed_invalid(&mut runtime);
    }
    let disabled = graphql_entry(
        Some(BodyRequirement {
            max_body_size: Some("invalid".into()),
            ..Default::default()
        }),
        false,
    );
    let mut runtime = PluginRuntime::new();
    assert!(runtime
        .add_from_request_filters(&[disabled], "default", "test-plugins")
        .is_empty());
    assert_eq!(runtime.request_plugins_count(), 0);
}

fn form_json_entry(
    direction: edgion_resources::resources::edgion_plugins::FormJsonDirection,
    body: Option<edgion_resources::resources::edgion_plugins::BodyRequirement>,
) -> RequestFilterEntry {
    use edgion_resources::resources::edgion_plugins::FormJsonTransformConfig;

    let mut entry = RequestFilterEntry::new(EdgionPlugin::FormJsonTransform(FormJsonTransformConfig { direction }));
    entry.body = body;
    entry
}

fn assert_sealed_invalid(runtime: &mut PluginRuntime) {
    runtime.seal_fail_closed();
    assert_eq!(
        runtime.request_plugins_iter().next().map(|filter| filter.name()),
        Some("RejectInvalidConfig")
    );
}

#[test]
fn entry_scope_distinguishes_entries_and_resources() {
    // Two entries of one resource, and the same entry position in another
    // resource, must never share a scope.
    assert_eq!(
        crate::plugins::common::entry_scope("default/api-limiter", None, 0),
        "default/api-limiter/#0"
    );
    assert_ne!(
        crate::plugins::common::entry_scope("default/api-limiter", None, 0),
        crate::plugins::common::entry_scope("default/api-limiter", None, 1)
    );
    assert_ne!(
        crate::plugins::common::entry_scope("default/api-limiter", None, 0),
        crate::plugins::common::entry_scope("other/api-limiter", None, 0)
    );
    // An alias pins the scope across entry reordering, and cannot collide
    // with the index form of another entry.
    assert_eq!(
        crate::plugins::common::entry_scope("default/api-limiter", Some("tenant"), 3),
        "default/api-limiter/tenant"
    );
    assert_ne!(
        crate::plugins::common::entry_scope("default/api-limiter", Some("1"), 0),
        crate::plugins::common::entry_scope("default/api-limiter", None, 1)
    );
}

#[tokio::test]
async fn add_from_request_filters_defers_hmac_body_capture() {
    // HMAC owns its resolved body requirement instead of using the generic
    // eager gate. A malformed Authorization header must be rejected before
    // any request-body capture is attempted.
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::HmacAuthConfig;

    let cfg = HmacAuthConfig {
        match_config: edgion_resources::resources::HmacMatchConfig {
            validate_request_body: true,
            ..Default::default()
        },
        anonymous: Some("anon".to_string()),
        ..Default::default()
    };
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(
        &[RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg))],
        "default",
        "test-plugins",
    );
    assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    assert_eq!(filter.name(), "HmacAuth", "gate must be transparent to name()");

    let mut session = MockPluginSession::new();
    session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(&mut session, &[("authorization", "malformed")]);
    session.expect_ensure_request_body().times(0);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let mut log = PluginLog::new("HmacAuth");
    let result = filter.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrTerminateRequest,
        "HMAC must reject malformed authentication before capturing the body"
    );
}

#[tokio::test]
async fn json_schema_validation_gate_captures_without_materializing() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;
    use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
    use serde_json::json;

    let entry = RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
        schema: json!({"type": "object"}),
    }));
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    assert_eq!(filter.name(), "JsonSchemaValidation");

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_claim_json_schema_validation().return_const(true);
    session.expect_declares_body().return_const(Some(false));
    session
        .expect_ensure_request_body()
        .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
        .return_once(|_| Ok(0));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(true))
        .return_const(Ok(0));

    let mut log = PluginLog::new("JsonSchemaValidation");
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 400,
            body: Some(Bytes::from_static(b"Invalid request body")),
        }
    );
}

#[test]
fn form_json_runtime_builds_and_rejects_fail_open_without_controller() {
    use edgion_resources::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement, FormJsonDirection};

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(
        &[form_json_entry(FormJsonDirection::FormToJson, None)],
        "default",
        "test-plugins",
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        runtime.request_plugins_iter().next().map(|filter| filter.name()),
        Some("FormJsonTransform")
    );

    let body = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..BodyRequirement::default()
    };
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(
        &[form_json_entry(FormJsonDirection::JsonToForm, Some(body))],
        "default",
        "test-plugins",
    );
    assert_eq!(
        errors,
        vec!["requestPlugins[0] (FormJsonTransform): body.onReadFailure must be failClose"]
    );
    assert_eq!(runtime.request_plugins_count(), 0);
    assert_sealed_invalid(&mut runtime);
}

#[tokio::test]
async fn form_json_stock_gate_fails_before_header_or_body_mutation() {
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::EdgionHttpContext;
    use edgion_resources::resources::edgion_plugins::FormJsonDirection;

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(
        &[form_json_entry(FormJsonDirection::FormToJson, None)],
        "default",
        "test-plugins",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_ensure_request_body()
        .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
        .return_once(|_| Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .with(mockall::predicate::eq(
            edgion_resources::EdgionErrorCode::BodyReadUnimplemented,
        ))
        .return_const(());
    session.expect_request_header_values().times(0);
    session.expect_read_request_body().times(0);
    session.expect_overwrite_request_body().times(0);
    session.expect_set_request_header().times(0);
    let mut log = PluginLog::new(filter.name());

    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        }
    );
}

#[test]
fn json_schema_invalid_body_rules_seal_fail_closed() {
    use edgion_resources::resources::edgion_plugins::BodyRequirement;

    let cases = [
        (
            BodyRequirement {
                max_body_size: Some("invalid".to_string()),
                ..BodyRequirement::default()
            },
            "invalid body.maxBodySize",
        ),
        (
            BodyRequirement {
                max_body_size: Some("0".to_string()),
                ..BodyRequirement::default()
            },
            "must be greater than 0",
        ),
        (
            BodyRequirement {
                max_body_size: Some((1024 * 1024 + 1).to_string()),
                ..BodyRequirement::default()
            },
            "must not exceed 1MiB",
        ),
        (
            BodyRequirement {
                conditions: Some(Default::default()),
                ..BodyRequirement::default()
            },
            "conditions are not supported",
        ),
    ];

    for (body, expected_error) in cases {
        let expected_errors: Vec<String> =
            edgion_resources::resources::edgion_plugins::bounded_structured_body_validation_errors(&body)
                .into_iter()
                .map(|error| format!("requestPlugins[0] (JsonSchemaValidation): {error}"))
                .collect();
        let mut runtime = PluginRuntime::new();
        let errors =
            runtime.add_from_request_filters(&[json_schema_entry(Some(body), true)], "default", "test-plugins");
        assert_eq!(errors, expected_errors, "Gateway must mirror canonical body errors");
        assert!(
            errors.iter().any(|error| error.contains(expected_error)),
            "expected {expected_error:?}, got {errors:?}"
        );
        assert_eq!(runtime.request_plugins_count(), 0);
        assert_sealed_invalid(&mut runtime);
    }
}

#[test]
fn proxy_rewrite_bounded_body_validation_matches_consumption() {
    use edgion_resources::resources::edgion_plugins::{
        BodyRequirement, JsonBodyRewrite, JsonPatchOperation, ProxyRewriteConfig,
    };

    let invalid_body = BodyRequirement {
        conditions: Some(Default::default()),
        ..BodyRequirement::default()
    };
    let mut consuming = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![JsonPatchOperation::Remove {
                path: "/obsolete".to_string(),
            }],
        }),
        ..Default::default()
    }));
    consuming.body = Some(invalid_body.clone());

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[consuming], "default", "test-plugins");
    assert_eq!(
        errors,
        vec!["requestPlugins[0] (ProxyRewrite): body: conditions are not supported"]
    );
    assert_eq!(runtime.request_plugins_count(), 0);
    assert_sealed_invalid(&mut runtime);

    let mut uri_only = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
        uri: Some("/rewritten".to_string()),
        ..Default::default()
    }));
    uri_only.body = Some(invalid_body);
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[uri_only], "default", "test-plugins");
    assert_eq!(
        errors,
        vec!["requestPlugins[0] (ProxyRewrite): body: plugin does not consume a body"]
    );
    assert!(runtime.invalid_plugins.is_empty());
    assert_eq!(runtime.request_plugins_count(), 1);
}

#[test]
fn multiple_enabled_json_schema_validators_seal_fail_closed() {
    let entries = [json_schema_entry(None, true), json_schema_entry(None, true)];
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");
    assert_eq!(
        errors,
        vec!["requestPlugins: at most one JsonSchemaValidation plugin is allowed, found 2"]
    );
    assert_sealed_invalid(&mut runtime);
}

#[test]
fn disabled_json_schema_validator_is_inert_for_cardinality() {
    let entries = [json_schema_entry(None, true), json_schema_entry(None, false)];
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    runtime.seal_fail_closed();
    assert_eq!(runtime.request_plugins_count(), 1);
    assert_eq!(
        runtime.request_plugins_iter().next().map(|filter| filter.name()),
        Some("JsonSchemaValidation")
    );
}

#[test]
fn bypassed_json_schema_is_inert_for_body_validation_and_cardinality() {
    use edgion_resources::resources::edgion_plugins::BodyRequirement;
    let malformed = json_schema_entry(
        Some(BodyRequirement {
            max_body_size: Some("invalid".into()),
            ..Default::default()
        }),
        true,
    );
    let compiled = compile_scoped_request_entries(
        vec![malformed, json_schema_entry(None, true)],
        serde_json::json!([{"name":"http/JsonSchemaValidation","action":"bypass"}]),
    );
    assert!(compiled.preparse_errors.is_empty());
    assert!(compiled.is_activatable);
    assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
    assert_eq!(
        compiled.plugin_runtime.request_plugins_iter().next().unwrap().name(),
        "PluginPolicyBypass"
    );
}

#[test]
fn blocked_json_schema_is_inert_for_body_validation_and_cardinality() {
    use edgion_resources::resources::edgion_plugins::BodyRequirement;
    let malformed = json_schema_entry(
        Some(BodyRequirement {
            conditions: Some(Default::default()),
            ..Default::default()
        }),
        true,
    );
    let compiled = compile_scoped_request_entries(
        vec![malformed, json_schema_entry(None, true)],
        serde_json::json!([{"name":"http/JsonSchemaValidation","blockStatus":451}]),
    );
    assert!(compiled.preparse_errors.is_empty());
    assert!(compiled.is_activatable);
    assert_eq!(
        compiled.plugin_runtime.request_plugins_iter().next().unwrap().name(),
        "PluginPolicyBlock"
    );
}

#[tokio::test]
async fn json_schema_default_limit_accepts_exactly_64_kib() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;
    use bytes::Bytes;

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[json_schema_entry(None, true)], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

    let mut body = vec![b' '; 64 * 1024];
    body[0] = b'{';
    body[1] = b'}';
    let body = Bytes::from(body);
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_claim_json_schema_validation().return_const(true);
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_ensure_request_body()
        .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
        .return_const(Ok(64 * 1024));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(false))
        .return_const(Ok(64 * 1024));
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    session.expect_request_header_map().times(1).return_const(headers);
    session
        .expect_read_request_body()
        .with(mockall::predicate::eq(None))
        .return_once(move |_| Ok(body));

    let mut log = PluginLog::new("JsonSchemaValidation");
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::Nothing
    );
}

#[tokio::test]
async fn json_schema_default_limit_rejects_64_kib_plus_one_before_materialization() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[json_schema_entry(None, true)], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_ensure_request_body()
        .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
        .return_const(Ok(64 * 1024 + 1));
    session
        .expect_record_error()
        .with(mockall::predicate::eq(
            edgion_resources::EdgionErrorCode::BodyReadTooLarge,
        ))
        .return_once(|_| ());

    let mut log = PluginLog::new("JsonSchemaValidation");
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None,
        }
    );
}

#[tokio::test]
async fn transcoder_body_admission_preserves_json_errors_and_operator_limit() {
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::EdgionHttpContext;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use edgion_resources::grpc_transcoding::GrpcJsonTranscoderConfig;
    use edgion_resources::resources::edgion_plugins::BodyRequirement;

    let config: GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({
        "descriptor": {
            "type": "inline",
            "base64": STANDARD.encode(include_bytes!("../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb"))
        },
        "services": ["test.TestService"]
    }))
    .unwrap();
    for (capture, status, diagnostic) in [
        (Err(BodyReadError::NotBuffered), 500, None),
        (
            Err(BodyReadError::StorageFailure),
            500,
            Some(edgion_resources::EdgionErrorCode::BodyCaptureStorageFailed),
        ),
        (
            Err(BodyReadError::BufferLimitExceeded {
                limit: 32 * 1024 * 1024,
            }),
            413,
            Some(edgion_resources::EdgionErrorCode::BodyReadTooLarge),
        ),
        (Ok(33), 413, Some(edgion_resources::EdgionErrorCode::BodyReadTooLarge)),
    ] {
        let mut entry = RequestFilterEntry::new(EdgionPlugin::GrpcJsonTranscoder(config.clone()));
        entry.body = Some(BodyRequirement {
            max_body_size: Some("32".into()),
            ..BodyRequirement::fail_close()
        });
        let mut runtime = PluginRuntime::new();
        assert!(runtime
            .add_from_request_filters(&[entry], "default", "test-transcoder")
            .is_empty());
        let filter = runtime.request_plugins_iter().next().unwrap();
        let mut session = MockPluginSession::new();
        session.expect_grpc_transcoding_allowed().times(1).return_const(true);
        session.expect_get_method().return_const("POST".to_string());
        session.expect_get_path().return_const("/v1/hello".to_string());
        session.expect_get_query().return_const(None);
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(32 * 1024 * 1024)))
            .times(1)
            .return_once(move |_| capture);
        session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("content-type"),
                mockall::predicate::eq("application/json"),
            )
            .times(1)
            .return_once(|_, _| Ok(()));
        if let Some(diagnostic) = diagnostic {
            session
                .expect_record_error()
                .with(mockall::predicate::eq(diagnostic))
                .times(1)
                .return_const(());
        }
        let mut log = PluginLog::new("GrpcJsonTranscoder");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status,
                body: Some(bytes::Bytes::from_static(
                    b"{\"error\":\"gRPC transcoding request failed\"}"
                )),
            }
        );
    }
}

#[tokio::test]
async fn request_body_buffer_fails_closed_when_capture_is_unimplemented() {
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::EdgionHttpContext;

    let entry = RequestFilterEntry::new(EdgionPlugin::RequestBodyBuffer(Default::default()));
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    assert_eq!(filter.name(), "RequestBodyBuffer");

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_ensure_request_body()
        .withf(|max_bytes| *max_bytes == Some(32 * 1024 * 1024))
        .returning(|_| Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyReadUnimplemented)
        .times(1)
        .returning(|_| ());

    let mut log = PluginLog::new("RequestBodyBuffer");
    let result = filter.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn add_from_request_filters_wraps_wasm_with_operator_body() {
    // Wasm has no INTRINSIC body_requirement() (the guest's hooks are only
    // known after module load), so it opts into request buffer mode purely
    // through an operator `body:` block on the entry. Proof, mirroring the
    // HMAC case above: with `body:` set, the built filter must force-read
    // the body (the mock returns Unimplemented) and fail-close 500 BEFORE ever
    // reaching WasmPlugin's own logic (which would otherwise try to load
    // the module and fail differently). If the gate were not wired for
    // Wasm, `read_request_body` would never be called by the gate at all
    // and this assertion would not observe a 500 from the gate.
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::EdgionHttpContext;
    use edgion_resources::resources::edgion_plugins::plugin_configs::WasmConfig;
    use edgion_resources::resources::edgion_plugins::BodyRequirement;

    let mut cfg = WasmConfig::default();
    cfg.source.file = Some("plugin.wasm".to_string());
    let mut entry = RequestFilterEntry::new(EdgionPlugin::Wasm(cfg));
    entry.body = Some(BodyRequirement::fail_close());

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
    assert!(
        errors.is_empty(),
        "operator body: on a Wasm entry must be accepted at admission, got: {errors:?}"
    );

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    assert_eq!(filter.name(), "Wasm", "gate must be transparent to name()");

    let mut session = MockPluginSession::new();
    session.expect_get_method().return_const("POST".to_string());
    session.expect_header_value().returning(|_| None);
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_record_error().returning(|_| ());
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("Wasm");
    let result = filter.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        },
        "operator body: on Wasm must actually wrap the plugin in the body gate"
    );
}

#[test]
fn add_from_request_filters_drops_a_duplicate_alias() {
    let mut runtime = PluginRuntime::new();

    let mut first = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
    first.alias = Some("dup".to_string());
    let mut second = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
    second.alias = Some("dup".to_string());

    let entries = vec![first, second];
    let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

    assert!(
        errors
            .iter()
            .any(|e| e.contains("requestPlugins[1]") && e.contains("duplicate alias 'dup'")),
        "expected a duplicate-alias error for index 1, got: {:?}",
        errors
    );

    // Reporting alone is not enough: lenient admission keeps an
    // `Accepted=False` resource serving, so the duplicate must also be
    // dropped. The second entry then falls back to its `#index` scope
    // instead of sharing the first entry's bucket namespace.
    let mut iter = runtime.request_plugins_iter();
    assert_eq!(iter.next().expect("first wrapper").alias(), Some("dup"));
    assert_eq!(
        iter.next().expect("second wrapper").alias(),
        None,
        "duplicate alias must be dropped, like an invalid one"
    );
}

#[test]
fn add_from_request_filters_reports_alias_validation_error_and_threads_alias() {
    let mut runtime = PluginRuntime::new();

    let mut bad_entry = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
    bad_entry.alias = Some("with space".to_string());

    let mut good_entry = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));
    good_entry.alias = Some("auth-internal".to_string());

    let entries = vec![bad_entry, good_entry];
    let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

    // The bad alias surfaces as a validation error referencing the index and the alias rule.
    assert!(
        errors
            .iter()
            .any(|e| e.contains("requestPlugins[0]") && e.contains("alias")),
        "expected alias error for index 0, got: {:?}",
        errors
    );

    // Both entries are still wrapped (we surface errors but don't drop the entry).
    let mut iter = runtime.request_plugins_iter();
    let first = iter.next().expect("first wrapper");
    // Bad alias is dropped on validation failure so an unvalidated value never reaches the access log.
    assert_eq!(first.alias(), None, "bad alias must be dropped (validation refused)");
    let second = iter.next().expect("second wrapper");
    assert_eq!(second.alias(), Some("auth-internal"));
}

/// Minimal valid `CanaryConfig` (all fields operator-required, no `Default`).
/// Canary is a request-stage plugin whose `dye_capabilities()` include
/// `Success`, so it can carry a request-direction dye rule.
fn canary_config_for_dye_test() -> edgion_resources::resources::edgion_plugins::plugin_configs::CanaryConfig {
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        CanaryConfig, DynUpstreamTarget, UpstreamFallback,
    };
    use std::collections::HashMap;
    CanaryConfig {
        enable: true,
        active_profile: "stable".to_string(),
        profiles: HashMap::from([(
            "stable".to_string(),
            DynUpstreamTarget {
                name: "app-v1".to_string(),
                namespace: None,
            },
        )]),
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    }
}

#[test]
fn add_from_request_filters_compiles_and_threads_dye_plan() {
    use edgion_resources::resources::edgion_plugins::{DyeRule, DyeSuite};

    let mut entry = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config_for_dye_test()));
    entry.dye = Some(DyeSuite {
        request: vec![DyeRule {
            name: "X-Canary-Profile".to_string(),
            on: vec![edgion_resources::resources::edgion_plugins::DyeOutcome::Success],
            value: None,
            conditions: None,
        }],
        response: vec![],
    });

    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    let plan = filter.dye_plan().expect("dye plan threaded through the wrapper");
    assert_eq!(plan.type_prefix, "canary");
    assert_eq!(plan.request.len(), 1);
    assert_eq!(plan.request[0].name, "X-Canary-Profile");
    assert!(plan.response.is_empty());
}

#[test]
fn add_from_request_filters_has_no_dye_plan_when_entry_has_none() {
    let entry = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config_for_dye_test()));
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "test-plugins");
    assert!(errors.is_empty(), "unexpected build errors: {errors:?}");

    let filter = runtime.request_plugins_iter().next().expect("one wrapped filter");
    assert!(filter.dye_plan().is_none());
}

#[test]
fn csrf_without_resolved_key_fails_validation() {
    use edgion_resources::resources::edgion_plugins::{CsrfConfig, EdgionPlugin};
    // No secret_ref + no resolved_key → rejected.
    let plugin = EdgionPlugin::Csrf(CsrfConfig::default());
    assert!(PluginRuntime::plugin_validation_error(&plugin).is_some());
    // Empty resolved_key → still rejected (the gate is stronger than the shared check).
    let empty_key = EdgionPlugin::Csrf(CsrfConfig {
        resolved_key: Some(String::new()),
        ..Default::default()
    });
    assert!(PluginRuntime::plugin_validation_error(&empty_key).is_some());
    // Non-empty resolved_key → accepted.
    let resolved = EdgionPlugin::Csrf(CsrfConfig {
        resolved_key: Some("k".to_string()),
        ..Default::default()
    });
    assert!(PluginRuntime::plugin_validation_error(&resolved).is_none());
}

#[test]
fn invalid_plugin_config_is_skipped_and_sealed_fail_closed() {
    use edgion_resources::resources::edgion_plugins::ForwardAuthConfig;

    let mut runtime = PluginRuntime::new();

    // Entry 0: invalid config (missing port fails ForwardAuthConfig validation).
    let invalid = RequestFilterEntry::new(EdgionPlugin::ForwardAuth(ForwardAuthConfig::default()));
    // Entry 1: valid config.
    let valid = RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()));

    let entries = vec![invalid, valid];
    let errors = runtime.add_from_request_filters(&entries, "default", "test-plugins");

    // The invalid plugin is reported against its index.
    assert!(
        errors
            .iter()
            .any(|e| e.contains("requestPlugins[0]") && e.contains("ForwardAuth")),
        "expected ForwardAuth validation error for index 0, got: {:?}",
        errors
    );

    // The invalid plugin is NOT constructed: only the valid entry is in the chain.
    assert_eq!(
        runtime.request_plugins_count(),
        1,
        "invalid plugin must be skipped, not constructed"
    );

    // Sealing prepends a fail-closed RejectFilter so the route returns 500.
    runtime.seal_fail_closed();
    assert_eq!(runtime.request_plugins_count(), 2, "RejectFilter must be prepended");
    let first = runtime.request_plugins_iter().next().expect("reject filter");
    assert_eq!(first.name(), "RejectInvalidConfig");
}

#[test]
fn invalid_edgion_request_transform_is_skipped_and_sealed_fail_closed() {
    let mut runtime = PluginRuntime::new();
    let redirect = serde_json::from_value(serde_json::json!({"scheme": "ftp"})).unwrap();
    let entries = [RequestFilterEntry::new(EdgionPlugin::RequestRedirect(redirect))];

    let errors = runtime.add_from_request_filters(&entries, "default", "transforms");
    assert!(errors
        .iter()
        .any(|error| error.contains("requestPlugins[0]") && error.contains("RequestRedirect")));
    assert_eq!(runtime.request_plugins_count(), 0);

    runtime.seal_fail_closed();
    assert_eq!(runtime.request_plugins_count(), 1);
    assert_eq!(
        runtime.request_plugins_iter().next().map(|filter| filter.name()),
        Some("RejectInvalidConfig")
    );
}

#[test]
fn cors_token_validation_matches_resource_ingest_and_fail_closed_gate() {
    use edgion_resources::resources::EdgionPlugins;

    for field in ["allowMethods", "allowHeaders", "exposeHeaders"] {
        for (value, enabled, invalid) in [
            ("PROPFIND, custom-Method,", true, false),
            ("X-Name: value", true, true),
            ("X-Name: value", false, false),
        ] {
            let mut config = serde_json::json!({"allowOrigins": "https://example.com"});
            config[field] = serde_json::json!(value);
            let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionPlugins",
                "metadata": {"name": "cors-validation", "namespace": "default"},
                "spec": {"requestPlugins": [{"type": "Cors", "enable": enabled, "config": config}]}
            }))
            .unwrap();
            let ingest_errors = resource.validate_plugins();
            let mut runtime = PluginRuntime::new();
            let build_errors = runtime.add_from_request_filters(
                resource.spec.request_plugins.as_ref().unwrap(),
                "default",
                "cors-validation",
            );
            assert_eq!(ingest_errors, build_errors);
            assert_eq!(!build_errors.is_empty(), invalid);
            assert_eq!(runtime.cors_config_arc().is_some(), enabled && !invalid);
            runtime.seal_fail_closed();
            let expected = if invalid {
                Some("RejectInvalidConfig")
            } else if enabled {
                Some("Cors")
            } else {
                None
            };
            assert_eq!(
                runtime.request_plugins_iter().next().map(|filter| filter.name()),
                expected
            );
        }
    }
}

#[test]
fn seal_fail_closed_is_noop_when_all_plugins_valid() {
    let mut runtime = PluginRuntime::new();
    let entries = vec![RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default()))];
    runtime.add_from_request_filters(&entries, "default", "test-plugins");

    runtime.seal_fail_closed();
    assert_eq!(
        runtime.request_plugins_count(),
        1,
        "no RejectFilter when every plugin config is valid"
    );
}

#[test]
fn scoped_restrictions_do_not_instantiate_or_invalidate_entries() {
    use edgion_resources::resources::edgion_plugins::{CorsConfig, MockConfig};
    let compiled = compile_scoped_request_entries(
        vec![
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
            RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default())),
            RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default())),
        ],
        serde_json::json!([
            {"name":"http/KeyAuth","blockStatus":451},
            {"name":"http/Cors","action":"bypass"}
        ]),
    );
    assert!(compiled.preparse_errors.is_empty());
    assert!(compiled.is_activatable);
    let names: Vec<_> = compiled
        .plugin_runtime
        .request_plugins_iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(names, vec!["PluginPolicyBlock", "PluginPolicyBypass", "Mock"]);
}

#[test]
fn policy_block_gate_precedes_fail_closed_reject() {
    let compiled = compile_scoped_request_entries(
        vec![
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
            RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                source: Some("invalid @@@".into()),
                ..Default::default()
            })),
        ],
        serde_json::json!([{"name":"http/KeyAuth","blockStatus":451}]),
    );
    assert!(!compiled.is_activatable);
    let mut filters = compiled.plugin_runtime.request_plugins_iter();
    assert_eq!(filters.next().unwrap().name(), "PluginPolicyBlock");
    assert_eq!(filters.next().unwrap().name(), "RejectInvalidConfig");
}

#[tokio::test]
async fn policy_bypass_note_cannot_hide_fail_closed_preflight_gate() {
    let compiled = compile_scoped_request_entries(
        vec![
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig::default())),
            RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                source: Some("invalid @@@".into()),
                ..Default::default()
            })),
        ],
        serde_json::json!([{"name":"http/KeyAuth","action":"bypass"}]),
    );
    assert!(!compiled.is_activatable);
    let names: Vec<_> = compiled
        .plugin_runtime
        .request_plugins_iter()
        .map(|filter| filter.name())
        .collect();
    assert_eq!(names[0], "RejectInvalidConfig");
    assert_eq!(names[1], "PluginPolicyBypass");

    let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
    let mut ctx = EdgionHttpContext::default();
    compiled.plugin_runtime.run_preflight_gate(&mut session, &mut ctx).await;

    assert!(ctx.plugin_terminated());
}

#[test]
fn unrestricted_entries_have_no_policy_filters() {
    use edgion_resources::resources::edgion_plugins::MockConfig;

    let normal = RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default()));
    let mut runtime = PluginRuntime::new();
    let _ = runtime.add_from_request_filters(&[normal], "default", "test-plugins");
    runtime.seal_fail_closed();
    runtime.seal_plugin_policy();
    assert_eq!(runtime.request_plugins.len(), 1); // just Mock, zero overhead
}

#[test]
fn invalid_response_stage_plugin_seals_request_stage_fail_closed() {
    use edgion_resources::resources::edgion_plugins::{BandwidthLimitConfig, UpstreamResponseBodyFilterEntry};

    let mut runtime = PluginRuntime::new();

    // BandwidthLimit is a body-stage plugin; an empty rate fails its validation.
    let invalid = UpstreamResponseBodyFilterEntry::new(EdgionPlugin::BandwidthLimit(BandwidthLimitConfig {
        rate: String::new(),
        rate_bytes_per_second: None,
    }));
    let errors = runtime.add_from_upstream_response_body_filters(&[invalid], "default");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("upstreamResponseBodyFilterPlugins[0]") && e.contains("BandwidthLimit")),
        "expected BandwidthLimit validation error, got: {:?}",
        errors
    );

    // The invalid body-stage plugin is skipped, and nothing is in the request stage yet.
    assert_eq!(runtime.upstream_response_body_plugins_count(), 0);
    assert_eq!(runtime.request_plugins_count(), 0);

    // Sealing puts the RejectFilter on the REQUEST stage even though the
    // invalid plugin was a body-stage plugin: the request stage runs before
    // upstream is contacted, so this is the only gate that guarantees a 500.
    runtime.seal_fail_closed();
    assert_eq!(
        runtime.request_plugins_count(),
        1,
        "request-stage RejectFilter must cover body-stage invalid configs"
    );
    let first = runtime.request_plugins_iter().next().expect("reject filter");
    assert_eq!(first.name(), "RejectInvalidConfig");
}

#[test]
fn dsl_construction_failure_seals_fail_closed() {
    use edgion_resources::resources::edgion_plugins::DslConfig;

    let mut runtime = PluginRuntime::new();

    // A DSL config whose source fails to build.
    let bad_dsl = RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
        name: "bad".to_string(),
        source: Some("this is not valid dsl @@@".to_string()),
        bytecode: None,
        ..Default::default()
    }));

    runtime.add_from_request_filters(&[bad_dsl], "default", "test-plugins");

    // A DSL config that fails to build (whether caught by `validation_error()`
    // or by the `Ok(None)` constructor-declined arm) must leave the route
    // sealed fail-closed — assert the end state: a RejectFilter is prepended.
    runtime.seal_fail_closed();
    assert_eq!(
        runtime.request_plugins_count(),
        1,
        "DSL construction failure must seal fail-closed: a RejectFilter must be prepended"
    );
    let first = runtime.request_plugins_iter().next().expect("reject filter");
    assert_eq!(first.name(), "RejectInvalidConfig");
}

#[test]
fn canary_plugin_construction() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        CanaryConfig, DynUpstreamTarget, UpstreamFallback,
    };
    use std::collections::HashMap;

    // Create a minimal valid CanaryConfig (must have at least one profile).
    let mut profiles = HashMap::new();
    profiles.insert(
        "base".to_string(),
        DynUpstreamTarget {
            name: "backend1".to_string(),
            namespace: None,
        },
    );

    let config = CanaryConfig {
        enable: true,
        active_profile: "base".to_string(),
        profiles,
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    };

    let plugin = EdgionPlugin::Canary(config);

    // Verify request stage returns Ok(Some(...)).
    let result = PluginRuntime::create_request_filter_from_edgion(&plugin, "default", None, "default/test-plugins/#0");
    assert!(
        matches!(result, CreateResult::Ok(Some(_))),
        "Canary should construct in request stage"
    );

    // Verify upstream response stage returns WrongStage.
    let result_response = PluginRuntime::create_upstream_response_filter_from_edgion(&plugin, "default");
    assert!(
        matches!(result_response, CreateResult::WrongStage),
        "Canary should not apply to upstream response stage"
    );

    // Verify get_plugin_name works.
    assert_eq!(PluginRuntime::get_plugin_name(&plugin), "Canary");
}

#[test]
fn ai_proxy_is_constructed_only_for_request_stage() {
    use edgion_resources::resources::edgion_plugins::{
        AiBackendSelection, AiBackendSelectionConfig, AiProxyConfig, OnUnknownModel,
    };

    let plugin = EdgionPlugin::AiProxy(AiProxyConfig {
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        backend_selection: AiBackendSelectionConfig {
            selection_type: AiBackendSelection::Weighted,
        },
        inject_usage_tracking: true,
        on_unknown_model: OnUnknownModel::Reject,
        model_routes: Vec::new(),
        token_quota: None,
    });

    let request = PluginRuntime::create_request_filter_from_edgion(
        &plugin,
        "tenant-a",
        plugin.body_requirement(edgion_resources::resources::edgion_plugins::BodyStage::Request),
        "tenant-a/plugins/#0",
    );
    let CreateResult::Ok(Some(filter)) = request else {
        panic!("AiProxy must construct in request stage");
    };
    assert_eq!(filter.name(), "AiProxy");

    assert!(matches!(
        PluginRuntime::create_upstream_response_filter_from_edgion(&plugin, "tenant-a"),
        CreateResult::WrongStage
    ));
    assert!(matches!(
        PluginRuntime::create_upstream_response_body_filter_from_edgion(&plugin, "tenant-a"),
        CreateResult::WrongStage
    ));
    assert!(matches!(
        PluginRuntime::create_upstream_response_from_edgion(&plugin, "tenant-a"),
        CreateResult::WrongStage
    ));
    assert_eq!(PluginRuntime::get_plugin_name(&plugin), "AiProxy");
}

#[tokio::test]
async fn native_guard_invocation_marker_respects_disable_condition_and_policy_bypass() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    use std::sync::atomic::Ordering;
    for variant in ["invoked", "disabled", "condition", "bypass"] {
        let mut entry:RequestFilterEntry=serde_json::from_value(serde_json::json!({"type":"AiGuard","config":{"rules":[{"name":"tripwire","match":{"type":"regex","pattern":"never-present"},"action":"reject"}]}})).unwrap();
        if variant == "disabled" {
            entry.enable = false;
        }
        if variant == "condition" {
            entry.conditions = Some(
                serde_json::from_value(
                    serde_json::json!({"skip":{"allOf":[{"type":"keyExist","key":{"type":"header","name":"host"}}]}}),
                )
                .unwrap(),
            );
        }
        let deny = if variant == "bypass" {
            serde_json::json!([{"name":"http/AiGuard","action":"bypass"}])
        } else {
            serde_json::json!([])
        };
        let compiled = compile_scoped_request_entries(vec![entry], deny);
        assert!(compiled.preparse_errors.is_empty());
        let mut session = session_from_request(b"GET /custom/responses HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.in_request_body_stage = true;
        let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
        storage.finish().await.unwrap();
        ctx.request_body.install(view);
        compiled
            .plugin_runtime
            .run_request_plugins(&mut session, &mut ctx)
            .await;
        assert_eq!(
            ctx.ai_request_guard_invoked.load(Ordering::Relaxed),
            variant == "invoked"
        );
        assert!(ctx.ai_state.is_none(), "ordinary traffic must not allocate an AI box");
    }
}

#[test]
fn graphql_strict_backend_admission_checks_selected_direct_and_pinned_effective_owners() {
    let entry:RequestFilterEntry=serde_json::from_value(serde_json::json!({"type":"AiGuard","config":{"rules":[{"name":"tripwire","match":{"type":"regex","pattern":"never-present"},"action":"reject"}]}})).unwrap();
    let active = compile_scoped_request_entries(vec![entry.clone()], serde_json::json!([]));
    let bypassed = compile_scoped_request_entries(
        vec![entry],
        serde_json::json!([{"name":"http/AiGuard","action":"bypass"}]),
    );
    let ctx = || {
        let mut ctx = EdgionHttpContext::default();
        ctx.try_set_strict_jump(crate::UpstreamJumpTarget::Internal(crate::InternalJumpPreset {
            backend_ref_name: "users".into(),
            backend_ref_namespace: Some("apps".into()),
        }));
        ctx
    };
    assert!(!active.plugin_runtime.backend_compatible(&mut ctx()));
    assert!(bypassed.plugin_runtime.backend_compatible(&mut ctx()));
    assert!(active
        .plugin_runtime
        .backend_compatible(&mut EdgionHttpContext::default()));
    let filter=serde_json::from_value(serde_json::json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"backend"}})).unwrap();
    let backend = PluginRuntime::from_httproute_filters_for_backend(
        &[filter],
        "apps",
        &edgion_resources::MirrorTuning::default(),
    );
    let mut selected = ctx();
    selected.pinned_backend_ext_ref("apps/backend", || Some(Arc::clone(&active.plugin_runtime)));
    assert!(!backend.backend_compatible(&mut selected));
    // An unrelated unselected backend owner never participates in this admission.
    let mut selected = ctx();
    selected.pinned_backend_ext_ref("apps/backend", || Some(Arc::clone(&bypassed.plugin_runtime)));
    selected.pinned_backend_ext_ref("apps/unselected", || Some(Arc::clone(&active.plugin_runtime)));
    assert!(backend.backend_compatible(&mut selected));
}
