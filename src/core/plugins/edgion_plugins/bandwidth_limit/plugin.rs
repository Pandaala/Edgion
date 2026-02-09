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

use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, UpstreamResponseBodyFilter};
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
        _session: &dyn PluginSession,
        _log: &mut PluginLog,
    ) -> Option<Duration> {
        // Skip if rate is invalid or zero (misconfiguration, fail-open)
        if self.rate_bps == 0 {
            return None;
        }

        if let Some(ref data) = body {
            let chunk_size = data.len();
            if chunk_size > 0 {
                // Calculate how long this chunk should take to transmit at the configured rate
                // delay = chunk_size_bytes / rate_bytes_per_second
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
        let session = MockSession;
        let delay = plugin.run_upstream_response_body_filter(&body, false, &session, &mut log);

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

        let session = MockSession;
        let delay = plugin.run_upstream_response_body_filter(&body, false, &session, &mut log);
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_none_body() {
        let plugin = create_test_plugin("1mb");
        let body: Option<bytes::Bytes> = None;
        let mut log = PluginLog::new("test");

        let session = MockSession;
        let delay = plugin.run_upstream_response_body_filter(&body, false, &session, &mut log);
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_invalid_rate() {
        let plugin = create_test_plugin("invalid");
        let body = Some(bytes::Bytes::from(vec![0u8; 1024]));
        let mut log = PluginLog::new("test");

        let session = MockSession;
        let delay = plugin.run_upstream_response_body_filter(&body, false, &session, &mut log);
        // Invalid rate -> fail-open, no throttling
        assert!(delay.is_none());
    }

    #[test]
    fn test_bandwidth_limit_512kb() {
        let plugin = create_test_plugin("512kb");
        let body = Some(bytes::Bytes::from(vec![0u8; 1024])); // 1KB chunk
        let mut log = PluginLog::new("test");

        let session = MockSession;
        let delay = plugin.run_upstream_response_body_filter(&body, false, &session, &mut log);

        // 1KB / 512KB/s = 1/512 ≈ 0.00195s ≈ 1.95ms
        assert!(delay.is_some());
        let d = delay.unwrap();
        let micros = d.as_micros();
        assert!(micros >= 1900 && micros <= 2000, "Expected ~1953us, got {}us", micros);
    }

    // Minimal mock session for unit tests (only needs Send)
    struct MockSession;

    // We need a minimal PluginSession impl for testing
    #[async_trait::async_trait]
    impl PluginSession for MockSession {
        fn header_value(&self, _name: &str) -> Option<String> {
            None
        }
        fn request_headers(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn method(&self) -> String {
            "GET".to_string()
        }
        fn get_query_param(&self, _name: &str) -> Option<String> {
            None
        }
        fn get_cookie(&self, _name: &str) -> Option<String> {
            None
        }
        fn get_path(&self) -> &str {
            "/"
        }
        fn get_query(&self) -> Option<String> {
            None
        }
        fn get_method(&self) -> &str {
            "GET"
        }
        fn get_ctx_var(&self, _key: &str) -> Option<String> {
            None
        }
        fn set_ctx_var(&mut self, _key: &str, _value: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn remove_ctx_var(&mut self, _key: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn get_path_param(&mut self, _name: &str) -> Option<String> {
            None
        }
        async fn write_response_header(
            &mut self,
            _resp: Box<pingora_http::ResponseHeader>,
            _end_of_stream: bool,
        ) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn write_response_header_boxed<'a>(
            &'a mut self,
            _resp: Box<pingora_http::ResponseHeader>,
            _end_of_stream: bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn set_response_header(&mut self, _name: &str, _value: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn append_response_header(&mut self, _name: &str, _value: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn remove_response_header(&mut self, _name: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn get_response_header(&self, _name: &str) -> Option<String> {
            None
        }
        fn set_response_status(&mut self, _status: u16) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn set_request_header(&mut self, _name: &str, _value: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn append_request_header(&mut self, _name: &str, _value: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn remove_request_header(&mut self, _name: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn set_upstream_uri(&mut self, _uri: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn set_upstream_host(&mut self, _host: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn set_upstream_method(&mut self, _method: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        async fn write_response_body(
            &mut self,
            _body: Option<bytes::Bytes>,
            _end_of_stream: bool,
        ) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        async fn shutdown(&mut self) {}
        fn client_addr(&self) -> &str {
            "127.0.0.1"
        }
        fn remote_addr(&self) -> &str {
            "127.0.0.1"
        }
        fn set_remote_addr(&mut self, _addr: &str) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
        fn ctx(&self) -> &crate::types::EdgionHttpContext {
            unimplemented!("Not needed for bandwidth limit tests")
        }
        fn push_plugin_ref(&mut self, _key: String) {}
        fn pop_plugin_ref(&mut self) {}
        fn plugin_ref_depth(&self) -> usize {
            0
        }
        fn has_plugin_ref(&self, _key: &str) -> bool {
            false
        }
        fn push_edgion_plugins_log(&mut self, _log: crate::core::plugins::plugin_runtime::log::EdgionPluginsLog) {}
        fn start_edgion_plugins_log(
            &mut self,
            _name: String,
        ) -> crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken {
            crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken::new(0, 0)
        }
        fn push_to_edgion_plugins_log(
            &mut self,
            _token: &crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken,
            _log: crate::core::plugins::plugin_runtime::log::PluginLog,
        ) {}
        fn key_get(&self, _key: &crate::types::common::KeyGet) -> Option<String> {
            None
        }
        fn key_set(
            &mut self,
            _key: &crate::types::common::KeySet,
            _value: Option<String>,
        ) -> crate::core::plugins::plugin_runtime::traits::PluginSessionResult<()> {
            Ok(())
        }
    }
}
