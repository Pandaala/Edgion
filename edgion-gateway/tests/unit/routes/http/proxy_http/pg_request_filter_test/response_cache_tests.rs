use super::*;
use crate::response_cache::tests::Harness;
use crate::PendingHeaderOp;
use pingora_proxy::{ProxyHttp, ResponseBodySink, UpstreamResponseBodyEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn proxy() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("127.0.0.1:80/HTTP".into()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    }
}

struct RequestFixture {
    proxy: EdgionHttpProxy,
    owner: Arc<crate::plugins::publication::GatewayScope>,
    publication: Arc<crate::plugins::publication::HttpListenerPublication>,
    _index: crate::runtime::store::listener_gateway_info::TestListenerIndexGuard,
}

struct BoundOriginFixture;
impl BoundOriginFixture {
    fn new() -> Self {
        use crate::backends::discovery::get_service_discovery_coordinator;
        use std::collections::{HashMap, HashSet};
        let coordinator = get_service_discovery_coordinator();
        let guard = Self;
        let service = serde_json::from_value(serde_json::json!({
            "metadata":{"namespace":"cache-c0","name":"origin","uid":"cache-c1-origin"},
            "spec":{"ports":[{"name":"http","port":8080,"targetPort":8080}]}
        }))
        .unwrap();
        let slice = serde_json::from_value(serde_json::json!({
            "metadata":{"namespace":"cache-c0","name":"origin-cache-c1-slice","uid":"cache-c1-slice", "labels":{"kubernetes.io/service-name":"origin"}},
            "addressType":"IPv4","ports":[{"name":"http","port":8080,"protocol":"TCP"}],
            "endpoints":[{"addresses":["10.9.0.7"],"conditions":{"ready":true}}]
        })).unwrap();
        coordinator.update_services(
            HashMap::from([("cache-c0/origin".into(), Arc::new(service))]),
            HashMap::new(),
            HashSet::new(),
        );
        coordinator.update_endpoint_slices(
            HashMap::from([("cache-c0/origin-cache-c1-slice".into(), Arc::new(slice))]),
            HashMap::new(),
            HashSet::new(),
        );
        guard
    }
}
impl Drop for BoundOriginFixture {
    fn drop(&mut self) {
        use std::collections::{HashMap, HashSet};
        let coordinator = crate::backends::discovery::get_service_discovery_coordinator();
        coordinator.update_services(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["cache-c0/origin".into()]),
        );
        coordinator.update_endpoint_slices(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["cache-c0/origin-cache-c1-slice".into()]),
        );
    }
}

#[tokio::test]
async fn response_cache_first_actual_tail_binding_and_consumed_peer_fill_then_hit_and_input_miss() {
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_request_filter::response_cache_tests",
        "response_cache_first_actual_tail_binding_and_consumed_peer_fill_then_hit_and_input_miss",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "response_cache_first_actual_tail_binding_and_consumed_peer_fill_then_hit_and_input_miss",
        || async {
            let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
            // Cache stamps include the global BackendTrafficPolicy generation; keep
            // concurrent store writers out so a fill stays valid for the next hit.
            let _traffic_serial = crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock();
            use crate::plugins::http::response_cache::ResponseCachePolicy;
            let mut fixture = RequestFixture::new(serde_json::json!([]));
            let _origin = BoundOriginFixture::new();
            let mut cache = Harness::new(1024);
            cache.stamp_unique_publication(&mut fixture.publication);
            let policy = ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig::default()).unwrap();
            for (index, target) in ["/cache", "/cache", "/cache?changed=true"].into_iter().enumerate() {
                let (mut client, server) = tokio::io::duplex(64 * 1024);
                client
                    .write_all(format!("GET {target} HTTP/1.1\r\nHost: cache-c0.example.test\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut session = Session::new_h1(Box::new(server));
                session.read_request().await.unwrap();
                let mut ctx = fixture.proxy.new_ctx();
                ctx.capture_listener_publication(fixture.publication.clone());
                ctx.capture_plugin_scope(fixture.owner.clone());
                ctx.request_info.in_request_body_stage = true;
                assert_eq!(
                    crate::response_cache::nominate(&mut ctx, &policy, cache.local_store()),
                    crate::plugins::runtime::traits::session::ResponseCacheIntentAdmission::Admitted
                );
                let before = cache.lookup_count();
                let handled = request_filter(&fixture.proxy, &mut session, &mut ctx).await.unwrap();
                assert_eq!(
                    cache.lookup_count(),
                    before + 1,
                    "actual tail binds and reaches canonical lookup"
                );
                assert_eq!(handled, index == 1);
                if index == 0 {
                    fixture
                        .proxy
                        .upstream_peer(&mut session, &mut ctx)
                        .await
                        .expect("real consumed peer selection");
                    let mut header = cache.response();
                    header.insert_header("content-length", "5").unwrap();
                    fixture
                        .proxy
                        .upstream_response_filter(&mut session, &mut header, &mut ctx)
                        .await
                        .unwrap();
                    fixture
                        .proxy
                        .response_filter(&mut session, &mut header, &mut ctx)
                        .await
                        .unwrap();
                    let mut body = Some(bytes::Bytes::from_static(b"hello"));
                    fixture
                        .proxy
                        .upstream_response_body_filter_event(
                            &mut session,
                            &mut body,
                            UpstreamResponseBodyEvent::Data { end_of_stream: true },
                            &mut ResponseBodySink::default(),
                            &mut ctx,
                        )
                        .await
                        .unwrap();
                    fixture
                        .proxy
                        .response_head_will_commit(&session, &header, &mut ctx)
                        .unwrap();
                    session.write_response_header(Box::new(header), false).await.unwrap();
                    session.write_response_body(body, true).await.unwrap();
                    fixture.proxy.logging(&mut session, None, &mut ctx).await;
                    assert_eq!(cache.entry_count(), 1, "only real consumed peer evidence admits a fill");
                } else if index == 1 {
                    assert!(ctx.get_current_upstream().is_none());
                    assert_eq!(ctx.try_cnt, 0);
                    assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
                } else {
                    assert!(session.response_written().is_none());
                }
                drop(session);
                drop(ctx);
                let mut wire = String::new();
                client.read_to_string(&mut wire).await.unwrap();
                if index != 2 {
                    assert!(wire.ends_with("hello"));
                }
            }
        },
    )
    .await;
}

