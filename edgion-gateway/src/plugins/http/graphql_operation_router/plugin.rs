use crate::plugins::http::common::graphql_request;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::routes::http::routes_mgr::RouteRules;
use crate::{InternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use async_trait::async_trait;
use edgion_resources::err::EdError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::graphql_request::GraphqlOperationType;
use edgion_resources::plugin_configs::DynUpstreamTarget;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, GraphqlOperationRouterConfig, PluginValidation};
use edgion_resources::EdgionErrorCode;
use std::collections::HashMap;

pub struct GraphqlOperationRouter {
    mappings: HashMap<GraphqlOperationType, HashMap<String, DynUpstreamTarget>>,
    default: Option<DynUpstreamTarget>,
    envelope_limit: usize,
}
impl GraphqlOperationRouter {
    pub fn create(
        config: &GraphqlOperationRouterConfig,
        body: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        Self::try_new(config, body).map(|p| Box::new(p) as Box<dyn RequestFilter>)
    }
    fn try_new(config: &GraphqlOperationRouterConfig, body: Option<BodyRequirement>) -> Option<Self> {
        if config.validation_error().is_some() {
            return None;
        }
        let envelope_limit = graphql_request::envelope_limit(body)?;
        let mut mappings: HashMap<_, HashMap<_, _>> = HashMap::new();
        for rule in &config.rules {
            mappings
                .entry(rule.operation_type)
                .or_default()
                .insert(rule.operation_name.clone(), rule.backend_ref.clone());
        }
        Some(Self {
            mappings,
            default: config.default_backend_ref.clone(),
            envelope_limit,
        })
    }
    fn target(&self, kind: GraphqlOperationType, name: Option<&str>) -> Option<(&DynUpstreamTarget, bool)> {
        let exact = name.and_then(|name| self.mappings.get(&kind)?.get(name));
        exact
            .map(|target| (target, false))
            .or_else(|| self.default.as_ref().map(|target| (target, true)))
    }
}
#[async_trait]
impl RequestFilter for GraphqlOperationRouter {
    fn name(&self) -> &str {
        "GraphqlOperationRouter"
    }
    fn requires_materialized_request_body(&self) -> bool {
        false
    }
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let Some(route) = session.ctx().route_unit.clone() else {
            log.push_err("invalid routing scope");
            return graphql_request::reject(session, 500);
        };
        if session.ctx().jump.is_some() {
            log.push_err("deny jump conflict");
            return graphql_request::reject(session, 500);
        }
        let target = match graphql_request::evaluate(session, log, self.envelope_limit, |parsed| {
            Ok(self.target(parsed.operation_type(), parsed.operation_name()))
        })
        .await
        {
            Ok(Some(target)) => target,
            Ok(None) => {
                log.push("deny no mapping");
                return graphql_request::reject(session, 400);
            }
            Err(reply) => return reply,
        };
        let (target, is_default) = target;
        let namespace = target.namespace.as_deref().unwrap_or(&route.matched_info.rsc.ns);
        if let Err(error) = RouteRules::validate_backend_by_name(&route, &target.name, Some(namespace)) {
            let status = if matches!(error, EdError::RefDenied { .. }) {
                session.record_error(EdgionErrorCode::RefDenied);
                log.push_err("backend ref denied");
                403
            } else {
                log.push_err("backend ref unusable");
                500
            };
            return graphql_request::reject(session, status);
        }
        let jump = UpstreamJumpTarget::Internal(InternalJumpPreset {
            backend_ref_name: target.name.clone(),
            backend_ref_namespace: Some(namespace.to_owned()),
        });
        if session.set_strict_upstream_jump(jump) == SetJumpOutcome::Rejected {
            log.push_err("deny jump conflict");
            return graphql_request::reject(session, 500);
        }
        log.push(if is_default { "route default" } else { "route exact" });
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/graphql_operation_router/plugin_test.rs"]
mod tests;
