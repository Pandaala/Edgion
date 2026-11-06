//! UpstreamResponseFilter trait - for upstream response filtering (sync)

use std::sync::Arc;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::PluginLog;
use edgion_resources::filters::PluginRunningResult;

/// Synchronous final-response-header processing in Pingora's
/// `upstream_response_filter` hook.
///
/// This stage runs only for the final, non-retried response after retry and
/// status arbitration. Interim 100, 102, and 103 responses are skipped; the
/// terminal 101 response runs normally. It precedes async [`super::UpstreamResponse`]
/// plugins, downstream header commit, and response body forwarding.
///
/// Implementations may mutate the current response headers through the session,
/// but own no response-body bytes. They cannot await and must perform no
/// blocking I/O; a plugin that needs I/O belongs in the async
/// [`super::UpstreamResponse`] stage. Never bridge with `block_in_place` /
/// `Handle::block_on`: Pingora runs current-thread runtimes when
/// `work_stealing` is disabled.
///
/// The result either continues the chain or terminates it. `ErrResponse` leaves
/// bounded local-reply ownership to the framework; `ErrTerminateRequest` is valid
/// only after the implementation has already written the response, never as a
/// naked termination signal.
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

    /// Suppress the generic stage log only when a canonical dedicated top-level
    /// access-log block owns the same outcome. Conditional wrappers delegate
    /// this value to their wrapped implementation; ordinary plugins return
    /// `false` and aggregate one plugin log entry per stage, never tracing.
    fn suppress_automatic_log(&self) -> bool {
        false
    }

    /// Run the synchronous final-response-header filter.
    fn run_upstream_response_filter(&self, session: &mut dyn PluginSession, log: &mut PluginLog)
        -> PluginRunningResult;
}
