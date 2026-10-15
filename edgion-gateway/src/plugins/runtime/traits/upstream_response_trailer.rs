//! Request-owned asynchronous upstream response-trailer companions.

use async_trait::async_trait;
use http::HeaderMap;

use super::response_stream_session::ResponseTrailerSession;
use crate::plugins::runtime::PluginLog;

/// Outcome of one canonical response-trailer companion.
///
/// `Continue` advances to the next companion. `FailClose` stops the chain and
/// aborts before trailers are forwarded, but headers or a partial body may
/// already be on the wire, so it cannot promise a new local error response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamTrailerResult {
    Continue,
    FailClose,
}

/// A request-owned companion for the upstream response trailer lifecycle.
///
/// Companions are installed during final-header processing. After processor
/// freeze, the request-owned driver awaits them serially, preserving plugin
/// response-layer order and transport backpressure. A request-local lifecycle
/// claim admits exactly one terminal path: body `TerminalBeforeTrailers`
/// precedes a real trailer callback, while a no-trailers terminal dispatches
/// the confirmed-absent path directly.
///
/// `Some(trailers)` is the transport-delivered map and may be mutated in place;
/// implementations must still validate fields they consume or forward. `None`
/// confirms a normal response with no trailers; it is not an empty map. H1
/// framing and trailer-name representability remain governed by existing
/// transport/session checks. Companions must not aggregate the body or change
/// its length.
///
/// The dispatcher records one `PluginLog` entry for every invoked companion in
/// the canonical `upstream_response_trailers` access-log stage. Companions not
/// invoked after fail-close have no stage log. Implementations must not use
/// data-plane tracing or write sensitive trailer values to logs or observability.
#[async_trait]
pub trait UpstreamResponseTrailer: Send + Sync {
    fn name(&self) -> &str;

    fn alias(&self) -> Option<&str> {
        None
    }

    async fn run_upstream_response_trailer(
        &mut self,
        trailers: Option<&mut HeaderMap>,
        session: &mut dyn ResponseTrailerSession,
        log: &mut PluginLog,
    ) -> UpstreamTrailerResult;

    /// Settle a companion that never observed a transport terminal callback.
    /// Must be synchronous, idempotent, and safe after prior completion.
    fn finalize_incomplete(&mut self, _reason: &'static str) {}

    /// Settle a companion skipped after an earlier companion failed closed.
    /// Must be synchronous, idempotent, and safe after prior completion.
    fn on_terminal_preempted(&mut self, _reason: &'static str) {}

    /// Release ownership when an in-flight async hook is cancelled. Must be
    /// synchronous, idempotent, and safe after prior completion.
    fn release_inflight(&mut self) {}
}
