//! RequestBodyBuffer plugin.
//!
//! The runtime body gate performs the actual capture before this filter runs.
//! This no-op filter exists to give operators an explicit, ordered request-stage
//! entry that retains the shared request-body cache for later consumers.

use async_trait::async_trait;

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;

pub struct RequestBodyBuffer;

impl RequestBodyBuffer {
    pub fn create() -> Box<dyn RequestFilter> {
        Box::new(Self)
    }
}

#[async_trait]
impl RequestFilter for RequestBodyBuffer {
    fn name(&self) -> &str {
        "RequestBodyBuffer"
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, _session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
        PluginRunningResult::GoodNext
    }
}
