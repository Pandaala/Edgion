//! RequestFilter trait - for request stage filtering (async)

use std::sync::Arc;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::PluginLog;
use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;

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

    /// Optional entry-level compiled dye plan.
    ///
    /// Default returns `None`; the conditional wrappers in
    /// `conditional_filter.rs` are the only impls that override this to
    /// surface the plan compiled from the EdgionPlugins entry's `dye` field.
    /// Plain plugin impls do not need to implement this.
    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        None
    }

    /// Whether the body gate must materialize the complete captured request
    /// body before this filter runs.
    ///
    /// The safe default preserves the gate's `on_read_failure` policy for
    /// storage-read failures. A filter may return `false` only when capture and
    /// length are sufficient for its own semantics.
    fn requires_materialized_request_body(&self) -> bool {
        true
    }

    /// Run the request filter (async)
    /// This is called during the request_filter stage before forwarding to upstream
    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult;
}
