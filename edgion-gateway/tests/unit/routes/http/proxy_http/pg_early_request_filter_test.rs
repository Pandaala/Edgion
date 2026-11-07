use super::*;
use edgion_resources::ResourceMeta;
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
    assert_eq!(ctx.request_info.discover_protocol, Some("grpc-web"));
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

    assert_eq!(ctx.request_info.discover_protocol, Some("grpc-web"));
    assert!(ctx.request_info.is_grpc_request);
    assert!(!super::super::pg_request_filter::native_grpc_requires_frontend_http2(
        ctx.request_info.discover_protocol,
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
    assert_eq!(ctx.request_info.discover_protocol, Some("websocket"));
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
        ctx.request_info.discover_protocol,
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

    assert_eq!(ctx.request_info.discover_protocol, Some("grpc"));
    assert_eq!(
        **session
            .downstream_modules_ctx
            .get::<GrpcWebBridge>()
            .expect("gRPC-Web bridge module"),
        GrpcWebCtx::Disabled
    );
    assert!(super::super::pg_request_filter::native_grpc_requires_frontend_http2(
        ctx.request_info.discover_protocol,
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
async fn early_global_generation_and_missing_reference_stay_pinned_and_deny() {
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
    // gw-core-02: the missing global target is pinned as a fail-closed
    // placeholder and denies the request before any plugin request ID runs.
    assert!(ctx.plugin_terminated());
    assert!(session.req_header().headers.contains_key("x-trace-id"));
    assert!(!session.req_header().headers.contains_key("x-first-id"));
    let pinned = ctx.resolved_global_plugins().unwrap();
    assert_eq!(pinned.len(), 2);
    assert!(Arc::ptr_eq(&pinned[0], &first.plugin_runtime));
    assert!(pinned[1].rejects_invalid_config());
    store.replace_all(std::collections::HashMap::from([
        ("early-id-test/present".into(), id_plugin("x-reloaded-id")),
        ("early-id-test/missing".into(), id_plugin("x-was-missing-id")),
    ]));
    configure_owners(&store, &["present", "missing"], false, &[]);
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut ctx, store.publisher())
        .await
        .unwrap();
    let repinned = ctx.resolved_global_plugins().unwrap();
    assert_eq!(repinned.len(), 2);
    assert!(Arc::ptr_eq(&repinned[0], &first.plugin_runtime));
    assert!(Arc::ptr_eq(&repinned[1], &pinned[1]));
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
    capture_matched_scope(&mut ctx).unwrap();
    initialize_gateway_request_ids(&mut session, &mut ctx).await.unwrap();
    // gw-core-02: the missing Gateway target is pinned as a fail-closed
    // placeholder and denies the request.
    assert!(ctx.plugin_terminated());
    let pinned = ctx.resolved_gateway_plugins().unwrap();
    assert_eq!(pinned.len(), 2);
    assert!(pinned[1].rejects_invalid_config());
    let first = pinned[0].clone();
    assert!(!session.req_header().headers.contains_key("x-gateway-first-id"));
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
    let repinned = ctx.resolved_gateway_plugins().unwrap();
    assert_eq!(repinned.len(), 2);
    assert!(Arc::ptr_eq(&repinned[0], &first));
    assert!(Arc::ptr_eq(&repinned[1], &pinned[1]));
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
        None,
    )));
    assert!(
        capture_matched_scope(&mut ctx).is_err(),
        "a different matched Gateway cannot inherit the captured policy"
    );
}

fn coherent_grpc_route(generation: &str) -> Arc<edgion_resources::GRPCRoute> {
    let mut route: edgion_resources::GRPCRoute = crate::test_current_status::from_value(serde_json::json!({
                    "metadata":{"namespace":"coherent","name":"grpc","annotations":{"edgion.io/sync-version":generation}},
                    "spec":{"resolvedStatusController":"edgion.io/gateway-controller","parentRefs":[{"name":"gateway"}],
                        "resolvedListeners":["0.0.0.0:18089/HTTP"],"hostnames":["coherent.example.com"],
                        "rules":[{"matches":[{"method":{"service":"svc","method":"Call"}}],"backendRefs":[{"name":"backend","port":8080}]}]}
                })).unwrap();
    route.pre_parse();
    Arc::new(route)
}

