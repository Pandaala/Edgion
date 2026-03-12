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

use std::time::Duration;
use tracing;

use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{PluginSession, UpstreamResponseBodyFilter};
use crate::types::resources::edgion_plugins::plugin_configs::BandwidthLimitConfig;

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
    /// If the rate cannot be parsed, creates a no-op instance (rate_bps = 0) that logs an error.
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
        log: &mut PluginLog,
    ) -> Option<Duration> {
        if self.rate_bps == 0 {
            tracing::warn!(plugin = "BandwidthLimit", "rate_bps is 0 — skipping (misconfiguration)");
            return None;
        }

        if let Some(ref data) = body {
            let chunk_size = data.len();
            if chunk_size > 0 {
                let delay_secs = chunk_size as f64 / self.rate_bps as f64;
                log.push(&format!(
                    "Throttled (chunk={}B, rate={}B/s, delay={:.4}s); ",
                    chunk_size, self.rate_bps, delay_secs
                ));
                return Some(Duration::from_secs_f64(delay_secs));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;

    fn create_test_plugin(rate: &str) -> Box<dyn UpstreamResponseBodyFilter> {
        let config = BandwidthLimitConfig {
            rate: rate.to_string(),
            rate_bytes_per_second: None,
        };
        BandwidthLimit::create(&config)
    }

    #[test]
    fn test_bandwidth_limit_1mb() {
        let plugin = create_test_plugin("1mb");
        let body = Some(bytes::Bytes::from(vec![0u8; 65536])); // 64KB chunk
        let mut log = PluginLog::new("test");

        // Create a minimal mock session
        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session, &mut log);

        // 64KB / 1MB/s = 0.0625s = 62.5ms
        assert!(delay.is_some());
        let d = delay.unwrap();
        assert!(d.as_millis() >= 62 && d.as_millis() <= 63);
    }

    #[test]
    fn test_bandwidth_limit_empty_body() {
        let plugin = create_test_plugin("1mb");
        let body: Option<bytes::Bytes> = Some(bytes::Bytes::new());
        let mut log = PluginLog::new("test");

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session, &mut log);
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_none_body() {
        let plugin = create_test_plugin("1mb");
        let body: Option<bytes::Bytes> = None;
        let mut log = PluginLog::new("test");

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session, &mut log);
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_invalid_rate() {
        let plugin = create_test_plugin("invalid");
        let body = Some(bytes::Bytes::from(vec![0u8; 1024]));
        let mut log = PluginLog::new("test");

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session, &mut log);
        // Invalid rate -> fail-open, no throttling
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_512kb() {
        let plugin = create_test_plugin("512kb");
        let body = Some(bytes::Bytes::from(vec![0u8; 1024])); // 1KB chunk
        let mut log = PluginLog::new("test");

        let mut session = MockPluginSession::new();
        let delay = plugin.run_upstream_response_body_filter(&body, false, &mut session, &mut log);

        // 1KB / 512KB/s = 1/512 ≈ 0.00195s ≈ 1.95ms
        assert!(delay.is_some());
        let d = delay.unwrap();
        let micros = d.as_micros();
        assert!((1900..=2000).contains(&micros), "Expected ~1953us, got {}us", micros);
    }
}
