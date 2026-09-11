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

use crate::plugins::http::common::{format_duration, resolve_rate_limit_key, RateLimitKey};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::RateLimitConfig;

// Fixed at 4 hash functions for the CMS estimator.
const CMS_HASHES: usize = 4;

// ========== RateLimit Plugin ==========

/// RateLimit plugin using Pingora's Count-Min Sketch algorithm
///
/// Each plugin instance owns its own Rate instance, which is lazily initialized
/// on the first request. The Rate instance is NOT shared between different plugins.
pub struct RateLimit {
    outcome_slot: Option<usize>,
    config: RateLimitConfig,
    reject_body: Bytes,
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
    /// automatically loaded from the YAML config file via `validate()`.
    pub fn create(config: &RateLimitConfig) -> Box<dyn RequestFilter> {
        Self::create_with_outcome_slot(config, None)
    }

    pub(crate) fn create_with_outcome_slot(
        config: &RateLimitConfig,
        outcome_slot: Option<usize>,
    ) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate();
        let message = validated_config
            .reject_message
            .take()
            .unwrap_or_else(|| "Rate limit exceeded".to_string());
        let reject_body = Bytes::from(serde_json::json!({"message": message}).to_string());

        let plugin = RateLimit {
            outcome_slot,
            config: validated_config,
            reject_body,
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
        // invariant: 429 is a compile-time-literal HTTP status code; ResponseHeader::build cannot fail.
        let mut resp = Box::new(
            ResponseHeader::build(self.config.reject_status, None)
                .unwrap_or_else(|_| ResponseHeader::build(429, None).expect("429 is valid")),
        );
        resp.insert_header("Content-Type", "application/json").ok();
        // RFC 7231 §7.1.3: Retry-After is a delta-seconds integer; "0" tells
        // RFC-compliant clients to retry immediately and risks a hot-loop under
        // pressure. Omit the header for Duration::ZERO (OnKeyMiss::Deny path)
        // and ceil any positive sub-second value to 1.
        if !retry_after.is_zero() {
            let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
            resp.insert_header("Retry-After", secs.to_string()).ok();
        }

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
                    resp.insert_header(limit_header.to_string(), effective_limit.to_string())
                        .ok();
                }
                if let Some(remaining_header) = headers.remaining_header() {
                    resp.insert_header(remaining_header.to_string(), "0").ok();
                }
                if let Some(reset_header) = headers.reset_header() {
                    resp.insert_header(reset_header.to_string(), reset_timestamp.to_string())
                        .ok();
                }
                if let Some(retry_in_header) = headers.retry_in_header() {
                    resp.insert_header(retry_in_header.to_string(), format_duration(retry_after))
                        .ok();
                }
            } else {
                // Default: use X-RateLimit-* style
                resp.insert_header("X-RateLimit-Limit", effective_limit.to_string())
                    .ok();
                resp.insert_header("X-RateLimit-Remaining", "0").ok();
                resp.insert_header("X-RateLimit-Reset", reset_timestamp.to_string())
                    .ok();
            }
        }

        if let Err(_e) = session.write_response_header(resp, false).await {
            return PluginRunningResult::ErrTerminateRequest;
        }

        if let Err(_e) = session.write_response_body(Some(self.reject_body.clone()), true).await {
            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for RateLimit {
    fn name(&self) -> &str {
        "RateLimit"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // PluginLog must stay short (≤40B) and must never contain the resolved
        // limit_key — it can be a user-identifying value (API key, user_id).
        // Key extraction is shared with RateLimitRedis via resolve_rate_limit_key.
        let limit_key = match resolve_rate_limit_key(
            session,
            &self.config.key,
            self.config.default_key.as_deref(),
            &self.config.on_key_miss,
            plugin_log,
        ) {
            RateLimitKey::Resolved(k) => k,
            RateLimitKey::Allow => return PluginRunningResult::GoodNext,
            RateLimitKey::Deny => return self.reject_request(session, Duration::ZERO).await,
        };

        let interval = self.config.get_interval_duration();
        let rate_limit = self.config.get_effective_rate();

        let rate = self.get_rate();

        // The limit_key is passed to Rate.observe() for per-key counting within the CMS.
        let curr_count = rate.observe(&limit_key, 1);

        let exceeded = curr_count > rate_limit;
        if let Some(slot) = self.outcome_slot {
            use crate::plugins::runtime::conditions::rate_limit_outcome::RateLimitOutcome;
            let outcome = if exceeded {
                RateLimitOutcome::Exceeded
            } else {
                RateLimitOutcome::WithinLimit
            };
            if !session.set_rate_limit_outcome(slot, outcome) {
                plugin_log.push_err("local outcome unavailable");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
        }

        if exceeded
            && self.config.on_limit_exceeded
                == edgion_resources::resources::edgion_plugins::plugin_configs::OnLimitExceeded::Continue
        {
            plugin_log.push("Continue local overflow");
            return PluginRunningResult::GoodNext;
        }

        let remaining = rate_limit - curr_count;

        let kl = limit_key.len();

        if curr_count > rate_limit {
            plugin_log.push_fmt("Deny rate limit kl={}", la![kl.to_string().as_str()]);
            // Use interval as retry_after since the window resets after interval.
            return self.reject_request(session, interval).await;
        }

        self.add_headers(session, remaining, interval);
        plugin_log.push_fmt(
            "Allow kl={} r={}",
            la![kl.to_string().as_str(), remaining.max(0).to_string().as_str()],
        );
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::OnKeyMiss;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn create_basic_config() -> RateLimitConfig {
        let mut config = RateLimitConfig {
            rate: 5,
            interval: "1s".to_string(),
            reject_message: Some("Too many requests".to_string()),
            ..Default::default()
        };
        let _ = config.validate();
        config
    }

    #[test]
    fn plugin_name_is_shared_static_storage() {
        let config = create_basic_config();
        let plugin1 = RateLimit::create(&config);
        let plugin2 = RateLimit::create(&config);

        assert_eq!(plugin1.name(), "RateLimit");
        assert!(std::ptr::eq(plugin1.name(), plugin2.name()));
    }

    #[tokio::test]
    async fn reject_body_matches_previous_json_encoding_without_mutating_caller_config() {
        let message = "Too \"many\"\nrequests";
        let config = RateLimitConfig {
            reject_message: Some(message.to_string()),
            on_key_miss: OnKeyMiss::Deny,
            ..Default::default()
        };
        let caller_snapshot = serde_json::to_value(&config).expect("config should serialize");
        let expected = Bytes::from(serde_json::json!({"message": message}).to_string());
        let plugin = RateLimit::create(&config);

        assert_eq!(serde_json::to_value(&config).unwrap(), caller_snapshot);

        let captured_body = Arc::new(Mutex::new(None));
        let captured_body_for_mock = Arc::clone(&captured_body);
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(None::<String>);
        session.expect_write_response_header().times(1).returning(|_, end| {
            assert!(!end);
            Ok(())
        });
        session
            .expect_write_response_body()
            .times(1)
            .returning(move |body, end| {
                assert!(end);
                *captured_body_for_mock.lock() = body;
                Ok(())
            });

        let mut plugin_log = PluginLog::new("RateLimit");
        assert_eq!(
            plugin.run_request(&mut session, &mut plugin_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(captured_body.lock().as_ref(), Some(&expected));
    }

    #[tokio::test]
    async fn repeated_missing_key_denials_clone_the_precomputed_body() {
        let config = RateLimitConfig {
            on_key_miss: OnKeyMiss::Deny,
            ..Default::default()
        };
        let plugin = RateLimit::create(&config);
        let captured_bodies = Arc::new(Mutex::new(Vec::new()));
        let captured_bodies_for_mock = Arc::clone(&captured_bodies);
        let mut session = MockPluginSession::new();
        session.expect_key_get().times(2).return_const(None::<String>);
        session.expect_write_response_header().times(2).returning(|_, end| {
            assert!(!end);
            Ok(())
        });
        session
            .expect_write_response_body()
            .times(2)
            .returning(move |body, end| {
                assert!(end);
                captured_bodies_for_mock
                    .lock()
                    .push(body.expect("rejection must have a body"));
                Ok(())
            });

        for _ in 0..2 {
            let mut plugin_log = PluginLog::new("RateLimit");
            assert_eq!(
                plugin.run_request(&mut session, &mut plugin_log).await,
                PluginRunningResult::ErrTerminateRequest
            );
        }

        let bodies = captured_bodies.lock();
        assert_eq!(bodies.len(), 2);
        assert!(!bodies[0].is_empty());
        assert_eq!(bodies[0], bodies[1]);
        assert!(std::ptr::eq(bodies[0].as_ptr(), bodies[1].as_ptr()));
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
        assert!(plugin_log.contains("Allow kl="));
    }
}
