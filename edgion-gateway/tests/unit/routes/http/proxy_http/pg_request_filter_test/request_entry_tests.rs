use super::*;
use crate::plugins::publication::inputs::{GatewayInput, RouteInput};
use crate::plugins::publication::{GatewayScope, PluginPublisher};
use crate::routes::grpc::{
    parse_route_to_units_for_test, DomainGrpcRouteRules, GrpcMatchEngine, GrpcPortRouteManager, GrpcRouteRules,
};
use crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes;
use crate::routes::http::routes_mgr::{DomainRouteRules, HttpPortRouteManager};
use crate::runtime::get_listener_gateway_info_store;
use crate::runtime::store::listener_gateway_info::{ListenerGatewayInfoStore, TestListenerIndexGuard};
use edgion_resources::{EdgionGatewayConfig, GRPCRoute, Gateway, GatewayClass, HTTPRoute, ListenerKey, ResourceMeta};
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::timeout;

const NAMESPACE: &str = "request-entry-test";
const OWNER: &str = "request-entry-test/gateway";
const HTTP_ROUTE: &str = "request-entry-test/http-fallback";

struct Fixture {
    proxy: EdgionHttpProxy,
    owner: Arc<GatewayScope>,
    publication: Arc<crate::plugins::publication::HttpListenerPublication>,
    egc: Arc<crate::config::ResolvedGatewayConfig>,
    _index: TestListenerIndexGuard,
}

