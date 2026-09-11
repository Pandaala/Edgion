use async_trait::async_trait;
use bytes::Bytes;
use std::net::{IpAddr, Ipv6Addr};

use crate::backends::{
    get_global_backend_tls_policy_store, get_service_discovery_coordinator, validate_endpoint_backend_addr,
    validate_endpoint_in_route,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::match_unit::CompiledBackend;
use crate::{DirectEndpointPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DirectEndpointConfig, UpstreamFallback};

const MISSING_TARGET_ENDPOINT_BODY: Bytes = Bytes::from_static(b"Missing target endpoint");
const INVALID_TARGET_ENDPOINT_BODY: Bytes = Bytes::from_static(b"Invalid target endpoint");
const TARGET_ENDPOINT_NOT_ALLOWED_BODY: Bytes = Bytes::from_static(b"Target endpoint not allowed");

pub struct DirectEndpoint {
    config: DirectEndpointConfig,
}

impl DirectEndpoint {
    pub fn new(config: &DirectEndpointConfig) -> Self {
        let mut validated = config.clone();
        validated.validate();
        Self { config: validated }
    }

    /// Resolve the override against one immutable discovery generation.
    fn resolve(
        &self,
        raw_value: String,
        backends: &[CompiledBackend],
        allow_loopback_upstream: bool,
    ) -> Result<DirectEndpointPreset, ResolutionError> {
        // 1. Apply regex extraction if configured
        let endpoint_str = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => return Err(ResolutionError::Missing),
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 2. Parse IP (support "ip" or "ip:port" format)
        let (target_ip, extracted_port) = parse_endpoint(&endpoint_str).map_err(|_| ResolutionError::Invalid)?;

        // 3. Security: validate against route's backend_ref endpoints
        let discovery = get_service_discovery_coordinator().load_generation();
        let policies = get_global_backend_tls_policy_store().snapshot();
        let endpoint = validate_endpoint_in_route(
            target_ip,
            extracted_port.or(self.config.port),
            backends,
            &discovery,
            &policies,
            self.config.inherit_tls,
        )
        .map_err(|_| ResolutionError::NotAllowed)?;
        let addr = endpoint.addr;
        if validate_endpoint_backend_addr(
            &pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
            allow_loopback_upstream,
        )
        .is_err()
        {
            return Err(ResolutionError::NotAllowed);
        }

        Ok(DirectEndpointPreset {
            addr,
            tls_policy: endpoint.tls_policy,
            backend_ref_idx: endpoint.backend_ref_idx,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolutionError {
    Missing,
    Invalid,
    NotAllowed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InvalidEndpoint;

#[async_trait]
impl RequestFilter for DirectEndpoint {
    fn name(&self) -> &str {
        "DirectEndpoint"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw endpoint value via KeyGet
        let raw_value = match session.key_get(&self.config.from) {
            Some(v) if !v.is_empty() => v,
            _ => match self.config.on_missing {
                UpstreamFallback::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-missing");
                    log.push("no value fallback");
                    return PluginRunningResult::GoodNext;
                }
                UpstreamFallback::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-missing");
                    log.push("no value 400");
                    return PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some(MISSING_TARGET_ENDPOINT_BODY),
                    };
                }
            },
        };

        // 2. Get route context
        let ctx = session.ctx();
        let backends = match ctx.route_unit.as_ref() {
            Some(unit) if !unit.backends.is_empty() => unit.backends.as_slice(),
            _ => {
                log.push("no route refs");
                return PluginRunningResult::GoodNext;
            }
        };

        // 3. Resolve endpoint info
        match self.resolve(raw_value, backends, ctx.allow_loopback_upstream) {
            Ok(info) => {
                let addr = info.addr;
                match session.set_upstream_jump(UpstreamJumpTarget::Direct(info)) {
                    SetJumpOutcome::Set => {
                        let addr_str = addr.to_string();
                        log.push_fmt("endpoint {}", la![sensitive(&addr_str)]);
                        session.record_outcome(DyeOutcome::Success, &addr_str);
                    }
                    SetJumpOutcome::Rejected => {
                        log.push("jump locked, rejected");
                        return PluginRunningResult::GoodNext;
                    }
                }

                PluginRunningResult::GoodNext
            }
            Err(ResolutionError::Missing) => {
                log.push("endpoint miss");
                match self.config.on_missing {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some(MISSING_TARGET_ENDPOINT_BODY),
                        }
                    }
                }
            }
            Err(error @ (ResolutionError::Invalid | ResolutionError::NotAllowed)) => {
                log.push("endpoint invalid");
                match self.config.on_invalid {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-invalid");
                        let (status, body) = match error {
                            ResolutionError::Invalid => (400, INVALID_TARGET_ENDPOINT_BODY),
                            ResolutionError::NotAllowed => (403, TARGET_ENDPOINT_NOT_ALLOWED_BODY),
                            ResolutionError::Missing => unreachable!("handled by the preceding match arm"),
                        };
                        PluginRunningResult::ErrResponse {
                            status,
                            body: Some(body),
                        }
                    }
                }
            }
        }
    }
}

