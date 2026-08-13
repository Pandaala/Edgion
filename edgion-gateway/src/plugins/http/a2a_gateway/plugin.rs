use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use edgion_dsl::json_path::DefiniteJsonPath;
use edgion_dsl::value::Value as DslValue;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{A2aGatewayConfig, BodyRequirement, A2A_AGENT_CARD_MAX_BYTES};
use pingora_http::ResponseHeader;
use serde_json::Value;
use url::Url;

use crate::ai::{global_edgion_backend_store, RuntimeEdgionBackend};
use crate::plugins::http::common::{json_request_headers_are_supported, CappedWriter};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::log::sensitive;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_body::{JsonDuplicatePolicy, JsonScanValue, JsonSelectPlan, JsonSelectScanner};
use crate::routes::http::match_unit::CompiledBackendTarget;

const CARD_PATH: &str = "/.well-known/agent-card.json";
const METHOD_MAX_BYTES: usize = 64;

pub struct A2aGateway {
    card: Value,
    external_origin: Option<Url>,
    fail_open: bool,
    method_plan: Arc<JsonSelectPlan>,
}

impl A2aGateway {
    fn method_scan_unavailable(log: &mut PluginLog) -> PluginRunningResult {
        log.push_err("method-unavailable");
        PluginRunningResult::GoodNext
    }

    pub fn create(config: &A2aGatewayConfig) -> Option<Box<dyn RequestFilter>> {
        config.validated_card_bytes().ok()?;
        let external_origin = config.validated_external_origin().ok()?;
        let method_path = "$.method".parse::<DefiniteJsonPath>().ok()?;
        Some(Box::new(Self {
            card: config.card.clone(),
            external_origin,
            fail_open: config.fail_open,
            method_plan: Arc::new(JsonSelectPlan::with_duplicate_policy(
                [("method".to_string(), method_path)],
                JsonDuplicatePolicy::RejectSelected,
            )),
        }))
    }

    fn is_local_card_rule(session: &dyn PluginSession) -> bool {
        let Some(unit) = session.ctx().route_unit.as_ref() else {
            return false;
        };
        unit.backends.is_empty()
            && unit.matched_info.m.method.as_deref() == Some("GET")
            && unit.matched_info.m.path.as_ref().is_some_and(|path| {
                path.match_type.as_deref() == Some("Exact") && path.value.as_deref() == Some(CARD_PATH)
            })
            && session.get_method() == "GET"
            && session.get_path() == CARD_PATH
    }

    fn is_rpc_proxy_rule(session: &mut dyn PluginSession) -> bool {
        if session.get_method() != "POST" {
            return false;
        }
        let Some(unit) = session.ctx().route_unit.as_ref() else {
            return false;
        };
        let positive = unit
            .backends
            .iter()
            .filter(|backend| backend.schema.weight.unwrap_or(1) > 0)
            .cloned()
            .collect::<Vec<_>>();
        if positive.is_empty() {
            return false;
        }
        let snapshot = session
            .ctx()
            .edgion_backend_snapshot
            .clone()
            .unwrap_or_else(|| global_edgion_backend_store().edgion_backend_snapshot());
        for backend in &positive {
            if backend.schema.ref_denied.is_some() {
                continue;
            }
            let CompiledBackendTarget::EdgionBackend { namespace, name } = &backend.target else {
                return false;
            };
            let key = format!("{namespace}/{name}");
            if snapshot
                .get_by_key(&key)
                .is_some_and(|runtime| !matches!(runtime.as_ref(), RuntimeEdgionBackend::A2a(_)))
            {
                return false;
            }
        }
        session.pin_edgion_backend_snapshot(snapshot);
        true
    }

    fn public_origin(&self, session: &dyn PluginSession) -> Option<Url> {
        if let Some(origin) = &self.external_origin {
            return Some(origin.clone());
        }
        let tls = session.downstream_is_tls()?;
        let hostname = session.ctx().request_info.hostname.as_str();
        if hostname.is_empty() {
            return None;
        }
        let mut origin = Url::parse(if tls { "https://localhost/" } else { "http://localhost/" }).ok()?;
        if let Ok(address) = hostname.parse::<std::net::Ipv6Addr>() {
            origin.set_host(Some(&format!("[{address}]"))).ok()?;
        } else {
            origin.set_host(Some(hostname)).ok()?;
        }
        let port = session.ctx().request_info.listener_port;
        let default_port = if tls { 443 } else { 80 };
        origin.set_port((port != default_port).then_some(port)).ok()?;
        Some(origin)
    }