impl RequestFixture {
    fn new(request_plugins: serde_json::Value) -> Self {
        Self::with_rules(
            request_plugins,
            serde_json::json!([{
                "matches":[{"path":{"type":"PathPrefix","value":"/"}}],
                "filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"request-policy"}}],
                "backendRefs":[{"name":"origin","port":8080}]
            }]),
        )
    }

    fn with_rules(request_plugins: serde_json::Value, rules: serde_json::Value) -> Self {
        use crate::plugins::publication::{
            inputs::{GatewayInput, RouteInput},
            PluginPublisher,
        };
        use crate::routes::http::{
            conf_handler_impl::build_domain_route_rules_from_routes, routes_mgr::DomainRouteRules,
        };
        use crate::runtime::store::listener_gateway_info::{ListenerGatewayInfoStore, TestListenerIndexGuard};
        use edgion_resources::{
            EdgionGatewayConfig, EdgionPlugins, Gateway, GatewayClass, HTTPRoute, ListenerKey, ResourceMeta,
        };
        use serde_json::json;
        use std::collections::HashMap;
        let key = ListenerKey::new("0.0.0.0:18599".parse().unwrap(), "HTTP");
        let gateway: Gateway = crate::test_current_status::from_value(json!({
            "metadata":{"namespace":"cache-c0", "name":"gateway"},
            "spec":{"gatewayClassName":"cache-c0", "listeners":[{"name":"http", "port":18599, "protocol":"HTTP"}]}
        }))
        .unwrap();
        let class: GatewayClass = serde_json::from_value(json!({
            "metadata":{"name":"cache-c0"}, "spec":{"controllerName":"edgion.io/gateway-controller",
            "parametersRef":{"group":"edgion.io", "kind":"EdgionGatewayConfig", "name":"cache-c0"}}
        }))
        .unwrap();
        let config: EdgionGatewayConfig =
            serde_json::from_value(json!({"metadata":{"name":"cache-c0"},"spec":{}})).unwrap();
        let mut plugins: EdgionPlugins = serde_json::from_value(json!({
            "metadata":{"namespace":"cache-c0", "name":"request-policy"}, "spec":{"requestPlugins":request_plugins}
        }))
        .unwrap();
        plugins.pre_parse();
        assert!(plugins.validate_plugins().is_empty());
        let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
            "metadata":{"namespace":"cache-c0", "name":"route"},
            "spec":{"resolvedStatusController":"edgion.io/gateway-controller", "parentRefs":[{"name":"gateway", "sectionName":"http"}],
                "hostnames":["cache-c0.example.test"], "rules":rules}
        })).unwrap();
        route.spec.resolved_listeners = Some(vec![key.clone()]);
        route.pre_parse();
        let route = Arc::new(route);
        let publisher = PluginPublisher::new();
        publisher.replace_gateway_classes(HashMap::from([("cache-c0".into(), Arc::new(class))]));
        publisher.replace_gateway_configs(HashMap::from([("cache-c0".into(), Arc::new(config))]));
        publisher.replace_gateways(HashMap::from([(
            "cache-c0/gateway".into(),
            GatewayInput::from_gateway(&gateway),
        )]));
        publisher.replace_http_plugins(HashMap::from([("cache-c0/request-policy".into(), Arc::new(plugins))]));
        publisher.replace_http_routes(
            HashMap::from([("cache-c0/route".into(), RouteInput::from_http(route.clone()))]),
            HashMap::new(),
        );
        let owner = publisher.snapshot().gateway("cache-c0/gateway").unwrap();
        assert!(owner.scope.available && owner.scope.ai_rule(&route, 0));
        let mut proxy = proxy();
        proxy.listener_port = 18599;
        proxy.listener_key = key.clone();
        proxy
            .http_route_manager
            .store_route_table(build_domain_route_rules_from_routes(
                &HashMap::from([("cache-c0/route".into(), route)]),
                &DomainRouteRules::new(),
                &crate::routes::delegation::DelegationSlotTable::new(),
            ));
        publisher.update_http_routes(
            HashMap::new(),
            &Default::default(),
            HashMap::from([(
                key.clone(),
                (
                    Arc::downgrade(&proxy.http_route_manager),
                    Arc::clone(&proxy.http_route_manager.load_route_table()),
                ),
            )]),
        );
        let publication = publisher.snapshot().http_listener(&key);
        let store = ListenerGatewayInfoStore::new();
        store.rebuild(&[gateway]);
        let index = store.get_index(&key);
        assert!(index.get("cache-c0/gateway", "http").is_some());
        let guard = TestListenerIndexGuard::install(key, index);
        Self {
            proxy,
            owner,
            publication,
            _index: guard,
        }
    }

    async fn request(
        &self,
        cache: &Harness,
        credential: &str,
        origins: &std::sync::atomic::AtomicUsize,
    ) -> (Session, tokio::io::DuplexStream) {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let request = format!("GET /cache HTTP/1.1\r\nHost: cache-c0.example.test\r\nx-c0-key: {credential}\r\nx-c0-quota: same-caller\r\n\r\n");
        client.write_all(request.as_bytes()).await.unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        ctx.capture_listener_publication(self.publication.clone());
        ctx.capture_plugin_scope(self.owner.clone());
        cache.arm(&mut ctx);
        // Observe the real request-stage decision at the origin dispatch gate.
        // No peer-selection or fake upstream context is fabricated by this seam.
        if !request_filter(&self.proxy, &mut session, &mut ctx).await.unwrap() {
            origins.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        assert!(
            ctx.route_unit.is_some(),
            "every request reaches the real published route"
        );
        (session, client)
    }
}

