//! RateLimit plugin implementation
//!
//! Rate limiting using Pingora's Count-Min Sketch (CMS) algorithm.
//! Provides high-performance, memory-efficient rate limiting for high-concurrency scenarios.
//!
//! ## Features:
//! - Count-Min Sketch algorithm with dual-slot sliding window
//! - Fixed memory footprint regardless of key cardinality
//! - Multiple key sources for rate limiting dimension
//! - Rate limit response headers
//!
//! ## Architecture:
//! Each RateLimit plugin instance owns its Rate instance (lazily initialized).
//! Rate instances are NOT shared between different plugins.
//!
//! ## Configuration Examples:
//!
//! ### Basic rate limiting by IP:
//! ```yaml
//! rateLimit:
//!   rate: 100              # 100 requests per interval
//!   interval: "1s"         # 1 second window
//!   key:
//!     source: ClientIP
//! ```
//!
//! ### Rate limiting by API key:
//! ```yaml
//! rateLimit:
//!   rate: 1000
//!   interval: "1m"         # 1000 requests per minute
//!   key:
//!     source: Header
//!     name: "X-API-Key"
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_limits::rate::Rate;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};

use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{OnMissingKey, RateLimitConfig};

// CMS estimator configuration
// HASHES is fixed at 4 (4 hash functions for CMS)
const CMS_HASHES: usize = 4;

// ========== RateLimit Plugin ==========

/// RateLimit plugin using Pingora's Count-Min Sketch algorithm
///
/// Each plugin instance owns its own Rate instance, which is lazily initialized
/// on the first request. The Rate instance is NOT shared between different plugins.
pub struct RateLimit {
    name: String,
    config: RateLimitConfig,
    /// The Rate instance, lazily initialized on first use
    rate: OnceLock<Rate>,
}

impl RateLimit {
    /// Create a new RateLimit plugin from configuration
    ///
    /// # Arguments
    /// * `config` - The rate limiter configuration
    ///
    /// The Rate instance is lazily initialized on the first request,
    /// using the configured interval and estimator slots.
    ///
    /// Global configuration (default_estimator_slots, max_estimator_slots) is
    /// automatically loaded from the toml config file via `validate()`.
    pub fn create(config: &RateLimitConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        validated_config.validate();

        let plugin = RateLimit {
            name: "RateLimit".to_string(),
            config: validated_config,
            rate: OnceLock::new(),
        };

        Box::new(plugin)
    }

    /// Get or initialize the Rate instance
    ///
    /// The Rate is lazily initialized on the first call.
    /// Uses configurable slots (CMS precision) and fixed HASHES (4).
    fn get_rate(&self) -> &Rate {
        self.rate.get_or_init(|| {
            let interval = self.config.get_interval_duration();
            let slots = self.config.get_effective_slots();

            debug!(
                "Initializing Rate instance: interval={:?}, slots={}, hashes={}",
                interval, slots, CMS_HASHES
            );

            Rate::new_with_estimator_config(interval, CMS_HASHES, slots)
        })
    }

    /// Add rate limit headers to response
    fn add_headers(&self, session: &mut dyn PluginSession, remaining: isize, reset_after: Duration) {
        if !self.config.show_limit_headers {
            return;
        }

        let limit = self.config.get_effective_rate();
        let reset_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() + reset_after.as_secs())
            .unwrap_or(0);

        // Ensure remaining is not negative
        let remaining = remaining.max(0) as u64;

