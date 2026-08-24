use async_trait::async_trait;
use std::net::IpAddr;

use crate::backends::{validate_endpoint_backend_addr, validate_endpoint_in_route};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{DirectEndpointPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DirectEndpointConfig, UpstreamFallback};
use edgion_resources::HTTPBackendRef;

pub struct DirectEndpoint {
    name: String,
    config: DirectEndpointConfig,
}

impl DirectEndpoint {
    pub fn new(config: &DirectEndpointConfig) -> Self {
        let mut validated = config.clone();
        validated.validate();
        Self {
            name: "DirectEndpoint".to_string(),
            config: validated,
        }
    }

    /// Pure function to resolve endpoint info from raw value + context
    fn resolve(
        &self,
        raw_value: String,
        backend_refs: &[HTTPBackendRef],
        route_ns: &str,
        allow_loopback_upstream: bool,
    ) -> Result<DirectEndpointPreset, ResolutionError> {
        // 1. Apply regex extraction if configured
        let endpoint_str = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => return Err(ResolutionError::Missing("Regex miss".to_string())),
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 2. Parse IP (support "ip" or "ip:port" format)
        let (target_ip, extracted_port) =
            parse_endpoint(&endpoint_str).map_err(|e| ResolutionError::Invalid(format!("ParseErr {}", e)))?;

        // 3. Security: validate against route's backend_ref endpoints
        let (backend_idx, effective_port) =
            match validate_endpoint_in_route(&target_ip, extracted_port.or(self.config.port), backend_refs, route_ns) {
                Ok(result) => result,
                Err(reason) => {
                    return Err(ResolutionError::Invalid(reason)); // Becomes Deny
                }
            };

        // 4. Determine TLS config
        let (use_tls, sni) = if self.config.inherit_tls {
            extract_tls_from_backend_ref(&backend_refs[backend_idx])
        } else {
            (false, String::new())
        };

        let addr = std::net::SocketAddr::new(target_ip, effective_port);
        if validate_endpoint_backend_addr(
            &pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
            allow_loopback_upstream,
        )
        .is_err()
        {
            return Err(ResolutionError::Invalid(
                "loopback/link-local/metadata endpoint address rejected".to_string(),
            ));
        }

        Ok(DirectEndpointPreset {
            addr,
            use_tls,
            sni,
            backend_ref_idx: backend_idx,
        })
    }
}

enum ResolutionError {
    Missing(String),
    Invalid(String),
}

#[async_trait]
impl RequestFilter for DirectEndpoint {
    fn name(&self) -> &str {
        &self.name
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
                        body: Some("Missing target endpoint".to_string()),
                    };
                }
            },
        };

        // 2. Get route context
        let ctx = session.ctx();
        let (backend_refs, route_ns) = match ctx.route_unit.as_ref() {
            Some(unit) if unit.rule.backend_refs.as_ref().is_some_and(|r| !r.is_empty()) => (
                unit.rule
                    .backend_refs
                    .as_ref()
                    .expect("guarded by is_some_and match arm"),
                unit.matched_info.rsc.ns.as_str(),
            ),
            _ => {
                log.push("no route refs");
                return PluginRunningResult::GoodNext;
            }
        };

        // 3. Resolve endpoint info
        match self.resolve(raw_value, backend_refs, route_ns, session.ctx().allow_loopback_upstream) {
            Ok(info) => {
                let addr_str = format!("{}", info.addr);
                log.push_fmt("endpoint {}", la![sensitive(&addr_str)]);
                // Clone the matched backend_ref alongside the preset — the gateway's
                // peer-construction path uses it via ctx.selected_backend to fill
                // rsc / service_key / service_kind without re-deriving them.
                let backend_ref = backend_refs[info.backend_ref_idx].clone();
                match session.set_upstream_jump(UpstreamJumpTarget::Direct {
                    preset: info,
                    backend_ref: Box::new(backend_ref),
                }) {
                    SetJumpOutcome::Set => {
                        session.record_outcome(DyeOutcome::Success, &addr_str);
                    }
                    SetJumpOutcome::Rejected => {
                        log.push("jump locked, rejected");
                        return PluginRunningResult::GoodNext;
                    }
                }

                PluginRunningResult::GoodNext
            }
            Err(ResolutionError::Missing(msg)) => {
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
                            body: Some(format!("DirectEndpoint missing: {}", msg)),
                        }
                    }
                }
            }
            Err(ResolutionError::Invalid(msg)) => {
                log.push("endpoint invalid");
                match self.config.on_invalid {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-invalid");
                        // 400 for parse errors ("ParseErr" in msg), 403 for validation denials.
                        let status = if msg.contains("ParseErr") { 400 } else { 403 };
                        PluginRunningResult::ErrResponse {
                            status,
                            body: Some(format!("DirectEndpoint invalid: {}", msg)),
                        }
                    }
                }
            }
        }
    }
}