fn coherent_gateway() -> edgion_resources::Gateway {
    crate::test_current_status::from_value(serde_json::json!({
        "metadata":{"namespace":"coherent", "name":"gateway"},
        "spec":{"gatewayClassName":"coherent", "listeners":[{"name":"web", "protocol":"HTTP", "port":18089}]}
    }))
    .unwrap()
}

fn coherent_route(generation: &str) -> Arc<edgion_resources::HTTPRoute> {
    let mut route: edgion_resources::HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
        "metadata":{"namespace":"coherent", "name":"route", "annotations":{"edgion.io/sync-version":generation}},
        "spec":{"resolvedStatusController":"edgion.io/gateway-controller", "parentRefs":[{"name":"gateway"}],
            "resolvedListeners":["0.0.0.0:18089/HTTP"], "hostnames":["coherent.example.com"],
            "rules":[{"matches":[{"path":{"type":"PathPrefix","value":"/"}}],"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"policy"}}],"backendRefs":[{"name":"backend","port":8080}]}]}
    })).unwrap();
    route.pre_parse();
    Arc::new(route)
}

fn coherent_plugin(generation: &str) -> Arc<edgion_resources::EdgionPlugins> {
    Arc::new(serde_json::from_value(serde_json::json!({
        "metadata":{"namespace":"coherent","name":"policy"},
        "spec":{"requestPlugins":[{"type":"RequestHeaderModifier","config":{"set":[{"name":"x-generation","value":generation}]}}]}
    })).unwrap())
}

fn install_coherent_gateway() {
    use std::collections::HashMap;
    crate::runtime::store::get_listener_gateway_info_store().rebuild(&[coherent_gateway()]);
    let publisher = crate::plugins::publication::global_publisher();
    publisher.replace_http_plugins(HashMap::from([("coherent/policy".into(), coherent_plugin("1"))]));
    let class = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"coherent"}, "spec":{"controllerName":"edgion.io/gateway-controller"}
    }))
    .unwrap();
    publisher.replace_gateway_classes(HashMap::from([("coherent".into(), Arc::new(class))]));
    publisher.replace_gateways(HashMap::from([(
        "coherent/gateway".into(),
        crate::plugins::publication::inputs::GatewayInput::from_gateway(&coherent_gateway()),
    )]));
}

