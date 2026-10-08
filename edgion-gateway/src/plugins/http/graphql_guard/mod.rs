use crate::plugins::http::common::graphql_request;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{CompiledGraphqlGuard, GraphqlGuardConfig};
use edgion_resources::resources::edgion_plugins::BodyRequirement;

pub struct GraphqlGuard {
    compiled: CompiledGraphqlGuard,
    envelope_limit: usize,
}

impl GraphqlGuard {
    pub fn create(config: &GraphqlGuardConfig, body: Option<BodyRequirement>) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self {
            envelope_limit: graphql_request::envelope_limit(body)?,
            compiled: catch_redacted_panic(|| config.compile()).ok()?.ok()?,
        }))
    }
}

#[async_trait]
impl RequestFilter for GraphqlGuard {
    fn name(&self) -> &str {
        "GraphqlGuard"
    }
    fn requires_materialized_request_body(&self) -> bool {
        false
    }
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        match graphql_request::evaluate(session, log, self.envelope_limit, |parsed| self.compiled.check(parsed)).await {
            Ok(()) => {
                log.push("allow");
                PluginRunningResult::GoodNext
            }
            Err(reply) => reply,
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/graphql_guard/mod_test.rs"]
mod tests;
