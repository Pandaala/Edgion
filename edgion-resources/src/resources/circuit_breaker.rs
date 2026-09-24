//! Circuit breaker (active concurrency cap) configuration for a backend endpoint.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerConfig {
    /// Max concurrent in-flight requests to a single backend ENDPOINT (one pod).
    /// Excess requests are shed with a non-retrying 503 rather than queued.
    /// This is a PER-ENDPOINT cap: a service with N ready endpoints admits up to
    /// N * maxParallelRequests concurrently (deliberately not Envoy's per-cluster
    /// semantic — the intent is to protect each individual pod). Must be >= 1.
    pub max_parallel_requests: u32,
}

impl CircuitBreakerConfig {
    pub fn get_validation_error(&self) -> Option<String> {
        if self.max_parallel_requests == 0 {
            return Some("maxParallelRequests must be >= 1".to_string());
        }
        None
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/circuit_breaker_test.rs"]
mod tests;