impl Fixture {
    fn new(https: bool, http2: bool, global_421: bool, annotation_421: bool, grpc: bool) -> Self {
        let protocol = if https { "HTTPS" } else { "HTTP" };
        let listener_key = ListenerKey::new("0.0.0.0:18479".parse().unwrap(), protocol);
        let gateway: Gateway = crate::test_current_status::from_value(json!({
            "apiVersion":"gateway.networking.k8s.io/v1", "kind":"Gateway",
            "metadata":{"name":"gateway", "namespace":NAMESPACE,
                "annotations":{"edgion.io/require-sni-host-match":annotation_421.to_string()}},
            "spec":{"gatewayClassName":"request-entry-class", "listeners":[
                {"name":"sni", "port":18479, "protocol":protocol, "hostname":"sni.example.net"},
                {"name":"host", "port":18479, "protocol":protocol, "hostname":"*.example.com"}
            ]}
        }))
        .unwrap();
        let config: EdgionGatewayConfig = serde_json::from_value(json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionGatewayConfig",
            "metadata":{"name":"request-entry-config"},
            "spec":{"securityProtect":{"requireSniHostMatch":global_421}}
        }))
        .unwrap();
        let class: GatewayClass = serde_json::from_value(json!({
            "apiVersion":"gateway.networking.k8s.io/v1", "kind":"GatewayClass",
            "metadata":{"name":"request-entry-class"},
            "spec":{"controllerName":"edgion.io/gateway-controller", "parametersRef":{
                "group":"edgion.io", "kind":"EdgionGatewayConfig", "name":"request-entry-config"
            }}
        }))
        .unwrap();
        let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute",
                "metadata":{"name":"http-fallback", "namespace":NAMESPACE},
                "spec":{"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs":[{"name":"gateway", "sectionName":"host"}],
                    "hostnames":["*.example.com"], "rules":[{
                        "matches":[{"path":{"type":"PathPrefix", "value":"/"}}],
                        "filters":[{"type":"RequestHeaderModifier", "requestHeaderModifier":{
                            "set":[{"name":"x-entry-http-route", "value":"selected"}]
                        }}],
                        "backendRefs":[{"name":"unused-backend", "port":8080}]
                    }]}
            }))
            .unwrap();
        route.spec.resolved_listeners = Some(vec![listener_key.clone()]);
        route.pre_parse();
        let route = Arc::new(route);
        let config = Arc::new(config);
        let publisher = PluginPublisher::new();
        publisher.replace_gateway_classes(HashMap::from([("request-entry-class".into(), Arc::new(class))]));
        publisher.replace_gateway_configs(HashMap::from([("request-entry-config".into(), config.clone())]));
        publisher.replace_gateways(HashMap::from([(OWNER.into(), GatewayInput::from_gateway(&gateway))]));
        publisher.replace_http_routes(
            HashMap::from([(HTTP_ROUTE.into(), RouteInput::from_http(route.clone()))]),
            HashMap::new(),
        );
        let owner = publisher.snapshot().gateway(OWNER).unwrap();
        assert!(owner.scope.available && owner.scope.ai_rule(&route, 0));

        let http_route_manager = Arc::new(HttpPortRouteManager::default());
        http_route_manager.store_route_table(build_domain_route_rules_from_routes(
            &HashMap::from([(HTTP_ROUTE.into(), route)]),
            &DomainRouteRules::new(),
        ));
        let grpc_route_manager = Arc::new(GrpcPortRouteManager::default());
        if grpc {
            let mut route: GRPCRoute = crate::test_current_status::from_value(json!({
                    "apiVersion":"gateway.networking.k8s.io/v1", "kind":"GRPCRoute",
                    "metadata":{"name":"grpc-existing", "namespace":NAMESPACE},
                    "spec":{"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs":[{"name":"gateway", "sectionName":"host"}],
                        "hostnames":["*.example.com"], "rules":[{
                            "matches":[{"method":{"service":"svc", "method":"Existing"}}],
                            "backendRefs":[{"name":"unused-backend", "port":8080}]
                        }]}
                }))
                .unwrap();
            route.spec.resolved_listeners = Some(vec![listener_key.clone()]);
            route.pre_parse();
            let units = parse_route_to_units_for_test("request-entry-test/grpc-existing", &Arc::new(route));
            assert_eq!(units.len(), 1, "the unmatched table must contain a real compiled Route");
            let rules = GrpcRouteRules {
                resource_keys: units.iter().map(|unit| unit.resource_key.clone()).collect(),
                match_engine: Some(Arc::new(GrpcMatchEngine::new(units.clone()))),
                route_rules_list: units,
            };
            grpc_route_manager.store_route_table(DomainGrpcRouteRules {
                grpc_routes: Arc::new(rules),
            });
        }
        publisher.update_http_routes(
            HashMap::new(),
            &std::collections::HashSet::new(),
            HashMap::from([(
                listener_key.clone(),
                (
                    Arc::downgrade(&http_route_manager),
                    Arc::clone(&http_route_manager.load_route_table()),
                ),
            )]),
        );
        publisher.update_grpc_routes(
            HashMap::new(),
            &std::collections::HashSet::new(),
            HashMap::from([(
                listener_key.clone(),
                (
                    Arc::downgrade(&grpc_route_manager),
                    Arc::clone(&grpc_route_manager.load_route_table()),
                ),
            )]),
        );
        let publication = publisher.snapshot().http_listener(&listener_key);
        let store = ListenerGatewayInfoStore::new();
        store.rebuild(&[gateway]);
        let index = store.get_index(&listener_key);
        assert!(index.get(OWNER, "host").is_some());
        let index = TestListenerIndexGuard::install(listener_key.clone(), index);
        let proxy = EdgionHttpProxy {
            listener_port: 18479,
            is_https_listener: https,
            listener_key,
            server_header_opts: Default::default(),
            access_logger: Arc::new(crate::observe::AccessLogger::default()),
            egc: crate::config::bind_gateway_config(None),
            enable_http2: http2,
            preflight_handler: super::super::PreflightHandler,
            http_route_manager,
            grpc_route_manager,
        };
        Self {
            egc: Arc::new(crate::config::ResolvedGatewayConfig::resolve(&config)),
            proxy,
            owner,
            publication,
            _index: index,
        }
    }

    fn context(&self) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        // The actual early hook still initializes IDs. Only its owner input
        // is local, so parallel tests cannot replace this policy generation.
        ctx.egc = self.egc.clone();
        ctx.capture_listener_publication(self.publication.clone());
        ctx.capture_plugin_scope(self.owner.clone());
        ctx
    }
}

async fn session(content_type: &str) -> (Session, DuplexStream) {
    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let mut modules = HttpModules::new();
    modules.add_module(Box::new(GrpcWeb));
    let mut session = Session::new_h1_with_modules(Box::new(server), &modules);
    let request = format!(
            "POST /svc/Missing HTTP/1.1\r\nHost: A.B.EXAMPLE.COM.:18479\r\nContent-Type: {content_type}\r\nContent-Length: 0\r\n\r\n"
        );
    timeout(Duration::from_secs(2), client.write_all(request.as_bytes()))
        .await
        .unwrap()
        .unwrap();
    assert!(timeout(Duration::from_secs(2), session.read_request())
        .await
        .unwrap()
        .unwrap());
    (session, client)
}

async fn run_entry(fixture: &Fixture, session: &mut Session, ctx: &mut EdgionHttpContext) -> bool {
    timeout(Duration::from_secs(2), async {
        super::super::pg_early_request_filter::early_request_filter(&fixture.proxy, session, ctx)
            .await
            .unwrap();
        session
            .downstream_modules_ctx
            .request_header_filter(session.downstream_session.req_header_mut())
            .await
            .unwrap();
        request_filter(&fixture.proxy, session, ctx).await.unwrap()
    })
    .await
    .expect("in-memory entry must complete within two seconds")
}