#[tokio::test]
async fn populated_cache_stays_behind_real_auth_and_quota_denials_and_corruption_falls_through() {
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let auth = json!({"type":"KeyAuth", "config":{"keySources":[{"type":"header", "name":"x-c0-key"}],
        "hideCredentials":true, "upstreamHeaderFields":[], "resolvedKeys":{"allowed":{"headers":{}, "group":"c0"}}}});
    {
        let fixture = RequestFixture::new(json!([auth.clone()]));
        let cache = Harness::new(1024);
        cache.fill(b"shared");
        let origins = AtomicUsize::new(0);
        for credential in ["wrong", "allowed"] {
            let before = cache.lookup_count();
            let (session, mut client) = fixture.request(&cache, credential, &origins).await;
            let denied = credential == "wrong";
            assert_eq!(
                session.response_written().unwrap().status.as_u16(),
                if denied { 401 } else { 200 }
            );
            assert_eq!(cache.lookup_count() - before, u64::from(!denied));
            if !denied {
                assert!(!session.req_header().headers.contains_key("x-c0-key"));
            }
            assert_eq!(origins.load(Ordering::Acquire), 0);
            drop(session);
            let mut wire = String::new();
            client.read_to_string(&mut wire).await.unwrap();
            assert_eq!(wire.ends_with("shared"), !denied);
        }
        cache.corrupt_tail(&[0]);
        let (session, _client) = fixture.request(&cache, "allowed", &origins).await;
        assert!(
            session.response_written().is_none(),
            "corrupt metadata fails before any downstream header"
        );
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(origins.load(Ordering::Acquire), 1);
    }
    {
        let fixture = RequestFixture::new(
            json!([auth, {"type":"RateLimitLocal", "config":{"rate":1, "interval":"1h", "key":[], "showLimitHeaders":false}}]),
        );
        let cache = Harness::new(1024);
        cache.fill(b"shared");
        let origins = AtomicUsize::new(0);
        for denied in [false, true] {
            let before = cache.lookup_count();
            let (session, mut client) = fixture.request(&cache, "allowed", &origins).await;
            assert_eq!(
                session.response_written().unwrap().status.as_u16(),
                if denied { 429 } else { 200 }
            );
            assert_eq!(cache.lookup_count() - before, u64::from(!denied));
            assert_eq!(origins.load(Ordering::Acquire), 0);
            assert_eq!(cache.entry_count(), 1);
            drop(session);
            let mut wire = String::new();
            client.read_to_string(&mut wire).await.unwrap();
            assert_eq!(wire.ends_with("shared"), !denied);
        }
    }
}

#[tokio::test]
async fn actual_miss_callbacks_then_hit_onion_preserve_current_queued_headers() {
    let proxy = proxy();
    let fixture = Harness::new(1024);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = proxy.new_ctx();
    fixture.arm(&mut ctx);
    assert!(!respond_from_cache(&proxy, &mut session, &mut ctx).await.unwrap());
    let mut header = fixture.response();
    header.insert_header("content-length", "5").unwrap();
    proxy
        .upstream_response_filter(&mut session, &mut header, &mut ctx)
        .await
        .unwrap();
    proxy
        .response_filter(&mut session, &mut header, &mut ctx)
        .await
        .unwrap();
    let mut body = Some(bytes::Bytes::from_static(b"hello"));
    let mut sink = ResponseBodySink::default();
    proxy
        .upstream_response_body_filter_event(
            &mut session,
            &mut body,
            UpstreamResponseBodyEvent::Data { end_of_stream: true },
            &mut sink,
            &mut ctx,
        )
        .await
        .unwrap();
    assert_eq!(fixture.entry_count(), 0);
    proxy.response_head_will_commit(&session, &header, &mut ctx).unwrap();
    session.write_response_header(Box::new(header), false).await.unwrap();
    session.write_response_body(body, true).await.unwrap();
    proxy.logging(&mut session, None, &mut ctx).await;
    assert_eq!(fixture.entry_count(), 1);
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.ends_with("hello"));

    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    fixture.arm(&mut ctx);
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "x-current".into(),
        value: http::HeaderValue::from_static("fresh"),
    });
    assert!(respond_from_cache(&proxy, &mut session, &mut ctx).await.unwrap());
    assert!(ctx.get_current_upstream().is_none());
    assert_eq!(ctx.try_cnt, 0);
    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert_eq!(session.response_written().unwrap().headers["x-current"], "fresh");
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.ends_with("hello"));
    assert!(wire.contains("x-current: fresh"));
}

