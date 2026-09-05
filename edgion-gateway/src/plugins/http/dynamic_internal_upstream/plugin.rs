use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DynamicInternalUpstreamConfig, UpstreamFallback};
use edgion_resources::HTTPBackendRefKind;

pub struct DynamicInternalUpstream {
    config: DynamicInternalUpstreamConfig,
}

impl DynamicInternalUpstream {
    pub fn create(config: &DynamicInternalUpstreamConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();
        Box::new(Self { config: validated })
    }
}

#[async_trait]
impl RequestFilter for DynamicInternalUpstream {
    fn name(&self) -> &str {
        "DynamicInternalUpstream"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw routing key via KeyGet
        let raw_value = match session.key_get(&self.config.from) {
            Some(v) if !v.is_empty() => v,
            _ => {
                return match self.config.on_missing {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
                        log.push_err("no routing key");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some(Bytes::from_static(b"Missing routing key for backend selection")),
                        }
                    }
                };
            }
        };

        // 2. Apply regex extraction if configured
        let routing_key = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str(),
                    None => {
                        return match self.config.on_missing {
                            UpstreamFallback::Fallback => {
                                session.record_outcome(DyeOutcome::Fallback, "on-missing");
                                log.push("upstream miss");
                                PluginRunningResult::GoodNext
                            }
                            UpstreamFallback::Reject => {
                                session.record_outcome(DyeOutcome::Reject, "on-missing");
                                log.push_err("regex extract fail");
                                PluginRunningResult::ErrResponse {
                                    status: 400,
                                    body: Some(Bytes::from_static(b"Routing key regex extraction failed")),
                                }
                            }
                        };
                    }
                }
            } else {
                raw_value.as_str()
            }
        } else {
            raw_value.as_str()
        };

        // 3. Resolve routing key → target backend_ref (name, namespace)
        let (target_name, target_namespace) = match self.config.resolve_target(routing_key) {
            Some(target) => target,
            None => {
                // Rules mode: no rule matched
                return match self.config.on_no_match {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-no-match");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-no-match");
                        log.push_err("no matching rule");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some(Bytes::from_static(b"No matching backend rule for routing key")),
                        }
                    }
                };
            }
        };

        // 4. Pre-validate that target backend_ref exists in route's backend_refs
        let ctx = session.ctx();
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit,
            None => {
                log.push("no route");
                return PluginRunningResult::GoodNext; // No route matched, skip
            }
        };
        let route_ns = &route_unit.matched_info.rsc.ns;
        let backend_refs = match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => refs,
            _ => {
                log.push("no backend refs");
                return PluginRunningResult::GoodNext; // No backend_refs, skip
            }
        };

        // Pre-validate: check if target name exists in backend_refs
        let found = backend_refs.iter().any(|br| {
            if br.backend_kind() != HTTPBackendRefKind::Service {
                return false;
            }
            if br.name.as_str() != target_name {
                return false;
            }
            if let Some(ns) = target_namespace {
                let br_ns = br.namespace.as_deref().unwrap_or(route_ns);
                br_ns == ns
            } else {
                true
            }
        });

        if !found {
            log.push_err("backend ref not found");
            return match self.config.on_invalid {
                UpstreamFallback::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-invalid");
                    PluginRunningResult::ErrResponse {
                        status: 403,
                        body: Some(Bytes::from_static(b"Target backend not allowed")),
                    }
                }
                UpstreamFallback::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                    PluginRunningResult::GoodNext
                }
            };
        }

        // 5. Clone only at the owned request-context boundary.
        let info = InternalJumpPreset {
            backend_ref_name: target_name.to_owned(),
            backend_ref_namespace: target_namespace.map(str::to_owned),
        };
        match session.set_upstream_jump(UpstreamJumpTarget::Internal(info)) {
            SetJumpOutcome::Set => {
                match target_namespace {
                    Some(namespace) => {
                        let resolved = format!("{}/{}", namespace, target_name);
                        session.record_outcome(DyeOutcome::Success, &resolved);
                    }
                    None => session.record_outcome(DyeOutcome::Success, target_name),
                }
                log.push("upstream selected");
            }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
                return PluginRunningResult::GoodNext;
            }
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
    use crate::{EdgionHttpContext, MatchInfo, Rsc};
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        DynUpstreamExtract, DynUpstreamRule, DynUpstreamTarget,
    };
    use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
    use std::sync::Arc;

    /// Minimal `HTTPBackendRef` for route-unit fixtures (only `name` matters to
    /// the plugin under test — the rest are the schema's non-operator defaults).
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

    fn test_backend_ref_in(name: &str, namespace: &str) -> HTTPBackendRef {
        HTTPBackendRef {
            namespace: Some(namespace.to_string()),
            ..test_backend_ref(name)
        }
    }

    /// Minimal `HttpRouteRuleUnit` fixture carrying only what the plugin reads:
    /// `rule.backend_refs` and `matched_info.rsc.ns`. The compiled-runtime
    /// fields are never touched by this plugin's `run_request`.
    fn test_route_unit(backend_refs: Vec<HTTPBackendRef>) -> HttpRouteRuleUnit {
        HttpRouteRuleUnit {
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
                route_key: "test-ns/test-route".to_string(),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(crate::plugins::PluginRuntime::new()),
            backends: Arc::new(vec![]),
            backend_finder: Arc::new(crate::lb::BackendSelector::new()),
        }
    }

    fn ctx_with_backend_refs(backend_refs: Vec<HTTPBackendRef>) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(Arc::new(test_route_unit(backend_refs)));
        ctx
    }

    /// Direct mode (`rules: None`): the routing key extracted via `from` IS the
    /// backend_ref name.
    fn direct_mode_config() -> DynamicInternalUpstreamConfig {
        DynamicInternalUpstreamConfig {
            from: KeyGet::Header {
                name: "X-Backend-Target".to_string(),
            },
            extract: None,
            rules: None,
            on_missing: UpstreamFallback::Fallback,
            on_no_match: UpstreamFallback::Fallback,
            on_invalid: UpstreamFallback::Reject,
            compiled_regex: None,
        }
    }

    #[test]
    fn dynamic_internal_upstream_name_is_static_across_instances() {
        let first = DynamicInternalUpstream::create(&direct_mode_config());
        let second = DynamicInternalUpstream::create(&DynamicInternalUpstreamConfig {
            on_invalid: UpstreamFallback::Fallback,
            ..direct_mode_config()
        });

        assert_eq!(first.name(), "DynamicInternalUpstream");
        assert_eq!(second.name(), "DynamicInternalUpstream");
    }

    #[tokio::test]
    async fn regex_capture_selects_namespaced_rule_and_records_namespaced_success() {
        let config = DynamicInternalUpstreamConfig {
            extract: Some(DynUpstreamExtract {
                regex: r"^tier=(.+)$".to_string(),
                group: 1,
            }),
            rules: Some(vec![DynUpstreamRule {
                value: "premium".to_string(),
                backend_ref: DynUpstreamTarget {
                    name: "premium-api".to_string(),
                    namespace: Some("shared-services".to_string()),
                },
            }]),
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs(vec![test_backend_ref_in(
                "premium-api",
                "shared-services",
            )]));
        session.expect_key_get().return_const(Some("tier=premium".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::Internal(jump)
                        if jump.backend_ref_name == "premium-api"
                            && jump.backend_ref_namespace.as_deref() == Some("shared-services")
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|outcome, value| *outcome == DyeOutcome::Success && value == "shared-services/premium-api")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicInternalUpstream");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("upstream selected"));
    }

    /// A successful selection (resolved backend_ref found in the route's
    /// backend_refs, `SetJumpOutcome::Set`) must record `Success` with the
    /// resolved `"[namespace/]name"` value — the value the deleted
    /// `apply_dye_headers` call used to pass.
    #[tokio::test]
    async fn diu_records_success_outcome_with_resolved_name() {
        let plugin = DynamicInternalUpstream::create(&direct_mode_config());

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs(vec![test_backend_ref("app-v1")]));
        session.expect_key_get().return_const(Some("app-v1".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::Internal(jump)
                        if jump.backend_ref_name == "app-v1" && jump.backend_ref_namespace.is_none()
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "app-v1")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicInternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("upstream selected"));
    }

    #[tokio::test]
    async fn rejected_jump_records_only_locked_log_and_no_success_dye() {
        let config = DynamicInternalUpstreamConfig {
            rules: Some(vec![DynUpstreamRule {
                value: "premium".to_string(),
                backend_ref: DynUpstreamTarget {
                    name: "premium-api".to_string(),
                    namespace: Some("shared-services".to_string()),
                },
            }]),
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs(vec![test_backend_ref_in(
                "premium-api",
                "shared-services",
            )]));
        session.expect_key_get().return_const(Some("premium".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::Internal(jump)
                        if jump.backend_ref_name == "premium-api"
                            && jump.backend_ref_namespace.as_deref() == Some("shared-services")
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Rejected);
        // No record_outcome expectation: any Success Dye publication fails the mock.

        let mut log = PluginLog::new("DynamicInternalUpstream");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            serde_json::json!(["jump locked, rejected"])
        );
    }

    /// Missing routing key with `on_missing: Reject` must record `Reject` with
    /// cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn diu_records_reject_outcome_on_missing_key() {
        let config = DynamicInternalUpstreamConfig {
            on_missing: UpstreamFallback::Reject,
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(None::<String>);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Reject && v == "on-missing")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicInternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some(Bytes::from_static(b"Missing routing key for backend selection")),
            }
        );
    }

    /// `on_invalid: Fallback` on an unresolvable backend ref must record
    /// `Fallback` with cause `on-invalid` and pass the request through.
    #[tokio::test]
    async fn diu_records_fallback_outcome_on_invalid_backend() {
        let config = DynamicInternalUpstreamConfig {
            on_invalid: UpstreamFallback::Fallback,
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        // Route's only backend_ref is "other-svc" — never matches the resolved
        // "app-v1" target (direct mode), so the `!found` branch fires.
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs(vec![test_backend_ref("other-svc")]));
        session.expect_key_get().return_const(Some("app-v1".to_string()));
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-invalid")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicInternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
