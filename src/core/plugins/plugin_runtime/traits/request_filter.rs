//! RequestFilter trait - for request stage filtering (async)

use super::session::PluginSession;
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::types::filters::PluginRunningResult;
use async_trait::async_trait;

/// RequestFilter trait for request stage plugins
/// Runs during request_filter hook (async)
#[async_trait]
pub trait RequestFilter: Send + Sync {
    /// Get the filter name
    fn name(&self) -> &str;

    /// Run the request filter (async)
    /// This is called during the request_filter stage before forwarding to upstream
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult;
}