#[tokio::test]
async fn hit_runs_current_cors_trace_and_veto_once_then_next_request_misses() {
    use edgion_resources::{CorsConfig, EdgionPlugin, RequestFilterEntry};
    let fixture = Harness::new(1024);
    fixture.fill(b"shared");
    let proxy = proxy();
    let mut runtime = PluginRuntime::new();
    let config = CorsConfig {
        allow_origins: "https://current.test".into(),
        ..Default::default()
    };
    let errors = runtime.add_from_request_filters(
        &[RequestFilterEntry::new(EdgionPlugin::Cors(config))],
        "default",
        "current-cors",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    runtime.test_add_counted_veto(effects.clone());
    let runtime = Arc::new(runtime);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client.write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\nOrigin: https://current.test\r\ntraceparent: 00-11111111111111111111111111111111-2222222222222222-01\r\n\r\n").await.unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    fixture.arm(&mut ctx);
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    runtime.run_request_plugins(&mut session, &mut ctx).await;
    assert!(!ctx.plugin_terminated());
    {
        use crate::plugins::runtime::{traits::PluginSession, CompiledDyePlan, PingoraSessionAdapter};
        use edgion_resources::resources::edgion_plugins::{DyeOutcome, DyeSuite};
        let dye: DyeSuite = yaml_serde::from_str("response:\n  - name: x-current-dye\n    on: [fail-open]\n").unwrap();
        ctx.install_current_dye_plan(Some(CompiledDyePlan::compile(&dye, "rate_limit_redis").unwrap()));
        PingoraSessionAdapter::new(&mut session, &mut ctx).record_outcome(DyeOutcome::FailOpen, "current");
    }
    ctx.install_resolved_global_plugins(Some(Arc::from(vec![runtime])));
    assert!(respond_from_cache(&proxy, &mut session, &mut ctx).await.unwrap());
    assert_eq!(
        effects.load(std::sync::atomic::Ordering::Acquire),
        1,
        "selected hit runs response effects exactly once"
    );
    assert_eq!(fixture.entry_count(), 0);
    assert!(ctx.get_current_upstream().is_none());
    let header = session.response_written().unwrap();
    assert_eq!(header.headers["cache-control"], "no-store");
    assert_eq!(header.headers["x-current-dye"], "rate_limit_redis:fail-open:current");
    assert_eq!(header.headers.get_all("x-current-dye").iter().count(), 1);
    assert_eq!(header.headers["access-control-allow-origin"], "https://current.test");
    assert_eq!(header.headers["x-trace-id"], "11111111111111111111111111111111");
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.ends_with("shared"));
    assert_eq!(wire.matches("HTTP/1.1 200").count(), 1);
    let (mut client, server) = tokio::io::duplex(1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    fixture.arm(&mut ctx);
    assert!(!respond_from_cache(&proxy, &mut session, &mut ctx).await.unwrap());
    assert!(session.response_written().is_none());
}

#[tokio::test]
async fn actual_typed_header_only_terminal_publishes_but_trailer_boundary_discards() {
    for trailers in [false, true] {
        let fixture = Harness::new(1024);
        let proxy = proxy();
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
            .await
            .unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = EdgionHttpContext::default();
        fixture.arm(&mut ctx);
        let mut header = fixture.response();
        header.insert_header("content-length", "0").unwrap();
        proxy
            .upstream_response_filter(&mut session, &mut header, &mut ctx)
            .await
            .unwrap();
        proxy
            .response_filter(&mut session, &mut header, &mut ctx)
            .await
            .unwrap();
        let mut body = None;
        let event = if trailers {
            UpstreamResponseBodyEvent::TerminalBeforeTrailers
        } else {
            UpstreamResponseBodyEvent::TerminalWithoutTrailers
        };
        proxy
            .upstream_response_body_filter_event(
                &mut session,
                &mut body,
                event,
                &mut ResponseBodySink::default(),
                &mut ctx,
            )
            .await
            .unwrap();
        if trailers {
            let mut metadata = http::HeaderMap::new();
            metadata.insert("x-terminal", http::HeaderValue::from_static("present"));
            proxy
                .upstream_response_trailer_filter(&mut session, &mut metadata, &mut ctx)
                .await
                .unwrap();
        }
        assert_eq!(fixture.entry_count(), 0);
        proxy.response_head_will_commit(&session, &header, &mut ctx).unwrap();
        session.write_response_header(Box::new(header), false).await.unwrap();
        session.write_response_body(None, true).await.unwrap();
        crate::response_cache::lifecycle::finalize(&mut session, &mut ctx, None).await;
        assert_eq!(fixture.entry_count(), usize::from(!trailers));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_response_denial_discards_private_fill_and_never_delivers_origin_body() {
    use edgion_resources::{DslConfig, EdgionPlugin, UpstreamResponseEntry};
    let fixture = Harness::new(1024);
    let proxy = proxy();
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_upstream_responses(
        &[UpstreamResponseEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "c0-denial".into(),
            source: Some("return deny(403, \"denied\")".into()),
            ..Default::default()
        }))],
        "c0-denial",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    fixture.arm(&mut ctx);
    ctx.install_resolved_global_plugins(Some(Arc::from(vec![Arc::new(runtime)])));
    let mut header = fixture.response();
    proxy
        .upstream_response_filter(&mut session, &mut header, &mut ctx)
        .await
        .unwrap();
    assert!(fixture.pending_bytes() > 0);
    let error = proxy
        .response_filter(&mut session, &mut header, &mut ctx)
        .await
        .unwrap_err();
    crate::response_cache::lifecycle::finalize(&mut session, &mut ctx, Some(&error)).await;
    assert_eq!(fixture.entry_count(), 0);
    assert_eq!(fixture.pending_bytes(), 0);
    assert_eq!(session.response_written().unwrap().status.as_u16(), 403);
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.starts_with("HTTP/1.1 403"));
    assert!(!wire.contains("shared"));
}

