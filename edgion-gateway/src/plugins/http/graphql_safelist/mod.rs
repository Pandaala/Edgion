use crate::plugins::http::common::graphql_request;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{CompiledGraphqlSafelist, GraphqlSafelistConfig};
use edgion_resources::resources::edgion_plugins::BodyRequirement;

pub struct GraphqlSafelist {
    compiled: CompiledGraphqlSafelist,
    envelope_limit: usize,
}

impl GraphqlSafelist {
    pub fn create(config: &GraphqlSafelistConfig, body: Option<BodyRequirement>) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self {
            envelope_limit: graphql_request::envelope_limit(body)?,
            compiled: catch_redacted_panic(|| config.compile()).ok()?.ok()?,
        }))
    }
}

#[async_trait]
impl RequestFilter for GraphqlSafelist {
    fn name(&self) -> &str {
        "GraphqlSafelist"
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
#[path = "../../../../tests/unit/plugins/http/graphql_safelist/mod_test.rs"]
mod tests;