    fn rewritten_card(&self, origin: &Url) -> Option<Bytes> {
        let mut card = self.card.clone();
        let interfaces = card.get_mut("supportedInterfaces")?.as_array_mut()?;
        interfaces.retain(|interface| {
            interface.get("protocolBinding").and_then(Value::as_str) == Some("JSONRPC")
                && interface.get("protocolVersion").and_then(Value::as_str) == Some("1.0")
        });
        if interfaces.is_empty() {
            return None;
        }
        let public_host = match origin.host()? {
            url::Host::Domain(host) => host.to_string(),
            url::Host::Ipv4(address) => address.to_string(),
            url::Host::Ipv6(address) => format!("[{address}]"),
        };
        for interface in interfaces {
            let object = interface.as_object_mut()?;
            let mut endpoint = Url::parse(object.get("url")?.as_str()?).ok()?;
            endpoint.set_scheme(origin.scheme()).ok()?;
            endpoint.set_host(Some(&public_host)).ok()?;
            endpoint.set_port(origin.port()).ok()?;
            object.insert("url".to_string(), Value::String(endpoint.into()));
        }
        let mut writer = CappedWriter::new(A2A_AGENT_CARD_MAX_BYTES);
        serde_json::to_writer(&mut writer, &card).ok()?;
        Some(Bytes::from(writer.into_inner()))
    }

    async fn serve_card(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let Some(origin) = self.public_origin(session) else {
            log.push_err("card-origin");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };
        let Some(body) = self.rewritten_card(&origin) else {
            log.push_err("card-rewrite");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };
        let Ok(mut response) = ResponseHeader::build(200, None) else {
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };
        let _ = response.insert_header("content-type", "application/json");
        let _ = response.insert_header("content-length", body.len().to_string());
        let _ = response.insert_header("cache-control", "no-store");
        if session.write_response_header(Box::new(response), false).await.is_err() {
            log.push_err("card-header-write");
            return PluginRunningResult::ErrTerminateRequest;
        }
        if session.write_response_body(Some(body), true).await.is_err() {
            log.push_err("card-write");
        } else {
            log.push("card-served");
        }
        PluginRunningResult::ErrTerminateRequest
    }

    fn protocol_headers_supported(session: &dyn PluginSession) -> bool {
        let versions = session.request_header_values("a2a-version");
        if !matches!(versions.as_deref(), Some([version]) if version == "1.0") {
            return false;
        }
        json_request_headers_are_supported(
            session.request_header_values("content-type").as_deref(),
            session.request_header_values("content-encoding").as_deref(),
        )
    }