fn assert_http_fallback(session: &Session, ctx: &EdgionHttpContext) {
    assert!(!ctx.is_grpc_route_matched);
    assert_eq!(ctx.route_unit.as_ref().unwrap().resource_key, HTTP_ROUTE);
    assert_eq!(
        session.req_header().headers.get("x-entry-http-route").unwrap(),
        "selected"
    );
    assert!(session.response_written().is_none());
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::Http2Required));
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::RouteNotFound));
}

#[tokio::test(flavor = "current_thread")]
async fn actual_entry_preserves_421_global_annotation_and_disabled_cases() {
    for (global, annotation, matched_listener, expected_reject) in [
        (true, false, "sni", true),
        (false, true, "sni", true),
        (false, false, "sni", false),
        (true, false, "host", false),
    ] {
        let fixture = Fixture::new(true, true, global, annotation, false);
        let (mut session, _client) = session("application/json").await;
        let mut ctx = fixture.context();
        // Install the existing handshake metadata representation, not a TLS
        // handshake or certificate test. The H1 stream has no SSL digest.
        let sni = if matched_listener == "sni" {
            "sni.example.net"
        } else {
            "a.b.example.com"
        };
        ctx.request_info.install_tls_info(
            None,
            Some(sni.into()),
            None,
            Some(matched_listener.into()),
            Some(OWNER.into()),
        );
        assert_eq!(run_entry(&fixture, &mut session, &mut ctx).await, expected_reject);
        assert_eq!(ctx.request_info.hostname.as_ref(), "a.b.example.com");
        assert_eq!(
            ctx.diagnostics.contains_error(EdgionErrorCode::SniHostMismatch),
            expected_reject
        );
        if expected_reject {
            assert_eq!(session.response_written().unwrap().status.as_u16(), 421);
            assert!(ctx.route_unit.is_none(), "421 must precede Route selection");
        } else {
            assert_http_fallback(&session, &ctx);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_entry_native_grpc_frontend_gate_and_unmatched_status_12() {
    // These are real H1 Sessions testing the configured frontend gate and
    // entry dispatch. They do not establish H2 negotiation/frame behavior.
    for (http2, grpc_routes, expected_status) in [(false, true, Some(13)), (true, true, Some(12)), (true, false, None)]
    {
        let fixture = Fixture::new(false, http2, false, false, grpc_routes);
        let (mut session, mut client) = session("application/grpc").await;
        let mut ctx = fixture.context();
        assert_eq!(
            run_entry(&fixture, &mut session, &mut ctx).await,
            expected_status.is_some()
        );
        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc"));
        assert_eq!(ctx.diagnostics.contains_error(EdgionErrorCode::Http2Required), !http2);
        if let Some(status) = expected_status {
            assert_eq!(ctx.request_info.grpc_status(), Some(status));
            let response = session.response_written().unwrap();
            assert_eq!(response.status.as_u16(), 200);
            assert_eq!(
                response.headers.get("grpc-status").unwrap().to_str().unwrap(),
                status.to_string()
            );
            assert!(
                ctx.route_unit.is_none(),
                "native rejection must not run the HTTP fallback"
            );
            assert_eq!(ctx.diagnostics.contains_error(EdgionErrorCode::RouteNotFound), http2);
            drop(session);
            let mut wire = Vec::new();
            timeout(
                Duration::from_secs(2),
                (&mut client).take(16 * 1024).read_to_end(&mut wire),
            )
            .await
            .unwrap()
            .unwrap();
            let wire = String::from_utf8(wire).unwrap().to_ascii_lowercase();
            assert!(wire.starts_with("http/1.1 200"));
            assert!(wire.contains(&format!("grpc-status: {status}\r\n")));
        } else {
            assert_http_fallback(&session, &ctx);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_entry_grpc_web_keeps_http_fallback_after_module_rewrite() {
    for http2 in [false, true] {
        let fixture = Fixture::new(false, http2, false, false, true);
        let (mut session, _client) = session("application/grpc-web+proto").await;
        let mut ctx = fixture.context();
        assert!(!run_entry(&fixture, &mut session, &mut ctx).await);
        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc-web"));
        assert_eq!(
            session.req_header().headers.get("content-type").unwrap(),
            "application/grpc+proto"
        );
        assert_http_fallback(&session, &ctx);
    }
}

#[test]
fn request_entry_index_override_restores_nested_state_on_unwind() {
    let key = ListenerKey::new("0.0.0.0:18479".parse().unwrap(), "HTTP");
    let store = get_listener_gateway_info_store();
    let baseline_empty = store.get_index(&key).is_empty();
    let fixture = Fixture::new(false, false, false, false, false);
    let outer = store.get_index(&key);
    assert!(outer.get(OWNER, "host").is_some());
    let empty = ListenerGatewayInfoStore::new().get_index(&ListenerKey("unused/HTTP".into()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _nested = TestListenerIndexGuard::install(key.clone(), empty);
        assert!(store.get_index(&key).is_empty());
        panic!("exercise the test-only unwind restoration");
    }));
    assert!(result.is_err());
    assert!(Arc::ptr_eq(&outer, &store.get_index(&key)));
    drop(fixture);
    assert_eq!(store.get_index(&key).is_empty(), baseline_empty);
}

#[tokio::test(flavor = "current_thread")]
async fn captured_empty_entry_keeps_404_and_metadata_400_precedence() {
    for (known_owner, malformed_host) in [(false, false), (true, false), (false, true)] {
        let fixture = Fixture::new(false, true, false, false, false);
        let publisher = PluginPublisher::new();
        if known_owner {
            let gateway: Gateway = crate::test_current_status::from_value(json!({
                "metadata":{"namespace":NAMESPACE,"name":"empty"},
                "spec":{"gatewayClassName":"empty","listeners":[{"name":"web","port":18479,"protocol":"HTTP"}]}
            }))
            .unwrap();
            let class: GatewayClass = serde_json::from_value(json!({
                "metadata":{"name":"empty"},"spec":{"controllerName":"edgion.io/gateway-controller"}
            }))
            .unwrap();
            publisher.replace_gateway_classes(HashMap::from([("empty".into(), Arc::new(class))]));
            publisher.replace_gateways(HashMap::from([(
                format!("{NAMESPACE}/empty"),
                GatewayInput::from_gateway(&gateway),
            )]));
        }
        let mut ctx = EdgionHttpContext::new();
        ctx.egc = fixture.egc.clone();
        ctx.capture_listener_publication(publisher.snapshot().http_listener(&fixture.proxy.listener_key));
        let (mut session, _client) = session("application/json").await;
        if malformed_host {
            session.req_header_mut().remove_header("host");
        }
        assert!(run_entry(&fixture, &mut session, &mut ctx).await);
        assert_eq!(
            session.response_written().unwrap().status.as_u16(),
            if malformed_host { 400 } else { 404 }
        );
        assert!(ctx.route_unit.is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unavailable_owner_keeps_unique_early_and_shared_matched_503() {
    for shared in [false, true] {
        let fixture = Fixture::new(false, true, false, false, false);
        let publisher = PluginPublisher::new();
        let mut sources = HashMap::new();
        for name in if shared { vec!["one", "two"] } else { vec!["one"] } {
            let gateway: Gateway = crate::test_current_status::from_value(json!({
                "metadata":{"namespace":NAMESPACE,"name":name},
                "spec":{"gatewayClassName":"missing","listeners":[{"name":"web","port":18479,"protocol":"HTTP"}]}
            }))
            .unwrap();
            sources.insert(format!("{NAMESPACE}/{name}"), GatewayInput::from_gateway(&gateway));
        }
        publisher.replace_gateways(sources);
        let mut ctx = EdgionHttpContext::new();
        ctx.capture_listener_publication(publisher.snapshot().http_listener(&fixture.proxy.listener_key));
        let (mut session, _client) = session("application/json").await;
        let early =
            super::super::pg_early_request_filter::initialize_request_ids(&fixture.proxy, &mut session, &mut ctx).await;
        if shared {
            assert!(early.is_ok());
            assert!(ctx.plugin_scope().is_none());
            ctx.gateway_info = Some(
                ctx.listener_publication()
                    .unwrap()
                    .index
                    .get(&format!("{NAMESPACE}/one"), "web")
                    .unwrap()
                    .info
                    .clone(),
            );
            super::super::pg_early_request_filter::capture_matched_scope(&mut ctx).unwrap();
            let error = super::super::pg_early_request_filter::initialize_gateway_request_ids(&mut session, &mut ctx)
                .await
                .unwrap_err();
            assert_eq!(error.etype(), &pingora_core::ErrorType::HTTPStatus(503));
        } else {
            assert_eq!(early.unwrap_err().etype(), &pingora_core::ErrorType::HTTPStatus(503));
        }
    }
}