/// Parse endpoint string into (IpAddr, optional port)
fn parse_endpoint(s: &str) -> Result<(IpAddr, Option<u16>), InvalidEndpoint> {
    if let Some(bracketed) = s.strip_prefix('[') {
        let (ip, suffix) = bracketed.split_once(']').ok_or(InvalidEndpoint)?;
        let ip = ip.parse::<Ipv6Addr>().map_err(|_| InvalidEndpoint)?;
        let port = match suffix {
            "" => None,
            _ => {
                let port = suffix
                    .strip_prefix(':')
                    .filter(|port| !port.is_empty())
                    .ok_or(InvalidEndpoint)?;
                Some(port.parse::<u16>().map_err(|_| InvalidEndpoint)?)
            }
        };
        return Ok((IpAddr::V6(ip), port));
    }

    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok((ip, None));
    }
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return Ok((addr.ip(), Some(addr.port())));
    }

    Err(InvalidEndpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::get_roundrobin_store;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
    use crate::{EdgionHttpContext, MatchInfo, Rsc};
    use edgion_resources::common::KeyGet;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use edgion_resources::resources::edgion_plugins::EndpointExtract;
    use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    /// Publish a Service-port mapping and its single-address EndpointSlice.
    fn insert_test_endpoint_slice(ns: &str, service_name: &str, slice_name: &str, ip: &str, port: i32) {
        use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
        use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
        get_service_discovery_coordinator().update_services(
            HashMap::from([(
                format!("{ns}/{service_name}"),
                Arc::new(Service {
                    metadata: ObjectMeta {
                        name: Some(service_name.to_owned()),
                        namespace: Some(ns.to_owned()),
                        ..Default::default()
                    },
                    spec: Some(ServiceSpec {
                        ports: Some(vec![ServicePort {
                            port: 443,
                            target_port: Some(IntOrString::Int(port)),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )]),
            HashMap::new(),
            Default::default(),
        );
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), service_name.to_string());
        let slice = EndpointSlice {
            metadata: ObjectMeta {
                name: Some(slice_name.to_string()),
                namespace: Some(ns.to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec![ip.to_string()],
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                port: Some(port),
                ..Default::default()
            }]),
        };
        let ep_key = format!("{}/{}", ns, slice_name);
        let mut add = HashMap::new();
        add.insert(ep_key, slice);
        get_roundrobin_store().update_data_only(add, HashMap::new(), &Default::default());
    }

    fn test_backend_ref(name: &str) -> HTTPBackendRef {
        HTTPBackendRef {
            name: name.to_string(),
            namespace: None,
            port: Some(443),
            weight: None,
            group: None,
            kind: None,
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        }
    }

    /// Minimal `HttpRouteRuleUnit` fixture carrying only what the plugin reads:
    /// `rule.backend_refs` and `matched_info.rsc.ns`.
    fn test_route_unit(ns: &str, backend_refs: Vec<HTTPBackendRef>) -> HttpRouteRuleUnit {
        let backends = backend_refs
            .iter()
            .enumerate()
            .map(|(slot, backend)| {
                CompiledBackend::compile(
                    backend.clone(),
                    ns,
                    slot,
                    Arc::new(crate::plugins::PluginRuntime::new()),
                )
            })
            .collect();
        HttpRouteRuleUnit {
            resource_key: format!("{}/test-route", ns),
            matched_info: MatchInfo {
                rsc: Rsc::new("HTTPRoute", ns, "test-route"),
                rule_id: 0,
                match_id: 0,
                sv: 0,
                m: HTTPRouteMatch {
                    path: None,
                    headers: None,
                    query_params: None,
                    method: None,
                },
                labels_meta: None,
                annotations_meta: None,
            },
            rule: Arc::new(HTTPRouteRule {
                name: None,
                resolved_terminal_route_uid: None,
                resolved_ai_admission: None,
                resolved_terminal_rule_identity: None,
                matches: None,
                filters: None,
                backend_refs: Some(backend_refs),
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
                has_method: false,
                header_count: 0,
                query_count: 0,
                creation_ts_ms: 0,
                route_key: format!("{}/test-route", ns),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(crate::plugins::PluginRuntime::new()),
            backends: Arc::new(backends),
            backend_finder: Arc::new(crate::lb::BackendSelector::new()),
        }
    }

    fn ctx_with_backend_refs(ns: &str, backend_refs: Vec<HTTPBackendRef>) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(Arc::new(test_route_unit(ns, backend_refs)));
        ctx
    }

    fn base_config() -> DirectEndpointConfig {
        DirectEndpointConfig {
            from: KeyGet::Header {
                name: "X-Target-Endpoint".to_string(),
            },
            extract: None,
            port: None,
            on_missing: UpstreamFallback::Fallback,
            on_invalid: UpstreamFallback::Reject,
            inherit_tls: false,
            compiled_regex: None,
        }
    }

    #[test]
    fn direct_endpoint_name_is_static_across_instances() {
        let first = DirectEndpoint::new(&base_config());
        let second = DirectEndpoint::new(&DirectEndpointConfig {
            port: Some(8080),
            ..base_config()
        });

        assert_eq!(first.name(), "DirectEndpoint");
        assert_eq!(second.name(), "DirectEndpoint");
    }

    /// A successful selection (resolved endpoint validated against a real
    /// EndpointSlice, `SetJumpOutcome::Set`) must record `Success` with the
    /// resolved `"ip:port"` address — the value the deleted `apply_dye_headers`
    /// call used to pass. Also proves the stale "fall through to existing
    /// log/header writes below" comment site now carries the record call.
    #[tokio::test]
    async fn direct_endpoint_records_success_outcome_with_addr() {
        insert_test_endpoint_slice("unit-test-ns", "app-v1", "de-success-slice", "10.0.9.9", 9191);

        let plugin = DirectEndpoint::new(&base_config());

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs("unit-test-ns", vec![test_backend_ref("app-v1")]));
        session.expect_key_get().return_const(Some("10.0.9.9:9191".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::Direct(info)
                        if info.addr == "10.0.9.9:9191".parse().unwrap()
                            && info.tls_policy.is_absent()
                            && info.backend_ref_idx == 0
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "10.0.9.9:9191")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("endpoint "));
        assert!(!log.contains("jump locked, rejected"));
    }

    #[tokio::test]
    async fn rejected_jump_records_only_locked_log_and_no_dye() {
        insert_test_endpoint_slice(
            "unit-test-rejected-ns",
            "app-rejected",
            "de-rejected-slice",
            "10.0.9.10",
            9192,
        );

        let plugin = DirectEndpoint::new(&base_config());
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_backend_refs(
            "unit-test-rejected-ns",
            vec![test_backend_ref("app-rejected")],
        ));
        session
            .expect_key_get()
            .return_const(Some("10.0.9.10:9192".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::Direct(info) if info.addr == "10.0.9.10:9192".parse().unwrap()
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Rejected);
        // No record_outcome expectation: any dye publication fails the mock.

        let mut log = PluginLog::new("DirectEndpoint");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            serde_json::json!(["jump locked, rejected"])
        );
    }

    /// Missing endpoint value with `on_missing: Reject` must record `Reject`
    /// with cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn direct_endpoint_records_reject_outcome_on_missing_value() {
        let config = DirectEndpointConfig {
            on_missing: UpstreamFallback::Reject,
            ..base_config()
        };
        let plugin = DirectEndpoint::new(&config);

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(None::<String>);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Reject && v == "on-missing")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some(MISSING_TARGET_ENDPOINT_BODY),
            }
        );
    }

    /// An endpoint that fails route-membership validation (not in any
    /// backend_ref's EndpointSlices) with `on_invalid: Fallback` must record
    /// `Fallback` with cause `on-invalid` and pass the request through.
    #[tokio::test]
    async fn direct_endpoint_records_fallback_outcome_on_invalid_endpoint() {
        let config = DirectEndpointConfig {
            on_invalid: UpstreamFallback::Fallback,
            ..base_config()
        };
        let plugin = DirectEndpoint::new(&config);

        let mut session = MockPluginSession::new();
        // No EndpointSlice registered for this ip:port under "app-v1" — the
        // security pre-validation must reject it as not-in-route.
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs("unit-test-ns", vec![test_backend_ref("app-v1")]));
        session
            .expect_key_get()
            .return_const(Some("192.0.2.55:7777".to_string()));
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-invalid")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn regex_miss_uses_missing_policy_and_fixed_body() {
        for (on_missing, expected) in [
            (
                UpstreamFallback::Reject,
                PluginRunningResult::ErrResponse {
                    status: 400,
                    body: Some(MISSING_TARGET_ENDPOINT_BODY),
                },
            ),
            (UpstreamFallback::Fallback, PluginRunningResult::GoodNext),
        ] {
            let config = DirectEndpointConfig {
                extract: Some(EndpointExtract {
                    regex: r"^target=(.+)$".to_string(),
                    group: 1,
                }),
                on_missing,
                ..base_config()
            };
            let plugin = DirectEndpoint::new(&config);
            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx_with_backend_refs(
                "unit-test-regex-miss-ns",
                vec![test_backend_ref("regex-miss-service")],
            ));
            session.expect_key_get().return_const(Some("no-match".to_string()));
            let expected_outcome = if on_missing == UpstreamFallback::Reject {
                DyeOutcome::Reject
            } else {
                DyeOutcome::Fallback
            };
            session
                .expect_record_outcome()
                .withf(move |outcome, value| *outcome == expected_outcome && value == "on-missing")
                .times(1)
                .return_const(());

            let mut log = PluginLog::new("DirectEndpoint");
            assert_eq!(plugin.run_request(&mut session, &mut log).await, expected);
            assert!(log.contains("endpoint miss"));
        }
    }

    #[tokio::test]
    async fn malformed_endpoint_rejects_with_fixed_invalid_body() {
        let plugin = DirectEndpoint::new(&base_config());
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_backend_refs(
            "unit-test-malformed-ns",
            vec![test_backend_ref("malformed-service")],
        ));
        session
            .expect_key_get()
            .return_const(Some("[2001:db8::1]tail".to_string()));
        session
            .expect_record_outcome()
            .withf(|outcome, value| *outcome == DyeOutcome::Reject && value == "on-invalid")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some(INVALID_TARGET_ENDPOINT_BODY),
            }
        );
        assert!(log.contains("endpoint invalid"));
        assert!(!format!("{log:?}").contains("2001:db8"));
    }

    #[tokio::test]
    async fn endpoint_outside_route_rejects_with_fixed_not_allowed_body() {
        let plugin = DirectEndpoint::new(&base_config());
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_backend_refs(
            "unit-test-not-allowed-ns",
            vec![test_backend_ref("not-allowed-service")],
        ));
        session
            .expect_key_get()
            .return_const(Some("192.0.2.56:7778".to_string()));
        session
            .expect_record_outcome()
            .withf(|outcome, value| *outcome == DyeOutcome::Reject && value == "on-invalid")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(TARGET_ENDPOINT_NOT_ALLOWED_BODY),
            }
        );
    }

    #[tokio::test]
    async fn unsafe_endpoint_in_route_rejects_with_fixed_not_allowed_body() {
        insert_test_endpoint_slice(
            "unit-test-unsafe-ns",
            "unsafe-service",
            "de-unsafe-slice",
            "169.254.169.254",
            9193,
        );

        let plugin = DirectEndpoint::new(&base_config());
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_backend_refs(
            "unit-test-unsafe-ns",
            vec![test_backend_ref("unsafe-service")],
        ));
        session
            .expect_key_get()
            .return_const(Some("169.254.169.254:9193".to_string()));
        session
            .expect_record_outcome()
            .withf(|outcome, value| *outcome == DyeOutcome::Reject && value == "on-invalid")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(TARGET_ENDPOINT_NOT_ALLOWED_BODY),
            }
        );
    }

    #[test]
    fn parse_endpoint_accepts_supported_address_forms() {
        for (value, expected_ip, expected_port) in [
            ("1.2.3.4", "1.2.3.4", None),
            ("1.2.3.4:80", "1.2.3.4", Some(80)),
            ("1.2.3.4:0", "1.2.3.4", Some(0)),
            ("::1", "::1", None),
            ("2001:db8::1", "2001:db8::1", None),
            ("[::1]", "::1", None),
            ("[2001:db8::1]:8080", "2001:db8::1", Some(8080)),
            ("[2001:db8::1]:65535", "2001:db8::1", Some(65535)),
        ] {
            assert_eq!(
                parse_endpoint(value).unwrap(),
                (expected_ip.parse().unwrap(), expected_port)
            );
        }
    }

    #[test]
    fn parse_endpoint_rejects_invalid_bracket_suffixes() {
        for value in [
            "invalid",
            "[::1",
            "[::1]tail",
            "[::1]80",
            "[::1]:",
            "[::1]:65536",
            "[::1]:80tail",
            "[::1]:80:90",
            "[::1]]",
            "[192.0.2.1]",
            "[192.0.2.1]:80",
        ] {
            assert_eq!(
                parse_endpoint(value),
                Err(InvalidEndpoint),
                "unexpectedly accepted {value}"
            );
        }
    }
}
