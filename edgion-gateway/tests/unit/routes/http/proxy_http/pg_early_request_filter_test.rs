use super::*;
use http::header::CONTENT_LENGTH;
use http::StatusCode;
use pingora_core::apps::{HttpPersistentSettings, ReusedHttpStream};
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;
use pingora_core::protocols::http::bridge::grpc_web::GrpcWebCtx;
use pingora_http::ResponseHeader;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

async fn normal_listener_wire_request_count(request_limit: u32, request_cap: usize) -> (usize, String) {
    use pingora_proxy::ProxyHttp;

    let proxy = id_proxy();
    let config = Arc::new(crate::config::ResolvedGatewayConfig {
        downstream_keepalive_request_limit: request_limit,
        ..Default::default()
    });
    let (mut client, server) = tokio::io::duplex(8192);
    let mut next_stream: pingora_core::protocols::Stream = Box::new(server);
    let mut persistent_settings: Option<HttpPersistentSettings> = None;
    let mut served = 0;

    for request_index in 0..request_cap {
        let client_closes = request_limit == 0 && request_index + 1 == request_cap;
        let connection = if client_closes { "Connection: close\r\n" } else { "" };
        client
            .write_all(format!("GET /{request_index} HTTP/1.1\r\nHost: test\r\n{connection}\r\n").as_bytes())
            .await
            .unwrap();

        let mut session = Session::new_h1(next_stream);
        if let Some(settings) = persistent_settings.take() {
            settings.apply_to_session(&mut session.downstream_session);
        }
        assert!(session.read_request().await.unwrap());

        let previous = session.downstream_session.take_connection_user_context();
        let mut ctx = EdgionHttpContext::new();
        ctx.egc = Arc::clone(&config);
        if let Some(previous) = previous {
            proxy.on_connection_reuse(&mut session, &mut ctx, previous);
        }
        apply_client_policy(&mut session, &ctx.egc, &mut ctx.downstream_keepalive_initialized);

        let mut response = ResponseHeader::build(StatusCode::NO_CONTENT, Some(2)).unwrap();
        response.insert_header("Content-Length", "0").unwrap();
        session.write_response_header(Box::new(response), true).await.unwrap();

        let mut settings = HttpPersistentSettings::for_session(&session);
        if let Some(user_context) = proxy.persist_connection_context(&session, &ctx) {
            settings.set_user_context(user_context);
        }
        let reusable = session.downstream_session.finish().await.unwrap();
        served += 1;
        let Some(reusable) = reusable else {
            break;
        };
        let (stream, carried) = ReusedHttpStream::from_reusable_stream(reusable, settings).consume();
        next_stream = stream;
        persistent_settings = carried;
    }

    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    (served, wire)
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

#[tokio::test]
async fn keepalive_budget_is_captured_once_while_new_connections_take_new_policy() {
    use pingora_proxy::ProxyHttp;
    let proxy = id_proxy();
    let mut first = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::new();
    let config = crate::config::ResolvedGatewayConfig {
        downstream_keepalive_request_limit: 4,
        ..Default::default()
    };
    ctx.egc = Arc::new(config);
    apply_client_policy(&mut first, &ctx.egc, &mut ctx.downstream_keepalive_initialized);
    assert_eq!(first.get_keepalive_reuses_remaining(), Some(3));
    let persisted = proxy.persist_connection_context(&first, &ctx).unwrap();

    let config = crate::config::ResolvedGatewayConfig {
        downstream_keepalive_request_limit: 9,
        ..Default::default()
    };
    let mut next = EdgionHttpContext::new();
    next.egc = Arc::new(config);
    let mut reused = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    reused.set_keepalive_reuses_remaining(Some(3));
    proxy.on_connection_reuse(&mut reused, &mut next, persisted);
    apply_client_policy(&mut reused, &next.egc, &mut next.downstream_keepalive_initialized);
    assert_eq!(reused.get_keepalive_reuses_remaining(), Some(3));

    let mut fresh = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    apply_client_policy(&mut fresh, &next.egc, &mut false);
    assert_eq!(fresh.get_keepalive_reuses_remaining(), Some(8));
}

#[tokio::test]
async fn normal_listener_enforces_total_request_limit_on_the_wire() {
    for (request_limit, request_cap, expected_served, expected_keepalive) in [(0, 3, 3, 2), (1, 2, 1, 0), (2, 3, 2, 1)]
    {
        let (served, wire) = normal_listener_wire_request_count(request_limit, request_cap).await;
        let wire_lower = wire.to_ascii_lowercase();
        assert_eq!(served, expected_served, "request_limit={request_limit}");
        assert_eq!(
            wire.matches("HTTP/1.1 204").count(),
            expected_served,
            "request_limit={request_limit}, wire={wire:?}"
        );
        assert_eq!(
            wire_lower.matches("\r\nconnection: keep-alive\r\n").count(),
            expected_keepalive,
            "request_limit={request_limit}, wire={wire:?}"
        );
        assert_eq!(
            wire_lower.matches("\r\nconnection: close\r\n").count(),
            1,
            "request_limit={request_limit}, wire={wire:?}"
        );
    }
}

fn id_config(global_names: &[&str]) -> edgion_resources::EdgionGatewayConfig {
    let mut config = edgion_resources::EdgionGatewayConfig::new("early-id-test", Default::default());
    config.spec.global_plugins_ref = Some(
        global_names
            .iter()
            .map(
                |name| edgion_resources::resources::edgion_gateway_config::PluginReference {
                    name: (*name).to_owned(),
                    namespace: Some("early-id-test".to_owned()),
                },
            )
            .collect(),
    );
    config
}

fn id_proxy() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 18089,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("0.0.0.0:18089/HTTP".to_owned()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    }
}