#[tokio::test]
async fn early_capture_and_real_route_commits_keep_exact_admission_generation() {
    const NAME: &str = "early_capture_and_real_route_commits_keep_exact_admission_generation";
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_early_request_filter::tests",
        NAME,
        "EDGION_COHERENT_ROUTE_CHILD",
        NAME,
        || async {
            use edgion_common::conf_sync::traits::ConfHandler;
            use std::collections::{HashMap, HashSet};
            let publisher = crate::plugins::publication::global_publisher();
            let manager = crate::routes::http::GlobalHttpRouteManagers::new();
            install_coherent_gateway();
            let first = coherent_route("1");
            manager.full_set(&HashMap::from([("coherent/route".into(), first.clone())]));
            let mut proxy = id_proxy();
            proxy.http_route_manager = manager.get_or_create_listener_manager(&proxy.listener_key);
            let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: coherent.example.com\r\n\r\n").await;
            let mut old = EdgionHttpContext::default();
            initialize_request_ids(&proxy, &mut session, &mut old).await.unwrap();
            old.request_info.hostname = crate::ctx::NormalizedHostname::new("coherent.example.com");
            old.request_info.normalized_current_path = "/".into();
            let view = old.listener_publication().unwrap().clone();
            let before = view
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &view.index)
                .unwrap();
            let owner = view.gateway("coherent/gateway").unwrap();
            assert!(Arc::ptr_eq(before.route_unit.route.as_ref().unwrap(), &first));
            assert!(owner.scope.ai_rule(&first, 0) && owner.scope.graphql_rule(&first, 0));

            // Plugin publication precedes the dependent Route, as in the live failure.
            publisher.update_http_plugins(
                HashMap::new(),
                HashMap::from([("coherent/policy".into(), coherent_plugin("2"))]),
                &HashSet::new(),
            );
            let intermediate = publisher.snapshot().http_listener(&proxy.listener_key);
            let intermediate_owner = intermediate.gateway("coherent/gateway").unwrap();
            let intermediate_match = intermediate
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &intermediate.index)
                .unwrap();
            assert!(Arc::ptr_eq(
                intermediate_match.route_unit.route.as_ref().unwrap(),
                &first
            ));
            assert!(intermediate_owner.scope.ai_rule(&first, 0) && intermediate_owner.scope.graphql_rule(&first, 0));
            assert!(!Arc::ptr_eq(
                &owner.scope.http("coherent/policy").unwrap(),
                &intermediate_owner.scope.http("coherent/policy").unwrap()
            ));
            drop(intermediate);
            let second = coherent_route("2");
            manager.partial_update(
                HashMap::new(),
                HashMap::from([("coherent/route".into(), second.clone())]),
                HashSet::new(),
            );
            initialize_request_ids(&proxy, &mut session, &mut old).await.unwrap();
            assert!(Arc::ptr_eq(old.listener_publication().unwrap(), &view));
            let retained = view
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &view.index)
                .unwrap();
            assert!(Arc::ptr_eq(retained.route_unit.route.as_ref().unwrap(), &first));
            let fresh = publisher.snapshot().http_listener(&proxy.listener_key);
            let selected = fresh
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &fresh.index)
                .unwrap();
            assert!(Arc::ptr_eq(selected.route_unit.route.as_ref().unwrap(), &second));
            let next_owner = fresh.gateway("coherent/gateway").unwrap();
            assert!(next_owner.scope.ai_rule(&second, 0) && next_owner.scope.graphql_rule(&second, 0));
            assert!(!owner.scope.ai_rule(&second, 0) && !owner.scope.graphql_rule(&second, 0));
            assert!(!next_owner.scope.ai_rule(&first, 0) && !next_owner.scope.graphql_rule(&first, 0));

            // Gateway-driven rebuilding commits the existing source with its new tables.
            manager.rebuild_all_listener_managers();
            let rebuilt = publisher.snapshot().http_listener(&proxy.listener_key);
            let selected = rebuilt
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &rebuilt.index)
                .unwrap();
            assert!(rebuilt
                .gateway("coherent/gateway")
                .unwrap()
                .scope
                .ai_rule(selected.route_unit.route.as_ref().unwrap(), 0));
            manager.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["coherent/route".into()]));
            let removed = publisher.snapshot().http_listener(&proxy.listener_key);
            assert!(removed
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &removed.index)
                .is_err());
            assert!(view
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &view.index)
                .is_ok());
            manager.full_set(&HashMap::from([("coherent/route".into(), coherent_route("3"))]));
            let recreated = publisher.snapshot().http_listener(&proxy.listener_key);
            let selected = recreated
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &recreated.index)
                .unwrap();
            assert!(recreated
                .gateway("coherent/gateway")
                .unwrap()
                .scope
                .graphql_rule(selected.route_unit.route.as_ref().unwrap(), 0));
            let weak = Arc::downgrade(&view);
            drop(old);
            drop(view);
            assert!(
                weak.upgrade().is_none(),
                "retired listener view is released after its request"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn captured_absence_survives_listener_creation_before_matching() {
    const NAME: &str = "captured_absence_survives_listener_creation_before_matching";
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_early_request_filter::tests",
        NAME,
        "EDGION_COHERENT_ABSENCE_CHILD",
        NAME,
        || async {
            use edgion_common::conf_sync::traits::ConfHandler;
            use std::collections::HashMap;
            let manager = crate::routes::http::GlobalHttpRouteManagers::new();
            let mut proxy = id_proxy();
            proxy.http_route_manager = manager.get_or_create_listener_manager(&proxy.listener_key);
            let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: coherent.example.com\r\n\r\n").await;
            let mut old = EdgionHttpContext::default();
            initialize_request_ids(&proxy, &mut session, &mut old).await.unwrap();
            let missing = old.listener_publication().unwrap().clone();
            assert!(missing.index.is_empty());
            install_coherent_gateway();
            manager.full_set(&HashMap::from([("coherent/route".into(), coherent_route("1"))]));
            initialize_request_ids(&proxy, &mut session, &mut old).await.unwrap();
            assert!(Arc::ptr_eq(old.listener_publication().unwrap(), &missing));
            assert!(old.plugin_scope().is_none());
            old.request_info.hostname = crate::ctx::NormalizedHostname::new("coherent.example.com");
            old.request_info.normalized_current_path = "/".into();
            assert!(missing
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &missing.index)
                .is_err());
            let mut next = EdgionHttpContext::default();
            initialize_request_ids(&proxy, &mut session, &mut next).await.unwrap();
            assert!(next.plugin_scope().is_some());
            let fresh = next.listener_publication().unwrap();
            assert!(fresh
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &fresh.index)
                .is_ok());
        },
    )
    .await;
}

