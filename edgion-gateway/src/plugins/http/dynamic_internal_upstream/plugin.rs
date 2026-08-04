use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    DynUpstreamOnInvalid, DynUpstreamOnMissing, DynUpstreamOnNoMatch, DynamicInternalUpstreamConfig,
};
use edgion_resources::HTTPBackendRefKind;

pub struct DynamicInternalUpstream {
    name: String,
    config: DynamicInternalUpstreamConfig,
}

impl DynamicInternalUpstream {
    pub fn create(config: &DynamicInternalUpstreamConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();
        Box::new(Self {
            name: "DynamicInternalUpstream".to_string(),
            config: validated,
        })
    }
}

#[async_trait]
impl RequestFilter for DynamicInternalUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw routing key via KeyGet
        let raw_value = match session.key_get(&self.config.from).await {
            Some(v) if !v.is_empty() => v,
            _ => {
                return match self.config.on_missing {
                    DynUpstreamOnMissing::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    DynUpstreamOnMissing::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
                        log.push_err("no routing key");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some("Missing routing key for backend selection".to_string()),
                        }
                    }
                };
            }
        };

        // 2. Apply regex extraction if configured
        let routing_key = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => {
                        return match self.config.on_missing {
                            DynUpstreamOnMissing::Fallback => {
                                session.record_outcome(DyeOutcome::Fallback, "on-missing");
                                log.push("upstream miss");
                                PluginRunningResult::GoodNext
                            }
                            DynUpstreamOnMissing::Reject => {
                                session.record_outcome(DyeOutcome::Reject, "on-missing");
                                log.push_err("regex extract fail");
                                PluginRunningResult::ErrResponse {
                                    status: 400,
                                    body: Some("Routing key regex extraction failed".to_string()),
                                }
                            }
                        };
                    }
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 3. Resolve routing key → target backend_ref (name, namespace)
        let (target_name, target_namespace) = match self.config.resolve_target(&routing_key) {
            Some((name, ns)) => (name.to_string(), ns.map(|s| s.to_string())),
            None => {
                // Rules mode: no rule matched
                return match self.config.on_no_match {
                    DynUpstreamOnNoMatch::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-no-match");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    DynUpstreamOnNoMatch::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-no-match");
                        log.push_err("no matching rule");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some("No matching backend rule for routing key".to_string()),
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
            if br.name != target_name {
                return false;
            }
            if let Some(ref ns) = target_namespace {
                let br_ns = br.namespace.as_deref().unwrap_or(route_ns);
                br_ns == ns.as_str()
            } else {
                true
            }
        });

        if !found {
            log.push_err("backend ref not found");
            return match self.config.on_invalid {
                DynUpstreamOnInvalid::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-invalid");
                    PluginRunningResult::ErrResponse {
                        status: 403,
                        body: Some("Target backend not allowed".to_string()),
                    }
                }
                DynUpstreamOnInvalid::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                    PluginRunningResult::GoodNext
                }
            };
        }

        // 5. Build InternalJumpPreset and set in context
        // Resolved selection value for dye rendering: "[namespace/]name".
        let resolved = match &target_namespace {
            Some(ns) => format!("{}/{}", ns, target_name),
            None => target_name.clone(),
        };
        let info = InternalJumpPreset {
            backend_ref_name: target_name,
            backend_ref_namespace: target_namespace,
        };
        match session.set_upstream_jump(UpstreamJumpTarget::Internal(info)) {
            SetJumpOutcome::Set => {
                session.record_outcome(DyeOutcome::Success, &resolved);
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
            on_missing: DynUpstreamOnMissing::default(),
            on_no_match: DynUpstreamOnNoMatch::default(),
            on_invalid: DynUpstreamOnInvalid::default(),
            compiled_regex: None,
        }
    }

    /// A successful selection (resolved backend_ref found in the route's
    /// backend_refs, `SetJumpOutcome::Set`) must record `Success` with the
    /// resolved `"[namespace/]name"` value — the value the deleted
    /// `apply_dye_headers` call used to pass.
    #[tokio::test]
    async fn diu_records_success_outcome_with_resolved_name() {
        let plugin = DynamicInternalUpstream {
            name: "DynamicInternalUpstream".to_string(),
            config: direct_mode_config(),
        };

        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .return_const(ctx_with_backend_refs(vec![test_backend_ref("app-v1")]));
        session.expect_key_get().return_const(Some("app-v1".to_string()));
        session.expect_set_upstream_jump().return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "app-v1")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicInternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    /// Missing routing key with `on_missing: Reject` must record `Reject` with
    /// cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn diu_records_reject_outcome_on_missing_key() {
        let config = DynamicInternalUpstreamConfig {
            on_missing: DynUpstreamOnMissing::Reject,
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream {
            name: "DynamicInternalUpstream".to_string(),
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

        let mut log = PluginLog::new("DynamicInternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some("Missing routing key for backend selection".to_string()),
            }
        );
    }

    /// `on_invalid: Fallback` on an unresolvable backend ref must record
    /// `Fallback` with cause `on-invalid` and pass the request through.
    #[tokio::test]
    async fn diu_records_fallback_outcome_on_invalid_backend() {
        let config = DynamicInternalUpstreamConfig {
            on_invalid: DynUpstreamOnInvalid::Fallback,
            ..direct_mode_config()
        };
        let plugin = DynamicInternalUpstream {
            name: "DynamicInternalUpstream".to_string(),
            config,
        };

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
