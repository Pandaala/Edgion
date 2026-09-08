//! UpstreamResponseBodyFilter trait - for upstream response body filtering (sync)
//!
//! This read-only, synchronous trait inspects the current upstream response
//! chunk and may return a delay for bandwidth limiting.

use std::sync::Arc;
use std::time::Duration;

use super::session::PluginSession;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;

/// Read-only plugins for Pingora's upstream response-body callback.
///
/// Callbacks cover ordinary data tasks and the `body = None`, end-of-stream
/// terminal-without-trailers task used by header-only responses. They do not
/// run for `TerminalBeforeTrailers`. Here, end of stream means that the response
/// closes without trailers.
///
/// The body is an immutable view of only the current chunk or window. This
/// stage cannot aggregate or mutate the response body and cannot deny the
/// response. Its `Option<Duration>` result controls bandwidth throttling:
/// `None` forwards normally, while `Some(duration)` delays forwarding or
/// completion of the current response task. When multiple plugins return a
/// duration, the largest one wins.
///
/// The mutable session exists for request-scoped metadata and framework state;
/// it does not grant response-body mutation or complete-response access.
///
/// # Synchronous hot-path contract
///
/// Implementations must not sleep, block, allocate per callback, log, or trace.
/// Any retained aggregation must be bounded request-scoped state and emitted
/// once through the canonical access log outside this callback.
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

    /// Run the synchronous upstream response-body filter.
    ///
    /// A returned duration delays forwarding or completion of this response
    /// task; it does not authorize the implementation to wait synchronously.
    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<Duration>;
}
