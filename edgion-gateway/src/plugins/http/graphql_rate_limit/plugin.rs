use async_trait::async_trait;
use bytes::Bytes;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::graphql_cost::{CompiledGraphqlCost, GraphqlCostError, MAX_ENVELOPE_BYTES};
use edgion_resources::resources::edgion_plugins::{BodyRequirement, GraphqlRateLimitConfig};
use edgion_resources::EdgionErrorCode;
use std::sync::LazyLock;
use tokio::sync::Semaphore;

use crate::plugins::http::common::{json_request_headers_are_supported, supported_identity_encoding};
use crate::plugins::http::rate_limit_redis::RateLimitRedis;
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

// The pinned parser's conservative allowance is 96MiB per evaluation. The
// canonical BodyStorage and immutable compiled schema have separate bounds.
const MAX_EVALUATIONS: usize = 4;
static EVALUATIONS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_EVALUATIONS));

/// A cost-model snapshot at the early request-stage execution position.
pub struct GraphqlRateLimit {
    compiled: CompiledGraphqlCost,
    limiter: RateLimitRedis,
    envelope_limit: usize,
    policy_capacity: u64,
}

impl GraphqlRateLimit {
    pub fn create(
        config: &GraphqlRateLimitConfig,
        entry_scope: &str,
        body: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        let envelope_limit = match body {
            Some(body) => {
                if !edgion_resources::plugin_configs::graphql_body_validation_errors(&body).is_empty() {
                    return None;
                }
                body.max_body_size_bytes()
                    .transpose()
                    .ok()?
                    .unwrap_or(MAX_ENVELOPE_BYTES as u64)
            }
            None => MAX_ENVELOPE_BYTES as u64,
        };
        let compiled = catch_redacted_panic(|| config.compile()).ok()?.ok()?;
        let rate_config = config.rate_limit_config();
        let policy_capacity = rate_config.policies.iter().map(|policy| policy.rate).min()?;
        Some(Box::new(Self {
            compiled,
            limiter: RateLimitRedis::new(&rate_config, entry_scope),
            envelope_limit: usize::try_from(envelope_limit).ok()?,
            policy_capacity,
        }))
    }

    fn reject(status: u16) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status,
            body: Some(Bytes::from_static(b"GraphQL request rejected")),
        }
    }

    fn estimate_result(
        result: Result<Result<u32, GraphqlCostError>, edgion_common::startup::RedactedPanic>,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<u32, PluginRunningResult> {
        match result {
            Ok(Ok(cost)) => Ok(cost),
            Ok(Err(GraphqlCostError::TooLarge)) => {
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
                log.push("deny envelope cap");
                Err(Self::reject(413))
            }
            Ok(Err(GraphqlCostError::MethodNotAllowed)) => {
                log.push("deny GET mutation");
                Err(Self::reject(405))
            }
            Ok(Err(GraphqlCostError::Invalid)) => {
                log.push("deny invalid query");
                Err(Self::reject(400))
            }
            Err(_) => {
                log.push_err("query evaluation panic");
                Err(Self::reject(500))
            }
        }
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
        let ctx = session.ctx();
        if !ctx.request_info.in_request_body_stage {
            log.push_err("invalid request stage");
            return Self::reject(500);
        }
        if ctx.request_info.is_grpc_request
            || ctx.is_grpc_route_matched
            || matches!(
                ctx.request_info.discover_protocol.as_deref(),
                Some("grpc-web" | "websocket")
            )
            || ctx.has_request_body_stream_handlers()
            || ctx.grpc_transcoding.is_some()
            || session.request_header_map().contains_key(http::header::UPGRADE)
        {
            log.push("deny protocol");
            return Self::reject(400);
        }

        let is_post = match session.get_method() {
            "POST" => true,
            "GET" => false,
            _ => {
                log.push("deny method");
                return Self::reject(405);
            }
        };
        if is_post && session.request_query().is_some_and(|query| !query.is_empty()) {
            log.push("deny POST query params");
            return Self::reject(400);
        }
        let known_bodyless = session.declares_body() == Some(false);
        let body_len = match session.request_body_available_len(known_bodyless) {
            Ok(len) => len,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body length unavailable");
                return Self::reject(500);
            }
        };
        if body_len > self.envelope_limit as u64
            || (!is_post
                && session
                    .request_query()
                    .is_some_and(|query| query.len() > self.envelope_limit))
        {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny envelope cap");
            return Self::reject(413);
        }
        if !is_post && (!known_bodyless || body_len != 0) {
            log.push("deny GET body");
            return Self::reject(400);
        }
        if (is_post && !json_request_headers_are_supported(session.request_header_map()))
            || (!is_post && !supported_identity_encoding(session.request_header_map()))
        {
            log.push("deny media type");
            return Self::reject(415);
        }

        // All owned envelope/JSON/CST data and the admission permit end before
        // the Redis await below. GET borrows the current URI until estimation ends.
        let cost = {
            let Ok(_permit) = EVALUATIONS.try_acquire() else {
                log.push_err("query budget exhausted");
                return Self::reject(503);
            };
            let result = if is_post {
                let body = match session.read_request_body(None).await {
                    Ok(body) => body,
                    Err(error) => {
                        record_body_read_error_status(session, &error);
                        log.push_err("body materialization failed");
                        return Self::reject(500);
                    }
                };
                catch_redacted_panic(|| self.compiled.estimate_post(&body, self.envelope_limit))
            } else {
                catch_redacted_panic(|| {
                    self.compiled
                        .estimate_get(session.request_query().unwrap_or_default(), self.envelope_limit)
                })
            };
            match Self::estimate_result(result, session, log) {
                Ok(cost) => cost,
                Err(reply) => return reply,
            }
        };
        if u64::from(cost) > self.policy_capacity {
            log.push("deny policy capacity");
            return Self::reject(429);
        }
        self.limiter.run_request_with_cost(session, log, cost).await
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/graphql_rate_limit/plugin_test.rs"]
mod tests;