impl RequestFixture {
    async fn actual_tail(
        &self,
        cache: &Harness,
        policy: &Arc<crate::plugins::http::response_cache::ResponseCachePolicy>,
        wire: &str,
    ) -> (Session, EdgionHttpContext, tokio::io::DuplexStream, bool) {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        client.write_all(wire.as_bytes()).await.unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = self.proxy.new_ctx();
        ctx.capture_listener_publication(self.publication.clone());
        ctx.capture_plugin_scope(self.owner.clone());
        ctx.request_info.in_request_body_stage = true;
        assert_eq!(
            crate::response_cache::nominate(&mut ctx, policy, cache.local_store()),
            crate::plugins::runtime::traits::session::ResponseCacheIntentAdmission::Admitted
        );
        let handled = request_filter(&self.proxy, &mut session, &mut ctx).await.unwrap();
        (session, ctx, client, handled)
    }

    async fn finish_actual_origin(&self, cache: &Harness, session: &mut Session, ctx: &mut EdgionHttpContext) {
        self.proxy
            .upstream_peer(session, ctx)
            .await
            .expect("real consumed origin selection");
        self.finish_selected_origin(cache, session, ctx).await;
    }

    async fn finish_selected_origin(&self, cache: &Harness, session: &mut Session, ctx: &mut EdgionHttpContext) {
        let mut header = cache.response();
        header.insert_header("content-length", "5").unwrap();
        self.proxy
            .upstream_response_filter(session, &mut header, ctx)
            .await
            .unwrap();
        self.proxy.response_filter(session, &mut header, ctx).await.unwrap();
        let mut body = Some(bytes::Bytes::from_static(b"hello"));
        self.proxy
            .upstream_response_body_filter_event(
                session,
                &mut body,
                UpstreamResponseBodyEvent::Data { end_of_stream: true },
                &mut ResponseBodySink::default(),
                ctx,
            )
            .await
            .unwrap();
        self.proxy.response_head_will_commit(session, &header, ctx).unwrap();
        session.write_response_header(Box::new(header), false).await.unwrap();
        session.write_response_body(body, true).await.unwrap();
        self.proxy.logging(session, None, ctx).await;
    }
}

#[tokio::test]
async fn response_cache_real_tail_ranges_and_conditionals_never_lookup_or_capture() {
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_request_filter::response_cache_tests",
        "response_cache_real_tail_ranges_and_conditionals_never_lookup_or_capture",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "response_cache_real_tail_ranges_and_conditionals_never_lookup_or_capture",
        || async {
            let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
            // Cache stamps include the global BackendTrafficPolicy generation; keep
            // concurrent store writers out so lookups observe a stable generation.
            let _traffic_serial = crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock();
            use crate::plugins::http::response_cache::ResponseCachePolicy;
            let mut fixture = RequestFixture::new(serde_json::json!([]));
            let _origin = BoundOriginFixture::new();
            let mut cache = Harness::new(1024);
            cache.stamp_unique_publication(&mut fixture.publication);
            let policy = ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig::default()).unwrap();
            let (mut session, mut ctx, _client, handled) = fixture
                .actual_tail(
                    &cache,
                    &policy,
                    "GET /cache HTTP/1.1\r\nHost: cache-c0.example.test\r\n\r\n",
                )
                .await;
            assert!(!handled);
            fixture.finish_actual_origin(&cache, &mut session, &mut ctx).await;
            drop(session);
            drop(ctx);
            assert_eq!(cache.entry_count(), 1);
            let lookups = cache.lookup_count();
            for (name, value) in [
                ("Range", "bytes=0-1"),
                ("If-Range", "\"version\""),
                ("If-Match", "*"),
                ("If-None-Match", "\"version\""),
                ("If-Modified-Since", "Wed, 21 Oct 2015 07:28:00 GMT"),
                ("If-Unmodified-Since", "Wed, 21 Oct 2015 07:28:00 GMT"),
            ] {
                for _ in 0..2 {
                    let wire = format!("GET /cache HTTP/1.1\r\nHost: cache-c0.example.test\r\n{name}: {value}\r\n\r\n");
                    let (mut session, mut ctx, _client, handled) = fixture.actual_tail(&cache, &policy, &wire).await;
                    assert!(!handled, "{name} remains origin-owned even with a populated cache");
                    assert_eq!(cache.lookup_count(), lookups);
                    assert!(ctx.response_cache.is_none(), "no capture owner survives bypass");
                    assert_eq!(cache.pending_bytes(), 0);
                    assert_eq!(session.req_header().headers.get(name).unwrap(), value);
                    fixture.finish_actual_origin(&cache, &mut session, &mut ctx).await;
                    assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
                    assert_eq!(cache.entry_count(), 1);
                }
            }
        },
    )
    .await;
}

