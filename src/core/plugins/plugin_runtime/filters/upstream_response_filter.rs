//! UpstreamResponseFilter trait - for upstream response filtering (sync)

use crate::types::filters::PluginRunningResult;
use super::session::PluginSession;
use crate::core::plugins::plugin_runtime::log::PluginLog;

/// UpstreamResponseFilter trait for upstream response stage plugins
/// Runs during upstream_response_filter hook (sync - no async allowed)
pub trait UpstreamResponseFilter: Send + Sync {
    /// Get the filter name
    fn name(&self) -> &str;

    /// Run the upstream response filter (sync)
    /// This is called during the upstream_response_filter stage (sync context only)
    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult;
}

