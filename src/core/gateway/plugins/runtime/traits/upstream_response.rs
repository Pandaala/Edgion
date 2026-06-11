//! UpstreamResponse trait - for upstream response handling (async)

use super::session::PluginSession;
use crate::core::gateway::plugins::runtime::log::PluginLog;
use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;

/// UpstreamResponse trait for upstream response stage plugins
/// Runs during response_filter hook (async)
#[async_trait]
pub trait UpstreamResponse: Send + Sync {
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

    /// Run the upstream response handler (async)
    /// This is called during the response_filter stage after upstream responds
    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult;
}