#[tokio::test]
async fn response_cache_actual_graphql_tail_shares_final_bytes_across_matches_and_body_generations_only() {
    crate::test_subprocess::assert_in_subprocess_async(
        "routes::http::proxy_http::pg_request_filter::response_cache_tests",
        "response_cache_actual_graphql_tail_shares_final_bytes_across_matches_and_body_generations_only",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "response_cache_actual_graphql_tail_shares_final_bytes_across_matches_and_body_generations_only",
        || async {
            let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
            // Cache stamps include the global BackendTrafficPolicy generation; keep
            // concurrent store writers out so a fill stays valid for the next hit.
            let _traffic_serial = crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock();
            use crate::plugins::http::response_cache::ResponseCachePolicy;
            use serde_json::json;
            const ENVELOPE: &str = r#"{"query":"query Q { ping }","operationName":"Q","variables":{"n":1}}"#;
            let script = format!(
                "if req.header(\"x-transport\") == \"id\" {{ json.set_body({}) }}",
                serde_json::to_string(ENVELOPE).unwrap()
            );
            let filters = json!([{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"request-policy"}}]);
            let backend = json!([{"name":"origin","port":8080}]);
            let mut fixture = RequestFixture::with_rules(
                json!([
                    {"type":"Dsl","config":{"name":"cache-reconstruction","source":script}},
                    {"type":"ProxyRewrite","config":{"uri":"/graphql","headers":{"remove":["x-transport"]}}},
                    {"type":"GraphqlGuard","config":{}}
                ]),
                json!([
                    {"matches":[
                        {"path":{"type":"Exact","value":"/graphql"},"headers":[{"name":"x-transport","value":"full","type":"Exact"}]},
                        {"path":{"type":"Exact","value":"/graphql"},"headers":[{"name":"x-transport","value":"id","type":"Exact"}]}
                    ],"filters":filters,"backendRefs":backend},
                    {"matches":[{"path":{"type":"Exact","value":"/other"}}],"filters":filters,"backendRefs":backend}
                ]),
            );
            let _origin = BoundOriginFixture::new();
            let mut cache = Harness::new(1024);
            cache.stamp_unique_publication(&mut fixture.publication);
            let policy = ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig {
                graphql_queries: true,
                ..Default::default()
            })
            .unwrap();
            let mut first_generation = None;
            for (index, (path, transport, input)) in [
                ("/graphql", "full", ENVELOPE),
                ("/graphql", "id", r#"{"id":"Q","variables":{"n":1}}"#),
                (
                    "/graphql",
                    "full",
                    r#"{"query":"query Q { ping }","operationName":"Q","variables":{"n":2}}"#,
                ),
                ("/other", "full", ENVELOPE),
            ]
            .into_iter()
            .enumerate()
            {
                let wire = format!("POST {path} HTTP/1.1\r\nHost: cache-c0.example.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nx-transport: {transport}\r\n\r\n{input}", input.len());
                let before = cache.lookup_count();
                let (mut session, mut ctx, _client, handled) = fixture.actual_tail(&cache, &policy, &wire).await;
                assert_eq!(cache.lookup_count(), before + 1, "successful real guard reaches lookup");
                let unit = ctx.route_unit.as_ref().unwrap();
                assert_eq!(
                    (unit.matched_info.rule_id, unit.matched_info.match_id),
                    match index {
                        1 => (0, 1),
                        3 => (1, 0),
                        _ => (0, 0),
                    }
                );
                assert_eq!(ctx.path(), "/graphql");
                let generation = ctx
                    .response_cache_graphql_proof
                    .get()
                    .expect("real successful GraphqlGuard evidence")
                    .generation;
                if handled {
                    assert!(
                        ctx.request_body.generation().is_none(),
                        "cached response releases the late request body view"
                    );
                } else {
                    assert_eq!(ctx.request_body.generation(), Some(generation));
                }
                assert_eq!(
                    handled,
                    index == 1,
                    "final envelope shares across matches; variables and rule remain isolated"
                );
                if index == 0 {
                    first_generation = Some(generation);
                    fixture.finish_actual_origin(&cache, &mut session, &mut ctx).await;
                    assert_eq!(cache.entry_count(), 1);
                } else if index == 1 {
                    assert_ne!(
                        Some(generation),
                        first_generation,
                        "reconstruction publishes a different physical body generation"
                    );
                    assert_eq!(ctx.try_cnt, 0);
                    assert!(ctx.get_current_upstream().is_none());
                }
            }
        },
    ).await;
}

