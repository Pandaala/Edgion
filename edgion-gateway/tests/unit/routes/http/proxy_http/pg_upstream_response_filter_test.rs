use super::{normalize_final_response_headers, release_late_request_body_view};
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::request_body::BodyStorage;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::EdgionHttpContext;
use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;

struct ProviderHeaderReset;

#[async_trait]
impl StreamingResponseProcessor for ProviderHeaderReset {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        response.remove_header("server");
        response.remove_header("strict-transport-security");
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
    ) {
    }
}

#[test]
fn gateway_server_headers_are_applied_after_provider_header_reset() {
    let mut tls_response = ResponseHeader::build(200, None).unwrap();
    tls_response.insert_header("server", "provider").unwrap();
    tls_response
        .insert_header("strict-transport-security", "max-age=1")
        .unwrap();
    let mut tls_ctx = EdgionHttpContext::default();
    tls_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));

    normalize_final_response_headers(&mut tls_response, &mut tls_ctx, &ServerHeaderOpts::default());

    assert_eq!(tls_response.headers["server"], "Edgion");
    assert_eq!(
        tls_response.headers["strict-transport-security"],
        "max-age=63072000; includeSubDomains; preload"
    );

    let mut plain_response = ResponseHeader::build(200, None).unwrap();
    plain_response.insert_header("server", "provider").unwrap();
    plain_response
        .insert_header("strict-transport-security", "max-age=1")
        .unwrap();
    let mut plain_ctx = EdgionHttpContext::default();
    plain_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));
    let mut plain_headers = ServerHeaderOpts::default();
    plain_headers.remove_hsts();

    normalize_final_response_headers(&mut plain_response, &mut plain_ctx, &plain_headers);

    assert_eq!(plain_response.headers["server"], "Edgion");
    assert!(plain_response.headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn final_response_header_stage_releases_request_body_view() {
    let (mut storage, view) = BodyStorage::new(64, 64);
    storage.write(&Bytes::from_static(b"cached")).await.unwrap();
    storage.finish().await.unwrap();
    let mut request_body = crate::request_body::RequestBodyState::default();
    request_body.install(view);
    request_body.overwrite(Bytes::from_static(b"edited")).unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_body = request_body;
    assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
    release_late_request_body_view(&mut ctx);

    assert!(!ctx.request_body.has_cached_body());
    assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
}

fn session_lifetime_context(grpc: bool, policy: serde_json::Value) -> EdgionHttpContext {
    use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
    use std::sync::Arc;
    let mut ctx = EdgionHttpContext::default();
    let envelope = serde_json::json!({"metadata":{"name":"session-wire", "namespace":"default"},
        "spec":{"rules":[{"sessionPersistence":policy}]}});
    if grpc {
        let mut envelope = envelope;
        envelope["spec"]["parentRefs"] = serde_json::json!([{"name":"gw"}]);
        envelope["spec"]["resolvedStatusController"] = serde_json::json!("edgion.io/gateway-controller");
        envelope["spec"]["rules"][0]["matches"] =
            serde_json::json!([{"method":{"service":"svc.User", "method":"Get"}}]);
        let route: edgion_resources::GRPCRoute = crate::test_current_status::from_value(envelope).unwrap();
        ctx.grpc_route_unit = Some(
            crate::routes::grpc::parse_route_to_units_for_test("default/session-wire", &Arc::new(route)).remove(0),
        );
    } else {
        let route: edgion_resources::HTTPRoute = serde_json::from_value(envelope).unwrap();
        let runtime = HttpRouteRuleUnit::compile_http_rule(&route, 0).unwrap();
        let matched = serde_json::from_value(serde_json::json!({"path":{"type":"PathPrefix", "value":"/"}})).unwrap();
        ctx.route_unit = Some(Arc::new(HttpRouteRuleUnit {
            delegate: None,
            filter_mask: 0,
            session_persistence_ttl_secs: runtime.session_persistence_ttl_secs,
            resource_key: "default/session-wire".into(),
            matched_info: crate::MatchInfo::new("default".into(), "session-wire".into(), 0, 0, matched, 0),
            rule: Arc::new(route.spec.rules.as_ref().unwrap()[0].clone()),
            route: Some(Arc::new(route)),
            path_regex: None,
            parent_refs: None,
            compiled_header_regexes: vec![],
            compiled_query_regexes: vec![],
            has_query_matchers: false,
            specificity: MatchSpecificity {
                has_method: false,
                header_count: 0,
                query_count: 0,
                creation_ts_ms: 0,
                route_key: "default/session-wire".into(),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            backend_finder: HttpRouteRuleUnit::build_backend_selector(&runtime.backends),
            plugin_runtime: runtime.plugin_runtime,
            backends: runtime.backends,
        }));
    }
    ctx
}

#[tokio::test]
async fn session_lifetime_actual_response_issuance_uses_captured_policy() {
    use super::{upstream_response_filter, EdgionHttpProxy};
    use crate::routes::http::session_persistence::now_unix_secs;
    use crate::SessionPersistenceState;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let proxy = EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("127.0.0.1:80/HTTP".into()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: crate::routes::http::proxy_http::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    };
    for grpc in [false, true] {
        for mode in ["Permanent", "Session", "Header"] {
            for timeout in [Some("1500ms"), Some("5s"), None] {
                if mode == "Permanent" && timeout.is_none() {
                    continue;
                }
                let policy = serde_json::json!({"sessionName":"sticky", "absoluteTimeout": timeout,
                    "type": if mode == "Header" {"Header"} else {"Cookie"},
                    "cookieConfig":{"lifetimeType":if mode == "Permanent" {"Permanent"} else {"Session"}}});
                let mut ctx = session_lifetime_context(grpc, policy);
                // Capturing another generation does not rewrite this request's selected unit.
                let newer = session_lifetime_context(grpc, serde_json::json!({"absoluteTimeout":"99s"}));
                assert_eq!(newer.effective_session_persistence_with_lifetime().unwrap().1, Some(99));
                let lifetime = ctx.effective_session_persistence_with_lifetime().unwrap().1;
                ctx.ensure_session_persistence().state = if timeout == Some("5s") {
                    SessionPersistenceState::InjectUpdate
                } else {
                    SessionPersistenceState::InjectNew
                };
                let mut upstream = crate::UpstreamInfo {
                    ip: None,
                    port: None,
                    status: None,
                    ct: None,
                    ht: None,
                    bt: None,
                    et: None,
                    wpt: None,
                    response_body_size: None,
                    start_time: std::time::Instant::now(),
                    err: vec![],
                    lb_backend_addr: None,
                    lb_backend: None,
                    lb_outlier_completion: None,
                    service_key: None,
                    lb_policy: None,
                    tls: None,
                    lb_committed: None,
                };
                upstream.lb_backend_addr = Some(pingora_core::protocols::l4::socket::SocketAddr::Inet(
                    "127.0.0.1:8080".parse().unwrap(),
                ));
                ctx.backend_context = Some(crate::BackendContext {
                    effective_policy_captured: false,
                    effective_policy: None,
                    source: None,
                    rsc: None,
                    service_key: None,
                    service_kind: None,
                    upstreams: vec![upstream],
                    current_upstream_id: Some(0),
                    backend_ref_idx: None,
                });
                let (mut client, server) = tokio::io::duplex(1024);
                client
                    .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
                    .await
                    .unwrap();
                let mut session = pingora_proxy::Session::new_h1(Box::new(server));
                assert!(session.read_request().await.unwrap());
                let mut response = ResponseHeader::build(200, None).unwrap();
                let before = now_unix_secs();
                upstream_response_filter(&proxy, &mut session, &mut response, &mut ctx).unwrap();
                let after = now_unix_secs();
                let value = response.headers[if mode == "Header" { "sticky" } else { "set-cookie" }]
                    .to_str()
                    .unwrap();
                let token = if mode == "Header" {
                    value
                } else {
                    value.split(';').next().unwrap().strip_prefix("sticky=").unwrap()
                };
                if let Some(ttl) = lifetime {
                    let expiry: u64 = token.rsplit_once('.').unwrap().1.parse().unwrap();
                    assert!((before + ttl..=after + ttl).contains(&expiry));
                    if mode == "Permanent" {
                        assert!(value.contains(&format!("Max-Age={ttl}")));
                    } else {
                        assert!(!value.contains("Max-Age="));
                    }
                } else {
                    assert!(!token.contains('.'));
                    assert!(!value.contains("Max-Age="));
                }
                assert_eq!(
                    ctx.session_persistence().unwrap().state,
                    SessionPersistenceState::Inactive
                );
            }
        }
    }
}

#[test]
fn session_lifetime_http_block_without_timeout_never_borrows_grpc_timeout() {
    let mut ctx = session_lifetime_context(false, serde_json::json!({"sessionName":"http"}));
    let grpc = session_lifetime_context(true, serde_json::json!({"sessionName":"grpc", "absoluteTimeout":"5s"}));
    ctx.grpc_route_unit = grpc.grpc_route_unit;
    let (policy, lifetime) = ctx.effective_session_persistence_with_lifetime().unwrap();
    assert_eq!(policy.session_name.as_deref(), Some("http"));
    assert_eq!(lifetime, None);
    ctx.route_unit = None;
    assert_eq!(ctx.effective_session_persistence_with_lifetime().unwrap().1, Some(5));
}
