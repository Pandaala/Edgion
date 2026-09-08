//! UpstreamResponse trait - for upstream response handling (async)

use std::sync::Arc;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::PluginLog;
use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;

/// Async final-response-header processing in Pingora's `response_filter` hook.
///
/// The hook runs for the final response candidate after synchronous response
/// filtering, retry arbitration, and response normalization. Interim 1xx
/// responses are skipped. It completes before the downstream header is
/// committed and before response body or terminal events are forwarded.
///
/// Implementations may await, mutate the current response headers through the
/// session, install bounded streaming companions, and return a
/// [`PluginRunningResult`] to continue or terminate processing. This stage does
/// not provide access to a complete response or a complete response body; the
/// main upstream response remains streaming.
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

    /// Optional entry-level compiled dye plan.
    ///
    /// Default returns `None`; the conditional wrappers in
    /// `conditional_filter.rs` are the only impls that override this to
    /// surface the plan compiled from the EdgionPlugins entry's `dye` field.
    /// Plain plugin impls do not need to implement this.
    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        None
    }

    /// Internal companions may attach their outcome to the request-stage owner
    /// and suppress a second automatic stage log.
    fn suppress_automatic_log(&self) -> bool {
        false
    }

    /// Run the async final-response-header handler.
    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult;
}