/// Extract TLS config from a backend_ref's BackendTLSPolicy
fn extract_tls_from_backend_ref(br: &HTTPBackendRef) -> (bool, String) {
    match &br.backend_tls_policy {
        Some(policy) => (true, policy.spec.validation.hostname.clone()),
        None => (false, String::new()),
    }
}

/// Parse endpoint string into (IpAddr, optional port)
fn parse_endpoint(s: &str) -> Result<(IpAddr, Option<u16>), String> {
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return Ok((addr.ip(), Some(addr.port())));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok((ip, None));
    }
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            let ip_str = &s[1..bracket_end];
            let ip: IpAddr = ip_str.parse().map_err(|e| format!("{}", e))?;
            let port = if bracket_end + 1 < s.len() && s.as_bytes()[bracket_end + 1] == b':' {
                Some(s[bracket_end + 2..].parse::<u16>().map_err(|e| format!("{}", e))?)
            } else {
                None
            };
            return Ok((ip, port));
        }
    }
    Err(format!("cannot parse '{}' as endpoint", s))
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
    use edgion_resources::{HTTPRouteMatch, HTTPRouteRule};
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    /// Register a single-address EndpointSlice in the global roundrobin store
    /// (additive `update_data_only`, so it does not disturb other tests sharing
    /// the process-wide singleton) so `validate_endpoint_in_route` finds `ip:port`
    /// as a legitimate address for `ns/service_name`.
    fn insert_test_endpoint_slice(ns: &str, service_name: &str, slice_name: &str, ip: &str, port: i32) {
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
            port: None,
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
            backends: Arc::new(vec![]),
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

    /// A successful selection (resolved endpoint validated against a real
    /// EndpointSlice, `SetJumpOutcome::Set`) must record `Success` with the
    /// resolved `"ip:port"` address — the value the deleted `apply_dye_headers`
    /// call used to pass. Also proves the stale "fall through to existing
    /// log/header writes below" comment site now carries the record call.
    #[tokio::test]
    async fn direct_endpoint_records_success_outcome_with_addr() {
        insert_test_endpoint_slice("unit-test-ns", "app-v1", "de-success-slice", "10.0.9.9", 9191);

        let plugin = DirectEndpoint {
            name: "DirectEndpoint".to_string(),
            config: base_config(),
        };

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs("unit-test-ns", vec![test_backend_ref("app-v1")]));
        session.expect_key_get().return_const(Some("10.0.9.9:9191".to_string()));
        session.expect_set_upstream_jump().return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "10.0.9.9:9191")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DirectEndpoint");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    /// Missing endpoint value with `on_missing: Reject` must record `Reject`
    /// with cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn direct_endpoint_records_reject_outcome_on_missing_value() {
        let config = DirectEndpointConfig {
            on_missing: UpstreamFallback::Reject,
            ..base_config()
        };
        let plugin = DirectEndpoint {
            name: "DirectEndpoint".to_string(),
            config,
        };

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
                body: Some("Missing target endpoint".to_string()),
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
        let plugin = DirectEndpoint {
            name: "DirectEndpoint".to_string(),
            config,
        };

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

    #[test]
    fn test_parse_endpoint() {
        assert_eq!(parse_endpoint("1.2.3.4").unwrap(), ("1.2.3.4".parse().unwrap(), None));
        assert_eq!(
            parse_endpoint("1.2.3.4:80").unwrap(),
            ("1.2.3.4".parse().unwrap(), Some(80))
        );
        assert_eq!(parse_endpoint("[::1]").unwrap(), ("::1".parse().unwrap(), None));
        assert_eq!(
            parse_endpoint("[::1]:8080").unwrap(),
            ("::1".parse().unwrap(), Some(8080))
        );
        assert!(parse_endpoint("invalid").is_err());
    }
}
