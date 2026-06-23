//! BandwidthLimit plugin implementation
//!
//! Limits downstream response bandwidth by calculating a delay for each body chunk
//! based on the configured rate (bytes per second).
//!
//! ## How it works
//!
//! Pingora calls `upstream_response_body_filter` for each body chunk received from upstream.
//! If the filter returns `Some(Duration)`, Pingora waits that duration before sending the
//! next chunk to the client. By calculating `chunk_size / rate_bps`, we achieve bandwidth
//! throttling.
//!
//! ## Example
//!
//! With rate = 1MB/s and a 64KB chunk:
//! - delay = 65536 / 1048576 = 0.0625 seconds ≈ 62.5ms per chunk
//! - Effective throughput ≈ 1MB/s
//!
//! ## Scope: per response stream, NOT per connection or global
//!
//! The plugin instance has no state across chunks — each filter invocation computes a delay
//! purely from the current chunk size and the configured rate. The instance is shared across
//! all requests routed through this plugin config, but because there is no shared token bucket
//! or wall-clock accounting, each in-flight response stream independently gets ~`rate_bps`
//! throughput:
//!
//! - **HTTP/2 multiplexing**: N concurrent streams on the same connection each receive the full
//!   `rate_bps`; the aggregate connection bandwidth can reach N × `rate_bps`.
//! - **HTTP/1.1 keep-alive**: sequential responses on the same connection each receive the full
//!   `rate_bps` in turn.
//! - **No cross-route / cross-connection / cross-pod aggregation**: a global cap requires an
//!   external mechanism (LB QoS, kernel `tc`, or a service-mesh policy).
//!
//! This is a deliberate design tradeoff (no state = no cross-request coordination cost). For
//! audits and user documentation see the "Important Notes" section of the user guide and the
//! `skills/04-review/rate-limit/bandwidth-limit-per-stream.md` decision record.

use std::time::Duration;

use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseBodyFilter};
use edgion_resources::resources::edgion_plugins::plugin_configs::BandwidthLimitConfig;

/// BandwidthLimit plugin
///
/// Controls downstream response bandwidth by introducing delays between body chunks.
pub struct BandwidthLimit {
    /// Plugin instance name (for logging)
    name: String,
    /// Rate limit in bytes per second
    rate_bps: u64,
}

impl BandwidthLimit {
    /// Create a new BandwidthLimit plugin from config
    ///
    /// Returns a boxed UpstreamResponseBodyFilter trait object.
    ///
    /// The `unwrap_or(0)` below is a defensive sentinel: a `rate_bps` of 0 short-circuits
    /// `run_upstream_response_body_filter` to `None` (throttling silently disabled). In
    /// production this branch is unreachable — `PluginRuntime::add_from_upstream_response_body_filters`
    /// calls `BandwidthLimitConfig::validation_error()` before construction, and an
    /// unparseable `rate` is rejected there with "invalid rate format..." which
    /// `seal_fail_closed` promotes into a fail-closed 500 + K8s status condition. The
    /// sentinel is only exercised by unit tests that build the plugin directly without
    /// going through the runtime validation gate.
    pub fn create(config: &BandwidthLimitConfig) -> Box<dyn UpstreamResponseBodyFilter> {
        let rate_bps = config.parse_rate().unwrap_or(0);

        Box::new(Self {
            name: format!("BandwidthLimit({})", &config.rate),
            rate_bps,
        })
    }
}

impl UpstreamResponseBodyFilter for BandwidthLimit {
    fn name(&self) -> &str {
        &self.name
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        _end_of_stream: bool,
        _session: &mut dyn PluginSession,
    ) -> Option<Duration> {
        if self.rate_bps == 0 {
            return None;
        }

        if let Some(ref data) = body {
            let chunk_size = data.len();
            if chunk_size > 0 {
                let delay_secs = chunk_size as f64 / self.rate_bps as f64;
                return Some(Duration::from_secs_f64(delay_secs));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    fn create_test_plugin(rate: &str) -> Box<dyn UpstreamResponseBodyFilter> {
        let config = BandwidthLimitConfig {
            rate: rate.to_string(),
            rate_bytes_per_second: None,
        };
        BandwidthLimit::create(&config)
    }

    #[test]
    fn test_bandwidth_limit_none_body() {
        let plugin = create_test_plugin("1mb");
        let body: Option<bytes::Bytes> = None;

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session);
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_invalid_rate() {
        let plugin = create_test_plugin("invalid");
        let body = Some(bytes::Bytes::from(vec![0u8; 1024]));

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session);
        // Invalid rate -> fail-open, no throttling
        assert!(delay.is_none());
    }
}
