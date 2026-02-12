//! UpstreamResponseBodyFilter trait - for upstream response body filtering (sync)
//!
//! This trait is called for each body chunk received from upstream.
//! Plugins can inspect the chunk and return an optional Duration to throttle
//! the downstream transmission rate (bandwidth limiting).

use std::time::Duration;

use super::session::PluginSession;
use crate::core::plugins::plugin_runtime::log::PluginLog;

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
pub trait UpstreamResponseBodyFilter: Send + Sync {
    /// Get the filter name
    fn name(&self) -> &str;

    /// Run the upstream response body filter (sync)
    ///
    /// Called for each body chunk received from upstream.
    ///
    /// # Arguments
    /// * `body` - The body chunk data (read-only reference)
    /// * `end_of_stream` - Whether this is the last chunk
    /// * `session` - Read-only session context for accessing request metadata
    /// * `log` - Plugin execution log
    ///
    /// # Returns
    /// * `None` - No throttling
    /// * `Some(duration)` - Delay next chunk delivery by this duration
    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Option<Duration>;
}
