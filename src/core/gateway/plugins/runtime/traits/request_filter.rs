//! RequestFilter trait - for request stage filtering (async)

use super::session::PluginSession;
use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::types::filters::PluginRunningResult;
use async_trait::async_trait;

/// RequestFilter trait for request stage plugins
/// Runs during request_filter hook (async)
#[async_trait]
pub trait RequestFilter: Send + Sync {
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

    /// Run the request filter (async)
    /// This is called during the request_filter stage before forwarding to upstream
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult;
}