#[tokio::test]
async fn shared_listener_captures_owner_policy_before_owner_is_selected() {
    let store = crate::plugins::http::PluginStore::new();
    store.replace_all(std::collections::HashMap::from([(
        "early-id-test/gateway-present".into(),
        id_plugin("x-pinned-id"),
    )]));
    let keys = ["early-id-test/gateway-present"];
    configure_owners(&store, &[], true, &keys);
    let proxy = id_proxy();
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut ctx, store.publisher())
        .await
        .unwrap();
    let pinned = ctx.listener_publication().unwrap().clone();
    assert!(ctx.plugin_scope().is_none());
    store.replace_all(std::collections::HashMap::from([(
        "early-id-test/gateway-present".into(),
        id_plugin("x-new-id"),
    )]));
    configure_owners(&store, &[], true, &keys);
    ctx.gateway_info = Some(id_gateway(&keys));
    capture_matched_scope(&mut ctx).unwrap();
    initialize_gateway_request_ids(&mut session, &mut ctx).await.unwrap();
    assert!(session.req_header().headers.contains_key("x-pinned-id"));
    assert!(!session.req_header().headers.contains_key("x-new-id"));
    assert!(Arc::ptr_eq(
        ctx.plugin_scope().unwrap(),
        &pinned.gateway("early-id-test/gateway").unwrap()
    ));
    let selected = ctx.listener_publication().unwrap();
    assert!(Arc::ptr_eq(
        &selected.http_routes(&proxy.http_route_manager),
        &pinned.http_routes(&proxy.http_route_manager)
    ));
    assert!(Arc::ptr_eq(
        &selected.grpc_routes(&proxy.grpc_route_manager),
        &pinned.grpc_routes(&proxy.grpc_route_manager)
    ));
    assert!(Arc::ptr_eq(&selected.index, &pinned.index));
    let weak_child = Arc::downgrade(selected);
    let weak_entry = Arc::downgrade(&pinned);
    drop(pinned);
    assert!(
        weak_entry.upgrade().is_none(),
        "selected child does not retain the shared entry or siblings"
    );
    assert!(weak_child.upgrade().is_some());
    ctx.gateway_info = Some(Arc::new(crate::runtime::GatewayInfo::new(
        Some("early-id-test".into()),
        "other".into(),
        Some("http".into()),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    )));
    assert!(
        capture_matched_scope(&mut ctx).is_err(),
        "selected owner is first-writer-wins"
    );
    drop(ctx);
    assert!(
        weak_child.upgrade().is_none(),
        "selected child is released with its request"
    );
    let mut fresh = EdgionHttpContext::default();
    initialize_request_ids_with_publisher(&proxy, &mut session, &mut fresh, store.publisher())
        .await
        .unwrap();
    fresh.gateway_info = Some(id_gateway(&keys));
    capture_matched_scope(&mut fresh).unwrap();
    initialize_gateway_request_ids(&mut session, &mut fresh).await.unwrap();
    assert!(session.req_header().headers.contains_key("x-new-id"));
}