fn id_plugin(header: &str) -> Arc<crate::plugins::http::CompiledPlugins> {
    let config = serde_json::from_value(serde_json::json!({
        "ids":[{"header":header,"source":{"type":"traceId"},"generateWhenMissing":false}]
    }))
    .unwrap();
    let resource = edgion_resources::EdgionPlugins::new(
        "early-id",
        edgion_resources::EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![edgion_resources::RequestFilterEntry::new(
                edgion_resources::EdgionPlugin::RequestId(config),
            )]),
            ..Default::default()
        },
    );
    crate::plugins::http::compile_plugins(&resource)
}

fn id_gateway(keys: &[&str]) -> Arc<crate::runtime::GatewayInfo> {
    Arc::new(crate::runtime::GatewayInfo::new(
        Some("early-id-test".into()),
        "gateway".into(),
        Some("http".into()),
        None,
        None,
        None,
        keys.iter().map(|key| (*key).to_owned()).collect(),
        None,
    ))
}

fn configure_owners(
    store: &crate::plugins::http::PluginStore,
    global_names: &[&str],
    ambiguous: bool,
    gateway_keys: &[&str],
) {
    use crate::plugins::publication::inputs::GatewayInput;
    use std::collections::HashMap;
    let mut config = id_config(global_names);
    config.metadata.name = Some("plugin-store-test".into());
    store
        .publisher()
        .replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
    let gateway: edgion_resources::Gateway = crate::test_current_status::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"Gateway","metadata":{"name":"gateway","namespace":"early-id-test"},
            "spec":{"gatewayClassName":"plugin-store-test","listeners":[{"name":"http","port":18089,"protocol":"HTTP"}]}
        })).unwrap();
    let mut input = GatewayInput::from_gateway(&gateway);
    input.http_keys = gateway_keys.iter().map(|key| (*key).to_string()).collect();
    let mut gateways = HashMap::from([("early-id-test/gateway".into(), input.clone())]);
    if ambiguous {
        gateways.insert("early-id-test/other".into(), input);
    }
    store.publisher().replace_gateways(gateways);
}

