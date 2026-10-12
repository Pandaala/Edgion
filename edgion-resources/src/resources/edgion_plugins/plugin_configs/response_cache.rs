//! Explicit, narrow public-origin response caching policy.

use crate::common::KeyGet;
use crate::duration::parse_duration;
use crate::resources::edgion_plugins::PluginValidation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn default_ttl() -> String {
    "60s".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseCacheConfig {
    #[serde(default = "default_ttl")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub max_ttl: String,
    #[serde(default)]
    pub graphql_queries: bool,
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub vary: Vec<KeyGet>,
}

impl Default for ResponseCacheConfig {
    fn default() -> Self {
        Self {
            max_ttl: default_ttl(),
            graphql_queries: false,
            vary: Vec::new(),
        }
    }
}

impl ResponseCacheConfig {
    pub fn validate_and_init(&mut self) -> Result<Duration, String> {
        if let Some(error) = self.validation_error() {
            return Err(error);
        }
        for source in &mut self.vary {
            if let KeyGet::Header { name } = source {
                name.make_ascii_lowercase();
            }
        }
        parse_duration(&self.max_ttl).map_err(|_| "ResponseCache maxTtl is invalid".into())
    }
}

impl PluginValidation for ResponseCacheConfig {
    fn validation_error(&self) -> Option<String> {
        let Ok(ttl) = parse_duration(&self.max_ttl) else {
            return Some("ResponseCache maxTtl is invalid".into());
        };
        if ttl.is_zero() || ttl > Duration::from_secs(86400) {
            return Some("ResponseCache maxTtl must be positive and no greater than 24h".into());
        }
        if self.vary.len() > 16 {
            return Some("ResponseCache vary supports at most 16 headers".into());
        }
        for source in &self.vary {
            let KeyGet::Header { name } = source else {
                return Some("ResponseCache vary requires Header sources".into());
            };
            if !crate::gwapi_types::is_valid_header_token(name)
                || crate::response_header_policy::is_protected_response_header(name)
                || super::access_log_extern::is_blocked(super::access_log_extern::LogExtSource::Header, name)
                || ["host", "traceparent", "tracestate", "x-trace-id"]
                    .iter()
                    .any(|value| name.eq_ignore_ascii_case(value))
            {
                return Some("ResponseCache vary header is excluded or sensitive".into());
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/response_cache_test.rs"]
mod tests;
