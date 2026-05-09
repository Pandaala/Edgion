//! UpstreamResponseFilter trait - for upstream response filtering (sync)

use super::session::PluginSession;
use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::types::filters::PluginRunningResult;

/// UpstreamResponseFilter trait for upstream response stage plugins
/// Runs during upstream_response_filter hook (sync - no async allowed)
pub trait UpstreamResponseFilter: Send + Sync {
    /// Get the filter name
    fn name(&self) -> &str;

    /// Optional operator-defined alias.
    ///
    /// Default returns `None`; the conditional wrappers in
    /// `conditional_filter.rs` are the only impls that override this to
    /// surface the value supplied via the EdgionPlugins entry's `alias`
    /// field. Plain plugin impls do not need to implement this.
    fn alias(&self) -> Option<&str> {
        None
    }

    /// Run the upstream response filter (sync)
    /// This is called during the upstream_response_filter stage (sync context only)
    fn run_upstream_response_filter(&self, session: &mut dyn PluginSession, log: &mut PluginLog)
        -> PluginRunningResult;
}
