//! Request-owned asynchronous upstream response-trailer companions.

use async_trait::async_trait;
use http::HeaderMap;

use super::session::PluginSession;
use crate::plugins::runtime::PluginLog;

/// Outcome of one canonical response-trailer companion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamTrailerResult {
    Continue,
    FailClose,
}

/// A request-owned companion for the upstream response trailer lifecycle.
///
/// Companions are installed by ordinary compiled plugins and run serially in
/// the response order in which those plugins install them. This is an internal
/// companion phase, not an operator-visible plugin stage.
#[async_trait]
pub trait UpstreamResponseTrailer: Send + Sync {
    fn name(&self) -> &str;

    fn alias(&self) -> Option<&str> {
        None
    }

    async fn run_upstream_response_trailer(
        &mut self,
        trailers: Option<&mut HeaderMap>,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> UpstreamTrailerResult;

    /// Settle a companion that never observed a transport terminal callback.
    fn finalize_incomplete(&mut self, _reason: &'static str) {}

    /// Settle a companion skipped after an earlier companion failed closed.
    fn on_terminal_preempted(&mut self, _reason: &'static str) {}

    /// Release ownership when an in-flight async hook is cancelled.
    fn release_inflight(&mut self) {}
}
