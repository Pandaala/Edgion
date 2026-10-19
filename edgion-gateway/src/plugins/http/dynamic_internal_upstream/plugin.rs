use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::routes_mgr::RouteRules;
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_resources::err::EdError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DynamicInternalUpstreamConfig, UpstreamFallback};

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
        let Some((target_name, target_namespace)) = self.config.resolve_target(routing_key) else {
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
        };

        // 4. Pre-validate the resolved target against the route's canonical
        //    compiled backend view before publishing the jump.
        let ctx = session.ctx();
        let Some(route_unit) = ctx.route_unit.as_ref() else {
            log.push("no route");
            return PluginRunningResult::GoodNext; // No route matched, skip
        };
        // Kept ahead of the lookup on purpose: a rule that declares no
        // backendRefs at all is not a target the operator got wrong, so it
        // passes through without engaging `on_invalid` or recording an
        // outcome. Folding it into the lookup below would turn it into a
        // policy decision.
        match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => {}
            _ => {
                log.push("no backend refs");
                return PluginRunningResult::GoodNext; // No backend_refs, skip
            }
        };

        // `validate_backend_by_name` uses the single backend matcher: it honours the
        // Controller's `ref_denied` decision, so a denied cross-namespace
        // target follows `on_invalid` here instead of being published as a
        // jump the upstream selector would have to reject later.
        if let Err(err) = RouteRules::validate_backend_by_name(route_unit, target_name, target_namespace) {
            // Both causes take `on_invalid`, but they are diagnosed apart: a
            // shared "not found" line sends an operator debugging a missing
            // ReferenceGrant hunting for a Service that exists.
            log.push_err(match err {
                EdError::RefDenied { .. } => "backend ref denied",
                EdError::BackendNotFound() => "backend ref not found",
                // Named explicitly rather than folded into the miss label: a
                // future lookup outcome must not be mis-diagnosed as absent.
                _ => "backend ref unusable",
            });
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
#[path = "../../../../tests/unit/plugins/http/dynamic_internal_upstream/plugin_test.rs"]
mod tests;