    async fn observe_method(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let cap = session.ctx().request_body_config.max_body_size_bytes;
        if let Err(error) = session.ensure_request_body(Some(cap)).await {
            return body_read_failure_result(session, &BodyRequirement::fail_close(), log, &error);
        }
        let scanner = match JsonSelectScanner::new(Arc::clone(&self.method_plan), METHOD_MAX_BYTES).await {
            Ok(scanner) => scanner,
            Err(_) => return Self::method_scan_unavailable(log),
        };
        let result = session.ctx().request_body.scan_raw(scanner, cap).await;
        let value = match result {
            Ok(scan) => scan.value.into_iter().next(),
            Err(_) => return Self::method_scan_unavailable(log),
        };
        let Some(value) = value else {
            return Self::method_scan_unavailable(log);
        };
        match value {
            JsonScanValue::Value(DslValue::Str(method))
                if !method.is_empty() && method.len() <= METHOD_MAX_BYTES && !method.chars().any(char::is_control) =>
            {
                log.push_fmt("method {}", &[sensitive(&method)]);
            }
            _ => {
                log.push("method-unknown");
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl RequestFilter for A2aGateway {
    fn name(&self) -> &str {
        "A2aGateway"
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let local_card = Self::is_local_card_rule(session);
        let rpc_proxy = !local_card && Self::is_rpc_proxy_rule(session);
        if !local_card && !rpc_proxy {
            return PluginRunningResult::GoodNext;
        }
        if !session.try_activate_a2a_gateway() {
            log.push_err("duplicate-active");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        }
        if local_card {
            return self.serve_card(session, log).await;
        }
        if !Self::protocol_headers_supported(session) {
            log.push_err("protocol-headers");
            return if self.fail_open {
                PluginRunningResult::GoodNext
            } else {
                PluginRunningResult::ErrResponse {
                    status: 400,
                    body: None,
                }
            };
        }
        self.observe_method(session, log).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{MatchInfo, Rsc};
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::request_body::RawBodyScanner;
    use crate::routes::http::match_unit::{CompiledBackend, HttpRouteRuleUnit, MatchSpecificity};
    use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
    use serde_json::json;

    fn config() -> A2aGatewayConfig {
        A2aGatewayConfig {
            card: json!({
                "name": "agent",
                "description": "Test agent",
                "version": "1.0.0",
                "capabilities": {},
                "defaultInputModes": ["application/json"],
                "defaultOutputModes": ["application/json"],
                "skills": [{
                    "id": "echo",
                    "name": "Echo",
                    "description": "Echo input",
                    "tags": ["test"]
                }],
                "unknownCard": {"kept": true},
                "supportedInterfaces": [
                    {
                        "protocolBinding": "GRPC",
                        "protocolVersion": "1.0",
                        "url": "https://private.example/grpc"
                    },
                    {
                        "before": 1,
                        "protocolBinding": "JSONRPC",
                        "protocolVersion": "1.0",
                        "url": "https://private.example/rpc?tenant=blue",
                        "after": 2
                    }
                ]
            }),
            external_url: Some("https://public.example:8443".to_string()),
            fail_open: false,
        }
    }

    fn rpc_ctx(backend_ref: HTTPBackendRef) -> crate::EdgionHttpContext {
        let compiled = CompiledBackend::compile(
            backend_ref.clone(),
            "test-ns",
            0,
            Arc::new(crate::plugins::PluginRuntime::new()),
        );
        let mut ctx = crate::EdgionHttpContext::new();
        ctx.route_unit = Some(Arc::new(HttpRouteRuleUnit {
            resource_key: "test-ns/test-route".to_string(),
            matched_info: MatchInfo {
                rsc: Rsc::new("HTTPRoute", "test-ns", "test-route"),
                rule_id: 0,
                match_id: 0,
                sv: 0,
                m: HTTPRouteMatch {
                    path: None,
                    headers: None,
                    query_params: None,
                    method: Some("POST".to_string()),
                },
                labels_meta: None,
                annotations_meta: None,
            },
            rule: Arc::new(HTTPRouteRule {
                name: None,
                resolved_terminal_route_uid: None,
                resolved_terminal_rule_identity: None,
                matches: None,
                filters: None,
                backend_refs: Some(vec![backend_ref]),
                timeouts: None,
                retry: None,
                session_persistence: None,
                parsed_timeouts: None,
                parsed_retry: None,
                parsed_forward_raw_path: None,
                parsed_allow_non_idempotent_retry: None,
                parsed_mirror_tuning: None,
            }),
            route: None,
            path_regex: None,
            parent_refs: None,
            compiled_header_regexes: vec![],
            compiled_query_regexes: vec![],
            has_query_matchers: false,
            specificity: MatchSpecificity {
                has_method: true,
                header_count: 0,
                query_count: 0,
                creation_ts_ms: 0,
                route_key: "test-ns/test-route".to_string(),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(crate::plugins::PluginRuntime::new()),
            backends: Arc::new(vec![compiled]),
            backend_finder: Arc::new(crate::lb::BackendSelector::new()),
        }));
        ctx
    }

    fn backend_ref(group: Option<&str>, kind: Option<&str>) -> HTTPBackendRef {
        HTTPBackendRef {
            name: "agent".to_string(),
            namespace: None,
            port: None,
            weight: Some(1),
            group: group.map(str::to_string),
            kind: kind.map(str::to_string),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        }
    }

    fn local_card_ctx(with_backend: bool) -> crate::EdgionHttpContext {
        let mut ctx = rpc_ctx(backend_ref(Some("edgion.io"), Some("EdgionBackend")));
        let unit = Arc::get_mut(ctx.route_unit.as_mut().expect("route unit")).expect("fixture owns route unit");
        unit.matched_info.m.method = Some("GET".to_string());
        unit.matched_info.m.path = Some(edgion_resources::HTTPPathMatch {
            match_type: Some("Exact".to_string()),
            value: Some(CARD_PATH.to_string()),
        });
        if !with_backend {
            unit.backends = Arc::new(Vec::new());
        }
        ctx
    }

    #[test]
    fn local_card_rule_requires_exact_backendless_get_shape_and_live_request() {
        let mut valid = MockPluginSession::new();
        valid.expect_ctx().return_const(local_card_ctx(false));
        valid.expect_get_method().return_const("GET".to_string());
        valid.expect_get_path().return_const(CARD_PATH.to_string());
        assert!(A2aGateway::is_local_card_rule(&valid));

        let mut wrong_method = MockPluginSession::new();
        wrong_method.expect_ctx().return_const(local_card_ctx(false));
        wrong_method.expect_get_method().return_const("POST".to_string());
        assert!(!A2aGateway::is_local_card_rule(&wrong_method));

        let mut wrong_path = MockPluginSession::new();
        wrong_path.expect_ctx().return_const(local_card_ctx(false));
        wrong_path.expect_get_method().return_const("GET".to_string());
        wrong_path.expect_get_path().return_const("/other".to_string());
        assert!(!A2aGateway::is_local_card_rule(&wrong_path));

        let mut backend = MockPluginSession::new();
        backend.expect_ctx().return_const(local_card_ctx(true));
        backend.expect_get_method().times(0);
        backend.expect_get_path().times(0);
        assert!(!A2aGateway::is_local_card_rule(&backend));
    }

    #[tokio::test]
    async fn protocol_gate_failure_obeys_fail_open_without_capturing_body() {
        for (fail_open, expected) in [
            (
                false,
                PluginRunningResult::ErrResponse {
                    status: 400,
                    body: None,
                },
            ),
            (true, PluginRunningResult::GoodNext),
        ] {
            let mut cfg = config();
            cfg.fail_open = fail_open;
            let external_origin = cfg.validated_external_origin().unwrap();
            let mut session = MockPluginSession::new();
            session
                .expect_ctx()
                .return_const(rpc_ctx(backend_ref(Some("edgion.io"), Some("EdgionBackend"))));
            session.expect_get_method().return_const("POST".to_string());
            session.expect_pin_edgion_backend_snapshot().times(1).return_const(());
            session.expect_try_activate_a2a_gateway().times(1).return_const(true);
            session
                .expect_request_header_values()
                .with(mockall::predicate::eq("a2a-version"))
                .times(1)
                .return_const(None);
            session.expect_ensure_request_body().times(0);
            let plugin = A2aGateway {
                card: cfg.card,
                external_origin,
                fail_open,
                method_plan: Arc::new(JsonSelectPlan::new([(
                    "method".to_string(),
                    "$.method".parse().unwrap(),
                )])),
            };
            assert_eq!(
                plugin
                    .run_request(&mut session, &mut PluginLog::new("A2aGateway"))
                    .await,
                expected
            );
        }
    }

    #[tokio::test]
    async fn capture_storage_failure_uses_canonical_fail_close_mapping() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        use edgion_resources::EdgionStatus;

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(crate::EdgionHttpContext::new());
        session
            .expect_ensure_request_body()
            .times(1)
            .return_once(|_| Err(BodyReadError::StorageFailure));
        session
            .expect_add_error()
            .with(mockall::predicate::eq(EdgionStatus::BodyCaptureStorageFailed))
            .times(1)
            .return_const(());
        let cfg = config();
        let plugin = A2aGateway {
            card: cfg.card,
            external_origin: None,
            fail_open: true,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };

        assert_eq!(
            plugin
                .observe_method(&mut session, &mut PluginLog::new("A2aGateway"))
                .await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        );
    }

    #[test]
    fn rpc_rule_pins_only_after_syntactic_qualification() {
        let mut qualifying = MockPluginSession::new();
        qualifying.expect_get_method().return_const("POST".to_string());
        qualifying
            .expect_ctx()
            .return_const(rpc_ctx(backend_ref(Some("edgion.io"), Some("EdgionBackend"))));
        qualifying
            .expect_pin_edgion_backend_snapshot()
            .times(1)
            .return_const(());
        assert!(A2aGateway::is_rpc_proxy_rule(&mut qualifying));

        let mut service = MockPluginSession::new();
        service.expect_get_method().return_const("POST".to_string());
        service.expect_ctx().return_const(rpc_ctx(backend_ref(None, None)));
        service.expect_pin_edgion_backend_snapshot().times(0);
        assert!(!A2aGateway::is_rpc_proxy_rule(&mut service));
    }

    #[test]
    fn rewrite_filters_interfaces_and_preserves_path_query_and_unknown_fields() {
        let mut card = config().card;
        card["supportedInterfaces"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "url": "grpc.example:443",
                "protocolBinding": "GRPC",
                "protocolVersion": "1.0"
            }));
        let plugin = A2aGateway {
            card,
            external_origin: Url::parse("https://public.example:8443").ok(),
            fail_open: false,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };
        let body = plugin.rewritten_card(plugin.external_origin.as_ref().unwrap()).unwrap();
        let card: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(card["unknownCard"]["kept"], true);
        let interfaces = card["supportedInterfaces"].as_array().unwrap();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0]["url"], "https://public.example:8443/rpc?tenant=blue");
        assert_eq!(interfaces[0]["before"], 1);
        assert_eq!(interfaces[0]["after"], 2);
    }

    #[test]
    fn rewrite_preserves_ipv6_public_authority() {
        let plugin = A2aGateway {
            card: config().card,
            external_origin: Url::parse("https://[2001:db8::1]:8443").ok(),
            fail_open: false,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };

        let body = plugin.rewritten_card(plugin.external_origin.as_ref().unwrap()).unwrap();
        let card: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            card["supportedInterfaces"][0]["url"],
            "https://[2001:db8::1]:8443/rpc?tenant=blue"
        );
    }