struct ActualPolicyCleanup {
    tls_before: Option<Arc<edgion_resources::BackendTLSPolicy>>,
    _traffic_serial: crate::backends::degrade_threshold::GlobalStoreTestGuard<'static>,
}
impl ActualPolicyCleanup {
    const TLS_KEY: &'static str = "cache-c0/cache-c1-drift-tls";
    const TRAFFIC_KEY: &'static str = "cache-c0/cache-c1-drift-traffic";
    fn new() -> Self {
        Self {
            _traffic_serial: crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock(),
            tls_before: crate::backends::policy::backend_tls::get_global_backend_tls_policy_store().get(Self::TLS_KEY),
        }
    }
}
impl Drop for ActualPolicyCleanup {
    fn drop(&mut self) {
        use std::collections::{HashMap, HashSet};
        let (update, remove) = if let Some(prior) = self.tls_before.take() {
            (HashMap::from([(Self::TLS_KEY.into(), prior)]), HashSet::new())
        } else {
            (HashMap::new(), HashSet::from([Self::TLS_KEY.into()]))
        };
        crate::backends::policy::backend_tls::get_global_backend_tls_policy_store().update(
            HashMap::new(),
            update,
            &remove,
        );
        crate::backends::policy::get_backend_traffic_policy_store().partial_update_policies(
            HashMap::new(),
            HashMap::new(),
            HashSet::from([Self::TRAFFIC_KEY.into()]),
        );
    }
}

#[tokio::test]
async fn response_cache_real_tail_to_peer_store_reload_discards_fill_and_preserves_ordinary_capture() {
    let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
    use crate::backends::policy::{backend_tls::get_global_backend_tls_policy_store, get_backend_traffic_policy_store};
    use crate::plugins::http::response_cache::ResponseCachePolicy;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    for kind in ["mapping", "tls", "traffic"] {
        let _cleanup = ActualPolicyCleanup::new();
        let mut fixture = RequestFixture::new(json!([]));
        let _origin = BoundOriginFixture::new();
        let mut cache = Harness::new(1024);
        cache.stamp_unique_publication(&mut fixture.publication);
        let policy = ResponseCachePolicy::compile(&edgion_resources::ResponseCacheConfig::default()).unwrap();
        let (mut session, mut ctx, _client, handled) = fixture
            .actual_tail(
                &cache,
                &policy,
                "GET /cache HTTP/1.1\r\nHost: cache-c0.example.test\r\n\r\n",
            )
            .await;
        assert!(!handled);
        assert_eq!(cache.lookup_count(), 1);
        assert!(ctx.response_cache.is_some(), "real tail bound an expected origin");
        match kind {
            "mapping" => {
                let coordinator = crate::backends::discovery::get_service_discovery_coordinator();
                let service = serde_json::from_value(json!({
                    "metadata":{"namespace":"cache-c0","name":"origin","uid":"cache-c1-origin"},
                    "spec":{"ports":[{"name":"http-next","port":8080,"targetPort":8081}]}
                }))
                .unwrap();
                let slice = serde_json::from_value(json!({
                    "metadata":{"namespace":"cache-c0","name":"origin-cache-c1-slice","uid":"cache-c1-slice","labels":{"kubernetes.io/service-name":"origin"}},
                    "addressType":"IPv4","ports":[{"name":"http-next","port":8081,"protocol":"TCP"}],
                    "endpoints":[{"addresses":["10.9.0.7"],"conditions":{"ready":true}}]
                })).unwrap();
                coordinator.update_services(
                    HashMap::new(),
                    HashMap::from([("cache-c0/origin".into(), Arc::new(service))]),
                    HashSet::new(),
                );
                coordinator.update_endpoint_slices(
                    HashMap::new(),
                    HashMap::from([("cache-c0/origin-cache-c1-slice".into(), Arc::new(slice))]),
                    HashSet::new(),
                );
            }
            "tls" => {
                let store = get_global_backend_tls_policy_store();
                let before = store.snapshot().generation();
                // A real unrelated policy reload changes the consumed global TLS generation,
                // while this origin's actual TLS result remains absent and plaintext.
                let tls = serde_json::from_value(json!({
                    "metadata":{"namespace":"cache-c0","name":"cache-c1-drift-tls"},
                    "spec":{"targetRefs":[{"group":"","kind":"Service","name":"unrelated-drift-origin"}],
                        "resolvedTargetClass":"service","preparedPrecedence":{},"preparedAncestorAdmission":{"mode":"unrestricted"},"validation":{"hostname":"backend.example.com","wellKnownCACertificates":"System"},"useSystemCa":true}
                })).unwrap();
                store.update(
                    HashMap::from([(ActualPolicyCleanup::TLS_KEY.into(), Arc::new(tls))]),
                    HashMap::new(),
                    &HashSet::new(),
                );
                assert_ne!(store.snapshot().generation(), before);
            }
            "traffic" => {
                let store = get_backend_traffic_policy_store();
                let before = store.snapshot().generation();
                let traffic = serde_json::from_value(json!({
                    "metadata":{"namespace":"cache-c0","name":"cache-c1-drift-traffic"},
                    "spec":{"targetRefs":[{"group":"","kind":"Service","name":"origin"}],
                        "resolvedTargetClass":"service","preparedPrecedence":{},"preparedAncestorAdmission":{"mode":"unrestricted"},"connection":{"connectTimeout":"1s"}}
                }))
                .unwrap();
                store.partial_update_policies(
                    HashMap::from([(ActualPolicyCleanup::TRAFFIC_KEY.into(), Arc::new(traffic))]),
                    HashMap::new(),
                    HashSet::new(),
                );
                assert_ne!(store.snapshot().generation(), before);
            }
            _ => unreachable!(),
        }
        let peer = fixture
            .proxy
            .upstream_peer(&mut session, &mut ctx)
            .await
            .expect("ordinary origin selection survives late reload");
        if kind == "mapping" {
            assert_eq!(peer._address.as_inet().unwrap().port(), 8081);
        }
        if kind == "traffic" {
            assert!(
                ctx.backend_context.as_ref().unwrap().effective_policy.is_some(),
                "ordinary absence-to-present capture still consumes the newly published policy"
            );
        }
        fixture.finish_selected_origin(&cache, &mut session, &mut ctx).await;
        assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
        assert_eq!(
            cache.entry_count(),
            0,
            "{kind}: actual consumed reload cannot publish stale expected-origin entry"
        );
        assert_eq!(cache.pending_bytes(), 0, "{kind}: discarded fill releases its arena");
    }
}

