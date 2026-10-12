//! Explicit request-stage nomination; lookup belongs to the successful proxy tail.

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::ResponseCacheConfig;
use std::sync::Arc;
use std::time::Duration;

/// Opaque cold-compiled input to the narrow session nomination capability.
/// Construction and fields remain internal; it exposes no store or mutable context.
pub struct ResponseCachePolicy {
    pub(crate) max_ttl: Duration,
    pub(crate) graphql_queries: bool,
    pub(crate) vary: Vec<KeyGet>,
}

impl ResponseCachePolicy {
    pub(crate) fn compile(config: &ResponseCacheConfig) -> Result<Arc<Self>, String> {
        let mut config = config.clone();
        let max_ttl = config.validate_and_init()?;
        Ok(Arc::new(Self {
            max_ttl,
            graphql_queries: config.graphql_queries,
            vary: config.vary,
        }))
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        self.max_ttl == other.max_ttl && self.graphql_queries == other.graphql_queries
            && self.vary.len() == other.vary.len()
            && self.vary.iter().zip(&other.vary).all(|(left, right)| {
                matches!((left, right), (KeyGet::Header { name: left }, KeyGet::Header { name: right }) if left == right)
            })
    }
}

pub struct ResponseCache {
    policy: Arc<ResponseCachePolicy>,
}

impl ResponseCache {
    pub(crate) fn create(config: &ResponseCacheConfig) -> Option<Box<dyn RequestFilter>> {
        Some(Box::new(Self {
            policy: ResponseCachePolicy::compile(config).ok()?,
        }))
    }
}

#[async_trait]
impl RequestFilter for ResponseCache {
    fn name(&self) -> &str {
        "ResponseCache"
    }
    fn requires_materialized_request_body(&self) -> bool {
        false
    }
    async fn run_request(&self, session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
        let _ = session.nominate_response_cache(&self.policy);
        PluginRunningResult::GoodNext
    }
}