    #[tokio::test]
    async fn shared_selector_extracts_only_bounded_top_level_method_across_chunks() {
        use crate::request_body::{BodyStorage, RawBodyScanner, ScanControl};

        let plan = Arc::new(JsonSelectPlan::new([(
            "method".to_string(),
            "$.method".parse().unwrap(),
        )]));
        let mut scanner = JsonSelectScanner::new(plan, METHOD_MAX_BYTES).await.unwrap();
        assert_eq!(
            scanner.push(br#"{"params":{"method":"secret"},"met"#).unwrap(),
            ScanControl::Continue
        );
        assert_eq!(
            scanner.push(br#"hod":"tasks/get","id":7}"#).unwrap(),
            ScanControl::Continue
        );
        let values = scanner.finish().unwrap();
        assert!(matches!(
            values.as_slice(),
            [JsonScanValue::Value(DslValue::Str(method))] if method.as_ref() == "tasks/get"
        ));

        // The same shared scanner also runs over the canonical storage view.
        let (mut storage, view) = BodyStorage::new(1, 1024);
        use pingora_core::protocols::http::RequestBodyBuffer;
        storage
            .write(&Bytes::from_static(br#"{"method":"tasks/send"}"#))
            .await
            .unwrap();
        storage.finish().await.unwrap();
        let scan = view
            .scan_raw(
                JsonSelectScanner::new(
                    Arc::new(JsonSelectPlan::new([(
                        "method".to_string(),
                        "$.method".parse().unwrap(),
                    )])),
                    METHOD_MAX_BYTES,
                )
                .await
                .unwrap(),
                1024,
            )
            .await
            .unwrap();
        assert!(matches!(
            scan.value.as_slice(),
            [JsonScanValue::Value(DslValue::Str(method))] if method.as_ref() == "tasks/send"
        ));
    }

    #[tokio::test]
    async fn a2a_selector_marks_duplicate_method_unknown_without_changing_dsl_default() {
        let path = "$.method".parse().unwrap();
        let rejecting = Arc::new(JsonSelectPlan::with_duplicate_policy(
            [("method".to_string(), path)],
            JsonDuplicatePolicy::RejectSelected,
        ));
        let mut scanner = JsonSelectScanner::new(rejecting, METHOD_MAX_BYTES).await.unwrap();
        scanner.push(br#"{"method":"first","method":"second"}"#).unwrap();
        assert!(matches!(scanner.finish().unwrap().as_slice(), [JsonScanValue::Missing]));

        let default = Arc::new(JsonSelectPlan::new([(
            "method".to_string(),
            "$.method".parse().unwrap(),
        )]));
        let mut scanner = JsonSelectScanner::new(default, METHOD_MAX_BYTES).await.unwrap();
        scanner.push(br#"{"method":"first","method":"second"}"#).unwrap();
        assert!(matches!(
            scanner.finish().unwrap().as_slice(),
            [JsonScanValue::Value(DslValue::Str(method))] if method.as_ref() == "second"
        ));
    }

    #[test]
    fn rewritten_output_is_bounded_before_response_writing() {
        let interface = json!({
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0",
            "url": "https://p/r"
        });
        let mut cfg = config();
        cfg.card["supportedInterfaces"] = Value::Array(vec![interface; 12_000]);
        let plugin = A2aGateway {
            card: cfg.card,
            external_origin: Some(Url::parse(&format!("https://{}.example", "a".repeat(60))).unwrap()),
            fail_open: false,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };
        assert!(plugin
            .rewritten_card(plugin.external_origin.as_ref().unwrap())
            .is_none());
    }

    #[test]
    fn rewritten_output_limit_is_inclusive() {
        let origin = Url::parse("https://public.example").unwrap();
        let base_card = config().card;
        let make_plugin = |padding: usize| {
            let mut card = base_card.clone();
            card["supportedInterfaces"][1]["padding"] = Value::String("x".repeat(padding));
            A2aGateway {
                card,
                external_origin: Some(origin.clone()),
                fail_open: false,
                method_plan: Arc::new(JsonSelectPlan::new([(
                    "method".to_string(),
                    "$.method".parse().unwrap(),
                )])),
            }
        };
        let mut low = 0usize;
        let mut high = A2A_AGENT_CARD_MAX_BYTES;
        while low < high {
            let midpoint = low + (high - low).div_ceil(2);
            if make_plugin(midpoint).rewritten_card(&origin).is_some() {
                low = midpoint;
            } else {
                high = midpoint - 1;
            }
        }
        let exact = make_plugin(low).rewritten_card(&origin).unwrap();
        assert_eq!(exact.len(), A2A_AGENT_CARD_MAX_BYTES);
        assert!(make_plugin(low + 1).rewritten_card(&origin).is_none());
    }

    #[tokio::test]
    async fn header_write_failure_terminates_without_requesting_a_second_response() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use crate::EdgionHttpContext;

        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.hostname = crate::ctx::NormalizedHostname::new("agent.example");
        ctx.request_info.listener_port = 443;
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session.expect_downstream_is_tls().return_const(Some(true));
        session
            .expect_write_response_header()
            .times(1)
            .returning(|_, _| Err(Box::new(std::io::Error::other("closed"))));
        session.expect_write_response_body().times(0);

        let concrete = config();
        let external_origin = concrete.validated_external_origin().unwrap();
        let plugin = A2aGateway {
            card: concrete.card,
            external_origin,
            fail_open: false,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };
        let mut log = PluginLog::new("A2aGateway");
        assert_eq!(
            plugin.serve_card(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }

    #[test]
    fn create_rejects_invalid_config_and_retains_valid_snapshot() {
        assert!(A2aGateway::create(&config()).is_some());
        let mut invalid = config();
        invalid.card["url"] = json!("https://legacy.example/rpc");
        assert!(A2aGateway::create(&invalid).is_none());
    }

    #[test]
    fn post_capture_scan_failure_is_static_failure_open() {
        let mut log = PluginLog::new("A2aGateway");
        assert_eq!(
            A2aGateway::method_scan_unavailable(&mut log),
            PluginRunningResult::GoodNext
        );
        let encoded = serde_json::to_string(&log).unwrap();
        assert!(encoded.contains("method-unavailable"));
        assert!(!encoded.contains("Incomplete"));
    }

    fn header_session(
        version: Option<Vec<String>>,
        content_type: Option<Vec<String>>,
        encoding: Option<Vec<String>>,
    ) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session
            .expect_request_header_values()
            .returning(move |name| match name {
                "a2a-version" => version.clone(),
                "content-type" => content_type.clone(),
                "content-encoding" => encoding.clone(),
                _ => unreachable!("unexpected header"),
            });
        session
    }

    #[test]
    fn protocol_gate_requires_one_exact_version_and_supported_json_headers() {
        let valid = || Some(vec!["1.0".to_string()]);
        let json = || Some(vec!["application/json; charset=utf-8".to_string()]);
        let identity = || Some(vec!["identity".to_string()]);
        assert!(A2aGateway::protocol_headers_supported(&header_session(
            valid(),
            json(),
            identity()
        )));

        for (version, content_type, encoding) in [
            (None, json(), identity()),
            (Some(vec!["1.0".into(), "1.0".into()]), json(), identity()),
            (
                Some(vec![String::from_utf8_lossy(&[0xff]).into_owned()]),
                json(),
                identity(),
            ),
            (Some(vec!["0.3".into()]), json(), identity()),
            (valid(), Some(vec!["text/plain".into()]), identity()),
            (valid(), json(), Some(vec!["gzip".into()])),
        ] {
            assert!(!A2aGateway::protocol_headers_supported(&header_session(
                version,
                content_type,
                encoding
            )));
        }
    }

    #[test]
    fn public_origin_uses_only_normalized_route_metadata_and_tls_state() {
        let mut cfg = config();
        cfg.external_url = None;

        for (tls, host, port, expected) in [
            (Some(false), "agent.example", 80, Some("http://agent.example/")),
            (Some(true), "agent.example", 443, Some("https://agent.example/")),
            (Some(true), "agent.example", 8443, Some("https://agent.example:8443/")),
            (Some(true), "2001:db8::1", 443, Some("https://[2001:db8::1]/")),
            (Some(true), "[2001:db8::1]", 443, Some("https://[2001:db8::1]/")),
            (Some(true), "[2001:db8::1]", 8443, Some("https://[2001:db8::1]:8443/")),
            (None, "agent.example", 443, None),
        ] {
            let mut ctx = crate::EdgionHttpContext::new();
            ctx.request_info.hostname = crate::ctx::NormalizedHostname::new(host);
            ctx.request_info.listener_port = port;
            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx);
            session.expect_downstream_is_tls().return_const(tls);
            let concrete = A2aGateway {
                card: cfg.card.clone(),
                external_origin: None,
                fail_open: false,
                method_plan: Arc::new(JsonSelectPlan::new([(
                    "method".to_string(),
                    "$.method".parse().unwrap(),
                )])),
            };
            assert_eq!(concrete.public_origin(&session).as_ref().map(Url::as_str), expected);
        }
    }

    #[tokio::test]
    async fn method_selector_classifies_invalid_shapes_without_exposing_values() {
        for body in [
            br#"{}"#.as_slice(),
            br#"{"method":""}"#,
            br#"{"method":"bad\u0000method"}"#,
            br#"{"method":7}"#,
            br#"{"method":"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789xxx"}"#,
            br#"{"method":"unterminated}"#,
            br#"[{"method":"tasks/get"}]"#,
        ] {
            let mut scanner = JsonSelectScanner::new(
                Arc::new(JsonSelectPlan::with_duplicate_policy(
                    [("method".to_string(), "$.method".parse().unwrap())],
                    JsonDuplicatePolicy::RejectSelected,
                )),
                METHOD_MAX_BYTES,
            )
            .await
            .unwrap();
            let outcome = scanner.push(body).and_then(|_| scanner.finish());
            if let Ok(values) = outcome {
                assert!(!matches!(
                    values.as_slice(),
                    [JsonScanValue::Value(DslValue::Str(method))]
                        if !method.is_empty()
                            && method.len() <= METHOD_MAX_BYTES
                            && !method.chars().any(char::is_control)
                ));
            }
        }
    }

    #[tokio::test]
    async fn successful_card_response_has_exact_safe_headers_and_terminates() {
        let mut session = MockPluginSession::new();
        let mut ctx = crate::EdgionHttpContext::new();
        ctx.request_info.hostname = crate::ctx::NormalizedHostname::new("ignored.example");
        ctx.request_info.listener_port = 443;
        session.expect_ctx().return_const(ctx);
        session.expect_downstream_is_tls().times(0);
        session
            .expect_write_response_header()
            .times(1)
            .withf(|header, eos| {
                !*eos
                    && header.status.as_u16() == 200
                    && header.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json")
                    && header.headers.get("cache-control").and_then(|v| v.to_str().ok()) == Some("no-store")
                    && header.headers.get("content-length").is_some()
            })
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .times(1)
            .withf(|body, eos| {
                *eos && body.as_ref().is_some_and(|bytes| {
                    serde_json::from_slice::<Value>(bytes).ok().is_some_and(|card| {
                        card["supportedInterfaces"][0]["url"] == "https://public.example:8443/rpc?tenant=blue"
                    })
                })
            })
            .returning(|_, _| Ok(()));
        let cfg = config();
        let external_origin = cfg.validated_external_origin().unwrap();
        let plugin = A2aGateway {
            card: cfg.card,
            external_origin,
            fail_open: false,
            method_plan: Arc::new(JsonSelectPlan::new([(
                "method".to_string(),
                "$.method".parse().unwrap(),
            )])),
        };
        assert_eq!(
            plugin.serve_card(&mut session, &mut PluginLog::new("A2aGateway")).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }
}