#[tokio::test]
async fn common_peer_barrier_projects_late_query_edits_and_rejects_failed_projection() {
    use crate::plugins::runtime::{PingoraSessionAdapter, PluginSession};
    let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
    // Cache stamps include the global BackendTrafficPolicy generation; keep
    // concurrent store writers out so lookups observe a stable generation.
    let _traffic_serial = crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock();
    let fixture = RequestFixture::new(serde_json::json!([]));
    let _origin = BoundOriginFixture::new();
    for fail_projection in [false, true] {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        client
            .write_all(b"GET /cache?token=secret&keep=1 HTTP/1.1\r\nHost: cache-c0.example.test\r\n\r\n")
            .await
            .unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        let mut ctx = fixture.proxy.new_ctx();
        ctx.capture_listener_publication(fixture.publication.clone());
        ctx.capture_plugin_scope(fixture.owner.clone());
        assert!(!request_filter(&fixture.proxy, &mut session, &mut ctx).await.unwrap());
        ctx.uri_finalized = true;
        PingoraSessionAdapter::new(&mut session, &mut ctx)
            .remove_request_query_param("token")
            .unwrap();
        assert_eq!(session.req_header().uri.query(), Some("token=secret&keep=1"));
        if fail_projection {
            session.req_header_mut().set_uri("*".parse().unwrap());
        }
        let result = fixture.proxy.upstream_peer(&mut session, &mut ctx).await;
        if fail_projection {
            let error = result.unwrap_err();
            assert!(matches!(error.etype(), pingora_core::ErrorType::HTTPStatus(500)));
            assert!(!error.retry());
            assert!(ctx.request_query.projection_is_dirty());
            assert_eq!(session.req_header().raw_path(), b"*");
            ctx.request_info.is_grpc_request = true;
            fixture.proxy.fail_to_proxy(&mut session, &error, &mut ctx).await;
            assert_eq!(session.response_written().unwrap().status.as_u16(), 200);
            assert_eq!(ctx.request_info.grpc_status(), Some(13));
            assert_eq!(ctx.final_downstream_status(), Some(200));
            drop(session);
            let mut wire = String::new();
            client.read_to_string(&mut wire).await.unwrap();
            assert!(wire.starts_with("HTTP/1.1 200"));
            assert!(wire.to_ascii_lowercase().contains("grpc-status: 13"));
            assert!(!wire.contains("secret"));
        } else {
            result.unwrap();
            assert_eq!(session.req_header().uri.query(), Some("keep=1"));
            assert!(!ctx.request_query.projection_is_dirty());
            assert!(ctx.uri_finalized);
        }
    }
}

#[tokio::test]
async fn common_peer_projection_failure_keeps_sealed_transcoder_rest_error_contract() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
    // Cache stamps include the global BackendTrafficPolicy generation; keep
    // concurrent store writers out so lookups observe a stable generation.
    let _traffic_serial = crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK.lock();
    let descriptor = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-tests/proto/grpc_json_transcoding/test.pb"
    ));
    let fixture = RequestFixture::new(serde_json::json!([{
        "type":"GrpcJsonTranscoder", "config":{
            "descriptor":{"type":"inline","base64":STANDARD.encode(descriptor)},
            "services":["test.TestService"]
        }
    }]));
    let _origin = BoundOriginFixture::new();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /v1/hello/name HTTP/1.1\r\nHost: cache-c0.example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    let mut ctx = fixture.proxy.new_ctx();
    ctx.capture_listener_publication(fixture.publication.clone());
    ctx.capture_plugin_scope(fixture.owner.clone());
    assert!(!request_filter(&fixture.proxy, &mut session, &mut ctx).await.unwrap());
    assert!(ctx.grpc_transcoding.is_some());
    // Inject corruption after a real seal to exercise the concrete projection
    // failure boundary; ordinary plugin mutations cannot alter this owner.
    ctx.uri_finalized = true;
    ctx.request_query.replace_raw(Some("late=secret".into())).unwrap();
    session.req_header_mut().set_uri("*".parse().unwrap());
    let error = fixture.proxy.upstream_peer(&mut session, &mut ctx).await.unwrap_err();
    assert!(matches!(error.etype(), pingora_core::ErrorType::HTTPStatus(502)));
    assert!(!error.retry());
    assert!(ctx.request_query.projection_is_dirty());
    fixture.proxy.fail_to_proxy(&mut session, &error, &mut ctx).await;
    assert_eq!(session.response_written().unwrap().status.as_u16(), 502);
    assert_eq!(ctx.final_downstream_status(), Some(502));
    assert!(session.response_written().unwrap().headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    drop(session);
    let mut wire = String::new();
    client.read_to_string(&mut wire).await.unwrap();
    assert!(wire.starts_with("HTTP/1.1 502"));
    assert!(!wire.contains("secret"));
}
