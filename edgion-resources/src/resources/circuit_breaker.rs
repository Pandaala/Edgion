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
mod tests {
    use super::*;
    #[test]
    fn zero_max_parallel_requests_rejected() {
        let c = CircuitBreakerConfig {
            max_parallel_requests: 0,
        };
        assert!(c.get_validation_error().is_some());
    }
    #[test]
    fn positive_max_parallel_requests_ok() {
        let c = CircuitBreakerConfig {
            max_parallel_requests: 1024,
        };
        assert!(c.get_validation_error().is_none());
    }
}
