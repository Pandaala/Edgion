//! UpstreamResponseFilter trait - for upstream response filtering (sync)

use std::sync::Arc;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::PluginLog;
use edgion_resources::filters::PluginRunningResult;

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

    /// Optional entry-level compiled dye plan.
    ///
    /// Default returns `None`; the conditional wrappers in
    /// `conditional_filter.rs` are the only impls that override this to
    /// surface the plan compiled from the EdgionPlugins entry's `dye` field.
    /// Plain plugin impls do not need to implement this.
    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        None
    }

    /// Plugins with a dedicated top-level access-log block may suppress the
    /// generic per-stage entry.
    fn suppress_automatic_log(&self) -> bool {
        false
    }

    /// Run the upstream response filter (sync)
    /// This is called during the upstream_response_filter stage (sync context only)
    fn run_upstream_response_filter(&self, session: &mut dyn PluginSession, log: &mut PluginLog)
        -> PluginRunningResult;
}