#[tokio::test]
async fn grpc_commits_and_http_fallback_share_captured_listener_generation() {
    const NAME: &str = "grpc_commits_and_http_fallback_share_captured_listener_generation";
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_early_request_filter::tests", NAME, "EDGION_COHERENT_GRPC_CHILD", NAME, || async {
            use edgion_common::conf_sync::traits::ConfHandler;
            use std::collections::{HashMap, HashSet};
            install_coherent_gateway();
            let http = crate::routes::http::GlobalHttpRouteManagers::new();
            http.full_set(&HashMap::from([("coherent/route".into(), coherent_route("1"))]));
            let grpc = crate::routes::grpc::GlobalGrpcRouteManagers::new();
            let first = coherent_grpc_route("1");
            grpc.full_set(&HashMap::from([("coherent/grpc".into(), first.clone())]));
            let mut proxy = id_proxy();
            proxy.http_route_manager = http.get_or_create_listener_manager(&proxy.listener_key);
            proxy.grpc_route_manager = grpc.get_or_create_listener_manager(&proxy.listener_key);
            let mut session = session_from_request(b"POST /svc/Call HTTP/1.1\r\nHost: coherent.example.com\r\nContent-Type: application/grpc\r\nContent-Length: 0\r\n\r\n").await;
            let mut ctx = EdgionHttpContext::default();
            initialize_request_ids(&proxy, &mut session, &mut ctx).await.unwrap();
            ctx.request_info.hostname = crate::ctx::NormalizedHostname::new("coherent.example.com");
            ctx.request_info.normalized_current_path = "/svc/Call".into();
            let old = ctx.listener_publication().unwrap().clone();
            let matched = old.grpc_routes(&proxy.grpc_route_manager).match_route(&mut session, &ctx, &old.index, "coherent.example.com").unwrap().route_unit;
            assert!(Arc::ptr_eq(matched.route.as_ref().unwrap(), &first));
            let second = coherent_grpc_route("2");
            grpc.partial_update(HashMap::new(), HashMap::from([("coherent/grpc".into(), second.clone())]), HashSet::new());
            let fresh = crate::plugins::publication::global_publisher().snapshot().http_listener(&proxy.listener_key);
            assert!(Arc::ptr_eq(&old.http_routes(&proxy.http_route_manager), &fresh.http_routes(&proxy.http_route_manager)), "a GRPC commit retains the HTTP table");
            assert!(Arc::ptr_eq(fresh.grpc_routes(&proxy.grpc_route_manager).match_route(&mut session, &ctx, &fresh.index, "coherent.example.com").unwrap().route_unit.route.as_ref().unwrap(), &second));
            assert!(Arc::ptr_eq(old.grpc_routes(&proxy.grpc_route_manager).match_route(&mut session, &ctx, &old.index, "coherent.example.com").unwrap().route_unit.route.as_ref().unwrap(), &first));
            grpc.rebuild_all_listener_managers();
            grpc.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["coherent/grpc".into()]));
            let removed = crate::plugins::publication::global_publisher().snapshot().http_listener(&proxy.listener_key);
            assert!(!removed.grpc_routes(&proxy.grpc_route_manager).has_routes());
            assert!(old.grpc_routes(&proxy.grpc_route_manager).has_routes());
            let fallback = removed.http_routes(&proxy.http_route_manager).match_route(&mut session, &ctx, &removed.index).unwrap();
            assert!(removed.gateway("coherent/gateway").unwrap().scope.ai_rule(fallback.route_unit.route.as_ref().unwrap(), 0));
            grpc.full_set(&HashMap::from([("coherent/grpc".into(), coherent_grpc_route("3"))]));
            assert!(crate::plugins::publication::global_publisher().snapshot().http_listener(&proxy.listener_key).grpc_routes(&proxy.grpc_route_manager).has_routes());
        }
    ).await;
}

