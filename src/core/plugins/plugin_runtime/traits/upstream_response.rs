//! UpstreamResponse trait - for upstream response handling (async)

use async_trait::async_trait;
use crate::types::filters::PluginRunningResult;
use super::session::PluginSession;
use crate::core::plugins::plugin_runtime::log::PluginLog;

/// UpstreamResponse trait for upstream response stage plugins
/// Runs during response_filter hook (async)
#[async_trait]
pub trait UpstreamResponse: Send + Sync {
    /// Get the filter name
    fn name(&self) -> &str;

    /// Run the upstream response handler (async)
    /// This is called during the response_filter stage after upstream responds
    async fn run_upstream_response(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult;
}

