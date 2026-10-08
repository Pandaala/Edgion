use crate::plugins::http::common::graphql_request;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{CompiledGraphqlGuard, GraphqlGuardConfig, GraphqlGuardMode};
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use std::io::Write as _;

pub struct GraphqlGuard {
    compiled: CompiledGraphqlGuard,
    envelope_limit: usize,
    measure: bool,
    show_query_cost: bool,
}

impl GraphqlGuard {
    pub fn create(config: &GraphqlGuardConfig, body: Option<BodyRequirement>) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self {
            envelope_limit: graphql_request::envelope_limit(body)?,
            compiled: catch_redacted_panic(|| config.compile()).ok()?.ok()?,
            measure: config.mode == GraphqlGuardMode::Measure,
            show_query_cost: config.show_query_cost,
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
        match graphql_request::evaluate(session, log, self.envelope_limit, |parsed| {
            self.compiled.inspect(parsed)
        })
        .await
        {
            Ok(report) => {
                let mut buffer = [0u8; 96];
                let mut output = buffer.as_mut_slice();
                if write!(
                    output,
                    "shape depth={} aliases={} roots={} fields={}",
                    report.shape.depth, report.shape.aliases, report.shape.root_fields, report.shape.fields
                )
                .is_ok()
                {
                    let len = 96 - output.len();
                    if let Ok(entry) = std::str::from_utf8(&buffer[..len]) {
                        log.push(entry);
                    }
                }
                if let Some(cost) = report.cost {
                    graphql_request::record_cost(session, log, cost, self.show_query_cost);
                }
                if self.measure {
                    let mut buffer = [0u8; 32];
                    let mut output = buffer.as_mut_slice();
                    if write!(output, "measure exceeded={}", report.exceeded).is_ok() {
                        let len = 32 - output.len();
                        if let Ok(entry) = std::str::from_utf8(&buffer[..len]) {
                            log.push(entry);
                        }
                    }
                } else {
                    log.push("allow");
                }
                PluginRunningResult::GoodNext
            }
            Err(reply) => reply,
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/graphql_guard/mod_test.rs"]
mod tests;
