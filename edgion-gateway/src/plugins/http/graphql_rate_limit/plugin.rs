use async_trait::async_trait;
#[cfg(test)]
use bytes::Bytes;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::graphql_cost::CompiledGraphqlCost;
#[cfg(test)]
use edgion_resources::graphql_request::MAX_ENVELOPE_BYTES;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, GraphqlRateLimitConfig};

use crate::plugins::http::common::graphql_request;
#[cfg(test)]
use crate::plugins::http::common::graphql_request::{EVALUATIONS, EVALUATION_TEST_LOCK, MAX_EVALUATIONS};
use crate::plugins::http::rate_limit_redis::RateLimitRedis;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

/// A cost-model snapshot at the early request-stage execution position.
pub struct GraphqlRateLimit {
    compiled: CompiledGraphqlCost,
    limiter: RateLimitRedis,
    envelope_limit: usize,
    policy_capacities: Vec<u64>,
    show_query_cost: bool,
}

impl GraphqlRateLimit {
    pub fn create(
        config: &GraphqlRateLimitConfig,
        entry_scope: &str,
        body: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        let envelope_limit = graphql_request::envelope_limit(body)?;
        let compiled = catch_redacted_panic(|| config.compile()).ok()?.ok()?;
        let rate_config = config.rate_limit_config();
        let policy_capacities = rate_config
            .quotas
            .iter()
            .map(|quota| quota.policies.iter().map(|policy| policy.rate).min())
            .collect::<Option<Vec<_>>>()?;
        Some(Box::new(Self {
            compiled,
            limiter: RateLimitRedis::new(&rate_config, entry_scope)
                .with_json_rejection_body(graphql_request::error_body(429)),
            envelope_limit,
            policy_capacities,
            show_query_cost: config.show_query_cost,
        }))
    }
}

#[async_trait]
impl RequestFilter for GraphqlRateLimit {
    fn name(&self) -> &str {
        "GraphqlRateLimit"
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let report = match graphql_request::evaluate(session, log, self.envelope_limit, |parsed| {
            self.compiled.estimate_quotas(parsed)
        })
        .await
        {
            Ok(report) => report,
            Err(reply) => return reply,
        };
        graphql_request::record_cost(session, log, report.total, self.show_query_cost);
        let mut costs = [0u32; 16];
        costs[0] = report.total;
        costs[1..].copy_from_slice(&report.roots);
        let costs = &costs[..self.policy_capacities.len()];
        if costs
            .iter()
            .zip(&self.policy_capacities)
            .any(|(&cost, &capacity)| u64::from(cost) > capacity)
        {
            log.push("deny policy capacity");
            return graphql_request::reject(session, 429);
        }
        self.limiter.run_request_with_costs(session, log, costs).await
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/graphql_rate_limit/plugin_test.rs"]
mod tests;