#[tokio::test]
async fn early_global_generation_and_missing_reference_stay_pinned() {
    let store = crate::plugins::http::PluginStore::new();
    store.replace_all(std::collections::HashMap::from([(
        "early-id-test/present".into(),
        id_plugin("x-first-id"),
    )]));
    let proxy = id_proxy();
    configure_owners(&store, &["present", "missing"], false, &[]);
    let first = store
        .publisher()
        .snapshot()
        .gateway("early-id-test/gateway")
        .unwrap()
        .scope
        .http("early-id-test/present")
        .unwrap();
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut ctx, store.publisher())
        .await
        .unwrap();
    assert!(ctx.global_request_ids_initialized && ctx.gateway_request_ids_initialized);
    assert!(session.req_header().headers.contains_key("x-trace-id"));
    assert!(session.req_header().headers.contains_key("x-first-id"));
    assert!(Arc::ptr_eq(
        &ctx.resolved_global_plugins().unwrap()[0],
        &first.plugin_runtime
    ));
    store.replace_all(std::collections::HashMap::from([
        ("early-id-test/present".into(), id_plugin("x-reloaded-id")),
        ("early-id-test/missing".into(), id_plugin("x-was-missing-id")),
    ]));
    configure_owners(&store, &["present", "missing"], false, &[]);
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut ctx, store.publisher())
        .await
        .unwrap();
    assert_eq!(ctx.resolved_global_plugins().unwrap().len(), 1);
    assert!(Arc::ptr_eq(
        &ctx.resolved_global_plugins().unwrap()[0],
        &first.plugin_runtime
    ));
    assert!(!session.req_header().headers.contains_key("x-reloaded-id"));
    assert!(!session.req_header().headers.contains_key("x-was-missing-id"));
    let mut next = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    initialize_request_ids_with_publisher(&proxy, &mut next, &mut EdgionHttpContext::default(), store.publisher())
        .await
        .unwrap();
    assert!(next.req_header().headers.contains_key("x-reloaded-id"));
    assert!(next.req_header().headers.contains_key("x-was-missing-id"));
}

#[tokio::test]
async fn deferred_gateway_owner_pins_hits_and_misses_when_first_known() {
    let store = crate::plugins::http::PluginStore::new();
    store.replace_all(std::collections::HashMap::from([(
        "early-id-test/gateway-present".into(),
        id_plugin("x-gateway-first-id"),
    )]));
    let proxy = id_proxy();
    let keys = ["early-id-test/gateway-present", "early-id-test/gateway-missing"];
    configure_owners(&store, &[], true, &keys);
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut ctx, store.publisher())
        .await
        .unwrap();
    assert!(ctx.plugin_scope().is_none());
    assert!(!session.req_header().headers.contains_key("x-gateway-first-id"));
    ctx.gateway_info = Some(id_gateway(&keys));
    capture_matched_scope_with_publisher(&mut ctx, store.publisher()).unwrap();
    initialize_gateway_request_ids(&mut session, &mut ctx).await.unwrap();
    let first = ctx.resolved_gateway_plugins().unwrap()[0].clone();
    assert!(session.req_header().headers.contains_key("x-gateway-first-id"));
    store.replace_all(std::collections::HashMap::from([
        (
            "early-id-test/gateway-present".into(),
            id_plugin("x-gateway-reloaded-id"),
        ),
        (
            "early-id-test/gateway-missing".into(),
            id_plugin("x-gateway-was-missing-id"),
        ),
    ]));
    configure_owners(&store, &[], true, &keys);
    initialize_gateway_request_ids(&mut session, &mut ctx).await.unwrap();
    assert_eq!(ctx.resolved_gateway_plugins().unwrap().len(), 1);
    assert!(Arc::ptr_eq(&ctx.resolved_gateway_plugins().unwrap()[0], &first));
    assert!(!session.req_header().headers.contains_key("x-gateway-reloaded-id"));
    assert!(!session.req_header().headers.contains_key("x-gateway-was-missing-id"));
    ctx.gateway_info = Some(Arc::new(crate::runtime::GatewayInfo::new(
        Some("early-id-test".into()),
        "other".into(),
        Some("http".into()),
        None,
        None,
        None,
        Vec::new(),
        None,
    )));
    assert!(
        capture_matched_scope_with_publisher(&mut ctx, store.publisher()).is_err(),
        "a different matched Gateway cannot inherit the captured policy"
    );
}
