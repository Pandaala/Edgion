//! UpstreamResponseBodyFilter trait - for upstream response body filtering (sync)
//!
//! This trait is called for each body chunk received from upstream.
//! Plugins can inspect the chunk and return an optional Duration to throttle
//! the downstream transmission rate (bandwidth limiting).

use std::sync::Arc;
use std::time::Duration;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;

/// UpstreamResponseBodyFilter trait for upstream response body stage plugins
/// Runs during upstream_response_body_filter hook (sync - no async allowed)
///
/// Unlike other filter traits that return PluginRunningResult, this trait returns
/// Option<Duration> to control bandwidth throttling:
/// - None: no throttling, continue normally
/// - Some(duration): delay the next chunk by this duration
///
/// When multiple plugins return Some(duration), the largest duration wins
/// (most restrictive rate limit applies).
///
/// # No per-chunk logging
///
/// This trait deliberately omits the `&mut PluginLog` argument that other filter
/// traits carry. Body filters are invoked once per upstream chunk; for a long
/// stream that is hundreds or thousands of calls per request. Surfacing those
/// to access log would explode log volume, and dropping them silently would
/// just be wasted allocation. Body filters therefore do not produce per-chunk
/// PluginLog entries. If a stage-level summary is needed in the future, it
/// should be aggregated once per request, not emitted per chunk.
pub trait UpstreamResponseBodyFilter: Send + Sync {
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

    /// Run the upstream response body filter (sync)
    ///
    /// Called for each body chunk received from upstream.
    ///
    /// # Arguments
    /// * `body` - The body chunk data (read-only reference)
    /// * `end_of_stream` - Whether this is the last chunk
    /// * `session` - Read-only session context for accessing request metadata
    ///
    /// # Returns
    /// * `None` - No throttling
    /// * `Some(duration)` - Delay next chunk delivery by this duration
    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<Duration>;
}