        if let Some(headers) = self.config.get_header_names() {
            // Custom headers: only show what's explicitly configured
            if let Some(limit_header) = headers.limit_header() {
                let _ = session.set_response_header(limit_header, &limit.to_string());
            }
            if let Some(remaining_header) = headers.remaining_header() {
                let _ = session.set_response_header(remaining_header, &remaining.to_string());
            }
            if let Some(reset_header) = headers.reset_header() {
                let _ = session.set_response_header(reset_header, &reset_timestamp.to_string());
            }
            if let Some(retry_in_header) = headers.retry_in_header() {
                if reset_after > Duration::ZERO {
                    let _ = session.set_response_header(retry_in_header, &format_duration(reset_after));
                }
            }
        } else {
            // Default: use X-RateLimit-* style
            let _ = session.set_response_header("X-RateLimit-Limit", &limit.to_string());
            let _ = session.set_response_header("X-RateLimit-Remaining", &remaining.to_string());
            let _ = session.set_response_header("X-RateLimit-Reset", &reset_timestamp.to_string());
        }
    }

    /// Build rejection response
    async fn reject_request(&self, session: &mut dyn PluginSession, retry_after: Duration) -> PluginRunningResult {
        let message = self.config.reject_message.as_deref().unwrap_or("Rate limit exceeded");

        let mut resp = Box::new(ResponseHeader::build(self.config.reject_status, None).unwrap());
        resp.insert_header("Content-Type", "application/json").ok();
        resp.insert_header("Retry-After", &retry_after.as_secs().to_string())
            .ok();

        // Add rate limit headers (all controlled by show_limit_headers)
        if self.config.show_limit_headers {
            let reset_timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() + retry_after.as_secs())
                .unwrap_or(0);

            let effective_limit = self.config.get_effective_rate();
            if let Some(headers) = self.config.get_header_names() {
                // Custom headers: only show what's explicitly configured
                if let Some(limit_header) = headers.limit_header() {
                    resp.insert_header(limit_header.to_string(), &effective_limit.to_string())
                        .ok();
                }
                if let Some(remaining_header) = headers.remaining_header() {
                    resp.insert_header(remaining_header.to_string(), "0").ok();
                }
                if let Some(reset_header) = headers.reset_header() {
                    resp.insert_header(reset_header.to_string(), &reset_timestamp.to_string())
                        .ok();
                }
                if let Some(retry_in_header) = headers.retry_in_header() {
                    resp.insert_header(retry_in_header.to_string(), &format_duration(retry_after))
                        .ok();
                }
            } else {
                // Default: use X-RateLimit-* style
                resp.insert_header("X-RateLimit-Limit", &effective_limit.to_string())
                    .ok();
                resp.insert_header("X-RateLimit-Remaining", "0").ok();
                resp.insert_header("X-RateLimit-Reset", &reset_timestamp.to_string())
                    .ok();
            }
        }

        let body = Bytes::from(format!(r#"{{"message":"{}"}}"#, message));

        if let Err(_e) = session.write_response_header(resp, false).await {
            return PluginRunningResult::ErrTerminateRequest;
        }

        if let Err(_e) = session.write_response_body(Some(body), true).await {
            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::ErrTerminateRequest
    }
}

/// Format duration in human-readable format (e.g., "1.5s", "500ms")
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[async_trait]
impl RequestFilter for RateLimit {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check for configuration errors
        if !self.config.is_valid() {
            let error = self.config.get_validation_error().unwrap_or("Unknown error");
            plugin_log.push(&format!("Config error: {}; ", error));
            // Allow request to proceed on config error (fail-open)
            return PluginRunningResult::GoodNext;
        }

        // Get the rate limiting key using session.key_get()
        // Multiple keys are combined with "_" separator
        let limit_key = {
            let parts: Vec<String> = self.config.key.iter().filter_map(|k| session.key_get(k)).collect();

            if parts.is_empty() {
                // No keys resolved - check if we have a default key configured
                if let Some(ref default_key) = self.config.default_key {
                    plugin_log.push(&format!("Using default key: {}; ", default_key));
                    default_key.clone()
                } else {
                    // No default key - check onMissingKey behavior
                    match self.config.on_missing_key {
                        OnMissingKey::Allow => {
                            plugin_log.push("No limit key, allowing (fail-open); ");
                            return PluginRunningResult::GoodNext;
                        }
                        OnMissingKey::Deny => {
                            plugin_log.push("No limit key, denying; ");
                            return self.reject_request(session, Duration::ZERO).await;
                        }
                    }
                }
            } else {
                parts.join("_")
            }
        };

        let interval = self.config.get_interval_duration();
        let rate_limit = self.config.get_effective_rate();

        // Get the Rate instance (lazily initialized on first access)
        let rate = self.get_rate();

        // Record the request and get current window count
        // The limit_key is passed to Rate.observe() for per-key counting within the CMS
        let curr_count = rate.observe(&limit_key, 1);

        // Calculate remaining quota
        let remaining = rate_limit - curr_count;

        // Check if rate limit exceeded
        if curr_count > rate_limit {
            plugin_log.push(&format!(
                "Rate limited (key: {}, count: {}, limit: {}, scope: {:?}); ",
                limit_key, curr_count, rate_limit, self.config.scope
            ));
            // Use interval as retry_after since the window will reset after interval
            return self.reject_request(session, interval).await;
        }

        // Add rate limit headers
        self.add_headers(session, remaining, interval);
        plugin_log.push(&format!(
            "Allowed (key: {}, count: {}, remaining: {}, scope: {:?}); ",
            limit_key,
            curr_count,
            remaining.max(0),
            self.config.scope
        ));
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::common::KeyGet;

    fn create_basic_config() -> RateLimitConfig {
        let mut config = RateLimitConfig {
            rate: 5,
            interval: "1s".to_string(),
            reject_message: Some("Too many requests".to_string()),
            ..Default::default()
        };
        config.validate();
        config
    }

    #[test]
    fn test_key_get_client_ip() {
        let mut mock_session = MockPluginSession::new();
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::ClientIp))
            .return_const(Some("192.168.1.1".to_string()));

        let key = KeyGet::ClientIp;
        assert_eq!(mock_session.key_get(&key), Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_key_get_empty_client_ip() {
        let mut mock_session = MockPluginSession::new();
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::ClientIp))
            .return_const(None);

        let key = KeyGet::ClientIp;
        assert_eq!(mock_session.key_get(&key), None);
    }

    #[test]
    fn test_key_get_header() {
        let mut mock_session = MockPluginSession::new();
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Header { name } if name == "X-API-Key"))
            .return_const(Some("api-key-123".to_string()));

        let key = KeyGet::Header {
            name: "X-API-Key".to_string(),
        };
        assert_eq!(mock_session.key_get(&key), Some("api-key-123".to_string()));
    }

    #[test]
    fn test_key_get_path() {
        let mut mock_session = MockPluginSession::new();
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Path))
            .return_const(Some("/api/users".to_string()));

        let key = KeyGet::Path;
        assert_eq!(mock_session.key_get(&key), Some("/api/users".to_string()));
    }

    #[test]
    fn test_key_get_client_ip_and_path() {
        let mut mock_session = MockPluginSession::new();
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::ClientIpAndPath))
            .return_const(Some("10.0.0.1:/api/data".to_string()));

        let key = KeyGet::ClientIpAndPath;
        assert_eq!(mock_session.key_get(&key), Some("10.0.0.1:/api/data".to_string()));
    }

    #[tokio::test]
    async fn test_rate_limit_allows_within_limit() {
        let config = create_basic_config();
        let plugin = RateLimit::create(&config);

        for i in 0..5 {
            let mut mock_session = MockPluginSession::new();
            let mut plugin_log = PluginLog::new("RateLimit");

            let ip = format!("test_basic_ip_{}", i);
            mock_session.expect_key_get().return_const(Some(ip)); // Each request different IP to test independent
            mock_session.expect_set_response_header().returning(|_, _| Ok(()));

            let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
            assert_eq!(result, PluginRunningResult::GoodNext, "Request {} should be allowed", i);
            assert!(plugin_log.contains("Allowed"));
        }
    }

    #[tokio::test]
    async fn test_rate_limit_rejects_over_limit() {
        let config = create_basic_config();
        let plugin = RateLimit::create(&config);
        let test_ip = "test_over_ip_unique";

        // Consume all quota (5 requests from same IP)
        for _ in 0..5 {
            let mut mock_session = MockPluginSession::new();
            let mut plugin_log = PluginLog::new("RateLimit");
            mock_session.expect_key_get().return_const(Some(test_ip.to_string()));
            mock_session.expect_set_response_header().returning(|_, _| Ok(()));
            plugin.run_request(&mut mock_session, &mut plugin_log).await;
        }

        // Next request should be rejected
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        mock_session.expect_key_get().return_const(Some(test_ip.to_string()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Rate limited"));
    }

    #[tokio::test]
    async fn test_rate_limit_no_key_allows() {
        let config = create_basic_config();
        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        mock_session.expect_key_get().return_const(None);

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("No limit key"));
    }

    #[tokio::test]
    async fn test_rate_limit_header_key() {
        let mut config = RateLimitConfig {
            rate: 100,
            interval: "1s".to_string(),
            key: vec![KeyGet::Header {
                name: "X-API-Key".to_string(),
            }],
            ..Default::default()
        };
        config.validate();

        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        mock_session
            .expect_key_get()
            .return_const(Some("test-key-123".to_string()));
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_rate_limit_composite_keys() {
        // Test multiple keys combined with "_"
        let mut config = RateLimitConfig {
            rate: 100,
            interval: "1s".to_string(),
            key: vec![
                KeyGet::ClientIp,
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
                KeyGet::Path,
            ],
            ..Default::default()
        };
        config.validate();

        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        // Mock key_get to return values for each key type
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::ClientIp))
            .return_const(Some("192.168.1.1".to_string()));
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Header { name } if name == "X-API-Key"))
            .return_const(Some("api-key-123".to_string()));
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Path))
            .return_const(Some("/api/users".to_string()));
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        // The combined key should be "192.168.1.1_api-key-123_/api/users"
    }

    #[tokio::test]
    async fn test_rate_limit_partial_keys() {
        // Test that partial keys work (some keys missing)
        let mut config = RateLimitConfig {
            rate: 100,
            interval: "1s".to_string(),
            key: vec![
                KeyGet::ClientIp,
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
            ],
            ..Default::default()
        };
        config.validate();

        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        // Only ClientIp available, Header missing
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::ClientIp))
            .return_const(Some("192.168.1.1".to_string()));
        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Header { .. }))
            .return_const(None);
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        // Should use just "192.168.1.1" as the key
    }

    #[tokio::test]
    async fn test_config_validation_error() {
        let mut config = RateLimitConfig {
            rate: 0, // Invalid rate
            ..Default::default()
        };
        config.validate();

        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Config error"));
    }

    #[tokio::test]
    async fn test_separate_plugin_instances() {
        // Test that different plugin instances have separate rate limiting state
        let config = create_basic_config();
        let plugin1 = RateLimit::create(&config);
        let plugin2 = RateLimit::create(&config);

        let shared_ip = "shared_ip_test";

        // Exhaust plugin1's limit
        for _ in 0..5 {
            let mut mock_session = MockPluginSession::new();
            let mut plugin_log = PluginLog::new("RateLimit");
            mock_session.expect_key_get().return_const(Some(shared_ip.to_string()));
            mock_session.expect_set_response_header().returning(|_, _| Ok(()));
            plugin1.run_request(&mut mock_session, &mut plugin_log).await;
        }

        // plugin2 should still allow (separate Rate instance)
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");
        mock_session.expect_key_get().return_const(Some(shared_ip.to_string()));
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = plugin2.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_custom_estimator_slots() {
        let mut config = RateLimitConfig {
            rate: 10,
            interval: "1s".to_string(),
            estimator_slots_k: Some(16), // 16K = 16384 slots
            ..Default::default()
        };
        config.validate();

        let plugin = RateLimit::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RateLimit");
        mock_session
            .expect_key_get()
            .return_const(Some("test_slots_ip".to_string()));
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