async fn coherent_entry(proxy: &EdgionHttpProxy, grpc: bool) -> (EdgionHttpContext, Session, bool) {
    use pingora_proxy::ProxyHttp;
    let (mut client, server) = tokio::io::duplex(16384);
    let mut session = Session::new_h1(Box::new(server));
    let content_type = if grpc { "application/grpc" } else { "application/json" };
    client.write_all(format!("POST /svc/Call HTTP/1.1\r\nHost: coherent.example.com\r\nContent-Type: {content_type}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
    assert!(session.read_request().await.unwrap());
    let mut ctx = proxy.new_ctx();
    early_request_filter(proxy, &mut session, &mut ctx).await.unwrap();
    let ended = super::super::pg_request_filter::request_filter(proxy, &mut session, &mut ctx)
        .await
        .unwrap();
    (ctx, session, ended)
}

#[tokio::test]
async fn retired_manager_identity_preserves_physical_listener_boundary() {
    const NAME: &str = "retired_manager_identity_preserves_physical_listener_boundary";
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_early_request_filter::tests",
        NAME,
        "EDGION_COHERENT_RETIRE_CHILD",
        NAME,
        || async {
            use edgion_common::conf_sync::traits::ConfHandler;
            use std::collections::HashMap;
            let publisher = crate::plugins::publication::global_publisher();
            let http = crate::routes::http::GlobalHttpRouteManagers::new();
            let grpc = crate::routes::grpc::GlobalGrpcRouteManagers::new();
            install_coherent_gateway();
            // Public Gateway-triggered commit paths precreate active, empty managers.
            http.rebuild_all_listener_managers();
            grpc.rebuild_all_listener_managers();
            let mut proxy = id_proxy();
            proxy.enable_http2 = true;
            proxy.http_route_manager = http.get_or_create_listener_manager(&proxy.listener_key);
            proxy.grpc_route_manager = grpc.get_or_create_listener_manager(&proxy.listener_key);
            let (_, session, ended) = coherent_entry(&proxy, false).await;
            assert!(ended);
            assert_eq!(session.response_written().unwrap().status, StatusCode::NOT_FOUND);
            http.full_set(&HashMap::from([("coherent/route".into(), coherent_route("1"))]));
            grpc.full_set(&HashMap::from([("coherent/grpc".into(), coherent_grpc_route("1"))]));
            assert!(Arc::ptr_eq(
                &proxy.http_route_manager,
                &http.get_or_create_listener_manager(&proxy.listener_key)
            ));
            assert!(Arc::ptr_eq(
                &proxy.grpc_route_manager,
                &grpc.get_or_create_listener_manager(&proxy.listener_key)
            ));
            let (old, mut session, ended) = coherent_entry(&proxy, false).await;
            assert!(!ended);
            let pinned = old.listener_publication().unwrap().clone();
            assert!(old.route_unit.is_some());
            let (old_grpc, _, ended) = coherent_entry(&proxy, true).await;
            assert!(!ended && old_grpc.is_grpc_route_matched);
            // Remove routes, then the logical owner; drive the same public family
            // rebuild/commit methods called by GatewayHandler on key removal.
            http.full_set(&HashMap::new());
            grpc.full_set(&HashMap::new());
            crate::runtime::store::get_listener_gateway_info_store().rebuild(&[]);
            http.rebuild_all_listener_managers();
            grpc.rebuild_all_listener_managers();
            publisher.replace_gateways(HashMap::new());
            assert_eq!(http.stats().listener_count, 0);
            assert_eq!(grpc.stats().listener_count, 0);
            install_coherent_gateway();
            http.rebuild_all_listener_managers();
            grpc.rebuild_all_listener_managers();
            http.full_set(&HashMap::from([("coherent/route".into(), coherent_route("2"))]));
            grpc.full_set(&HashMap::from([("coherent/grpc".into(), coherent_grpc_route("2"))]));
            let new_http = http.get_or_create_listener_manager(&proxy.listener_key);
            let new_grpc = grpc.get_or_create_listener_manager(&proxy.listener_key);
            assert!(!Arc::ptr_eq(&proxy.http_route_manager, &new_http));
            assert!(!Arc::ptr_eq(&proxy.grpc_route_manager, &new_grpc));
            for request_grpc in [false, true] {
                let (ctx, response, ended) = coherent_entry(&proxy, request_grpc).await;
                assert!(ended);
                let response = response.response_written().unwrap();
                if request_grpc {
                    assert_eq!(response.status, StatusCode::OK);
                    assert_eq!(response.headers.get("grpc-status").unwrap(), "5");
                    assert_eq!(ctx.request_info.grpc_status(), Some(5));
                } else {
                    assert_eq!(response.status, StatusCode::NOT_FOUND);
                }
                assert!(ctx
                    .diagnostics
                    .contains_error(edgion_resources::EdgionErrorCode::RouteNotFound));
                assert!(!ctx.is_grpc_route_matched && ctx.route_unit.is_none());
            }
            let retained = pinned
                .http_routes(&proxy.http_route_manager)
                .match_route(&mut session, &old, &pinned.index)
                .unwrap();
            assert!(pinned
                .gateway("coherent/gateway")
                .unwrap()
                .scope
                .ai_rule(retained.route_unit.route.as_ref().unwrap(), 0));
            proxy.http_route_manager = new_http;
            proxy.grpc_route_manager = new_grpc;
            let (ctx, _, ended) = coherent_entry(&proxy, false).await;
            assert!(!ended && ctx.route_unit.is_some());
            let (ctx, _, ended) = coherent_entry(&proxy, true).await;
            assert!(!ended && ctx.is_grpc_route_matched);
        },
    )
    .await;
}

#[tokio::test]
async fn family_retirement_preserves_other_family_and_grpc_http_fallback() {
    const NAME: &str = "family_retirement_preserves_other_family_and_grpc_http_fallback";
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_early_request_filter::tests",
        NAME,
        "EDGION_COHERENT_FAMILY_CHILD",
        NAME,
        || async {
            use edgion_common::conf_sync::traits::ConfHandler;
            use std::collections::HashMap;
            for retire_grpc in [true, false] {
                let publisher = crate::plugins::publication::global_publisher();
                let http = crate::routes::http::GlobalHttpRouteManagers::new();
                let grpc = crate::routes::grpc::GlobalGrpcRouteManagers::new();
                install_coherent_gateway();
                http.full_set(&HashMap::from([("coherent/route".into(), coherent_route("1"))]));
                grpc.full_set(&HashMap::from([("coherent/grpc".into(), coherent_grpc_route("1"))]));
                let mut proxy = id_proxy();
                proxy.enable_http2 = true;
                proxy.http_route_manager = http.get_or_create_listener_manager(&proxy.listener_key);
                proxy.grpc_route_manager = grpc.get_or_create_listener_manager(&proxy.listener_key);
                let (ctx, _, ended) = coherent_entry(&proxy, true).await;
                assert!(!ended && ctx.is_grpc_route_matched);
                if retire_grpc {
                    grpc.full_set(&HashMap::new());
                } else {
                    http.full_set(&HashMap::new());
                }
                crate::runtime::store::get_listener_gateway_info_store().rebuild(&[]);
                http.rebuild_all_listener_managers();
                grpc.rebuild_all_listener_managers();
                publisher.replace_gateways(HashMap::new());
                assert_eq!(http.stats().listener_count, usize::from(retire_grpc));
                assert_eq!(grpc.stats().listener_count, usize::from(!retire_grpc));
                install_coherent_gateway();
                http.rebuild_all_listener_managers();
                grpc.rebuild_all_listener_managers();
                if retire_grpc {
                    grpc.full_set(&HashMap::from([("coherent/grpc".into(), coherent_grpc_route("2"))]));
                } else {
                    http.full_set(&HashMap::from([("coherent/route".into(), coherent_route("2"))]));
                }
                let new_http = http.get_or_create_listener_manager(&proxy.listener_key);
                let new_grpc = grpc.get_or_create_listener_manager(&proxy.listener_key);
                assert_eq!(Arc::ptr_eq(&proxy.http_route_manager, &new_http), retire_grpc);
                assert_eq!(Arc::ptr_eq(&proxy.grpc_route_manager, &new_grpc), !retire_grpc);
                let (ctx, response, ended) = coherent_entry(&proxy, true).await;
                assert!(!ended, "retiring one family cannot reject the surviving family's route");
                assert!(
                    response.response_written().is_none(),
                    "inaccessible new gRPC routes cannot emit UNIMPLEMENTED"
                );
                assert_eq!(ctx.is_grpc_route_matched, !retire_grpc);
                if retire_grpc {
                    assert_eq!(ctx.route_unit.as_ref().unwrap().resource_key, "coherent/route");
                    let source = ctx.route_unit.as_ref().unwrap().route.as_ref().unwrap();
                    assert!(ctx.plugin_scope().unwrap().scope.ai_rule(source, 0));
                    assert!(ctx.plugin_scope().unwrap().scope.graphql_rule(source, 0));
                } else {
                    assert_eq!(ctx.grpc_route_unit.as_ref().unwrap().resource_key, "coherent/grpc");
                    let (_, response, ended) = coherent_entry(&proxy, false).await;
                    assert!(ended);
                    assert_eq!(response.response_written().unwrap().status, StatusCode::NOT_FOUND);
                }
                // A fresh physical handle can reach the newly created family.
                proxy.http_route_manager = new_http;
                proxy.grpc_route_manager = new_grpc;
                let (ctx, _, ended) = coherent_entry(&proxy, true).await;
                assert!(!ended && ctx.is_grpc_route_matched);
            }
        },
    )
    .await;
}
