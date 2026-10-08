//! Schema-aware GraphQL query cost and canonical Redis quota configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::KeyGet;
use crate::graphql_cost::CompiledGraphqlCost;
use crate::graphql_request::MAX_ENVELOPE_BYTES;
use crate::resources::edgion_plugins::{BodyRequirement, PluginValidation};

use super::json_schema_validation::bounded_structured_body_validation_errors;
use super::rate_limit_local::OnKeyMiss;
use super::rate_limit_redis::{OnRedisFailure, RateLimitPolicy, RateLimitRedisConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlCostRule {
    pub field: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub multipliers: Vec<String>,
    #[serde(default = "default_multiplier")]
    pub default_multiplier: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlRateLimitConfig {
    pub schema: String,
    #[serde(default)]
    pub cost_rules: Vec<GraphqlCostRule>,
    #[serde(default = "default_max_cost")]
    pub max_cost: u32,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    pub redis_ref: String,
    #[serde(default = "default_keys")]
    pub key: Vec<KeyGet>,
    pub policies: Vec<RateLimitPolicy>,
    #[serde(default = "default_redis_failure")]
    pub on_redis_failure: OnRedisFailure,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: String,
}

fn default_weight() -> u32 {
    1
}
fn default_multiplier() -> u32 {
    100
}
fn default_max_cost() -> u32 {
    10_000
}
fn default_max_depth() -> usize {
    20
}
fn default_keys() -> Vec<KeyGet> {
    vec![KeyGet::RemoteIp]
}
fn default_redis_failure() -> OnRedisFailure {
    OnRedisFailure::Deny
}
fn default_request_timeout() -> String {
    "200ms".to_string()
}

impl GraphqlRateLimitConfig {
    pub fn compile(&self) -> Result<CompiledGraphqlCost, String> {
        self.rate_limit_config().validate()?;
        CompiledGraphqlCost::compile(&self.schema, &self.cost_rules, self.max_cost, self.max_depth)
    }

    pub fn rate_limit_config(&self) -> RateLimitRedisConfig {
        RateLimitRedisConfig {
            redis_ref: self.redis_ref.clone(),
            policies: self.policies.clone(),
            key: self.key.clone(),
            on_key_miss: OnKeyMiss::Deny,
            default_key: None,
            reject_status: 429,
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            key_prefix: "edgion:graphql:".to_string(),
            on_redis_failure: self.on_redis_failure.clone(),
            cost: 1,
            max_key_len: 4096,
            request_timeout: Some(self.request_timeout.clone()),
            request_timeout_duration: None,
        }
    }
}

impl PluginValidation for GraphqlRateLimitConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile().err()
    }
}

pub fn graphql_body_validation_errors(body: &BodyRequirement) -> Vec<String> {
    let mut errors = bounded_structured_body_validation_errors(body);
    if body
        .max_body_size_bytes()
        .and_then(Result::ok)
        .is_some_and(|n| n > MAX_ENVELOPE_BYTES as u64)
    {
        errors.push("body.maxBodySize must not exceed 64KiB".to_string());
    }
    errors
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/graphql_rate_limit_test.rs"]
mod tests;
