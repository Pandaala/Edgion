use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    DynUpstreamOnInvalid, DynUpstreamOnMissing, DynUpstreamOnNoMatch, DynamicInternalUpstreamConfig,
};
use crate::types::InternalJumpPreset;

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
            log.push("Skip gRPC; ");
            return PluginRunningResult::GoodNext;
        }

        // 1. Check config validation
        if let Some(err) = self.config.get_validation_error() {
            log.push(&format!("CfgErr {}; ", err));
            return PluginRunningResult::GoodNext; // Fail-open on config error
        }

        // 2. Extract raw routing key via KeyGet
        let raw_value = match session.key_get(&self.config.from).await {
            Some(v) if !v.is_empty() => v,
            _ => {
                return match self.config.on_missing {
                    DynUpstreamOnMissing::Fallback => {
                        log.push("NoVal fallback; ");
                        PluginRunningResult::GoodNext
                    }
                    DynUpstreamOnMissing::Reject => {
                        log.push("NoVal 400; ");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some("Missing routing key for backend selection".to_string()),
                        }
                    }
                };
            }
        };

        // 3. Apply regex extraction if configured
        let routing_key = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => {
                        log.push("Regex miss; ");
                        return match self.config.on_missing {
                            DynUpstreamOnMissing::Fallback => PluginRunningResult::GoodNext,
                            DynUpstreamOnMissing::Reject => PluginRunningResult::ErrResponse {
                                status: 400,
                                body: Some("Routing key regex extraction failed".to_string()),
                            },
                        };
                    }
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 4. Resolve routing key → target backend_ref (name, namespace)
        let (target_name, target_namespace) = match self.config.resolve_target(&routing_key) {
            Some((name, ns)) => (name.to_string(), ns.map(|s| s.to_string())),
            None => {
                // Rules mode: no rule matched
                log.push(&format!("NoMatch '{}'; ", routing_key));
                return match self.config.on_no_match {
                    DynUpstreamOnNoMatch::Fallback => PluginRunningResult::GoodNext,
                    DynUpstreamOnNoMatch::Reject => PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some("No matching backend rule for routing key".to_string()),
                    },
                };
            }
        };

        // 5. Pre-validate that target backend_ref exists in route's backend_refs
        let ctx = session.ctx();
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit,
            None => {
                log.push("NoRoute; ");
                return PluginRunningResult::GoodNext; // No route matched, skip
            }
        };
        let route_ns = &route_unit.matched_info.rns;
        let backend_refs = match route_unit.rule.backend_refs.as_ref() {
            Some(refs) if !refs.is_empty() => refs,
            _ => {
                log.push("NoBkRefs; ");
                return PluginRunningResult::GoodNext; // No backend_refs, skip
            }
        };

        // Pre-validate: check if target name exists in backend_refs
        let found = backend_refs.iter().any(|br| {
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
            tracing::warn!(
                target_name = %target_name,
                target_namespace = ?target_namespace,
                "DynamicInternalUpstream: backend_ref not found in route"
            );
            log.push("Deny; ");
            return match self.config.on_invalid {
                DynUpstreamOnInvalid::Reject => PluginRunningResult::ErrResponse {
                    status: 403,
                    body: Some("Target backend not allowed".to_string()),
                },
                DynUpstreamOnInvalid::Fallback => PluginRunningResult::GoodNext,
            };
        }

        // 6. Build InternalJumpPreset and set in context
        let info = InternalJumpPreset {
            backend_ref_name: target_name.clone(),
            backend_ref_namespace: target_namespace.clone(),
        };
        session.set_internal_jump(info);

        log.push(&format!(
            "OK {}{}; ",
            target_name,
            target_namespace
                .as_ref()
                .map(|ns| format!("@{}", ns))
                .unwrap_or_default()
        ));

        // 7. Optionally set debug request header (sent to upstream for tracing)
        if self.config.debug_header {
            let header_value = match &target_namespace {
                Some(ns) => format!("{}/{}", ns, target_name),
                None => target_name,
            };
            let _ = session.set_request_header("X-Dynamic-Internal-Upstream", &header_value);
        }

        PluginRunningResult::GoodNext
    }
}
