//! RateLimitRedis plugin implementation
//!
//! Redis-based precise rate limiting with cluster-wide enforcement.
//! Uses Lua scripts for atomic rate limit checks via EVALSHA with EVAL fallback.
//!
//! ## Architecture:
//! - Each plugin instance holds a `RedisRateLimiter` core with shared script descriptors
//! - Redis client is resolved at runtime via `get_redis_client()` (ArcSwap-backed, hot-swappable)
//! - All Lua scripts operate on single KEYS[1] — Redis Cluster compatible
//! - Multiple policies are evaluated independently; ALL must pass
//!
//! ## Error Handling:
//! All Redis errors (connection, timeout, OOM, auth) are handled uniformly via
//! `on_redis_failure` policy (Allow = fail-open, Deny = fail-close).

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::link_sys::get_redis_client;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::plugins::common::redis_rate_limiter::{RateLimitResult, RedisRateLimiter};
use crate::plugins::http::common::{format_duration, resolve_rate_limit_key, RateLimitKey};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{OnRedisFailure, RateLimitRedisConfig};

#[cfg(test)]
use edgion_resources::resources::edgion_plugins::{RateLimitAlgorithm, RateLimitPolicy};

fn absolute_reset_timestamp_secs(now: SystemTime, retry_after: Duration) -> Option<u64> {
    let deadline = now.checked_add(retry_after)?;
    let since_epoch = deadline.duration_since(UNIX_EPOCH).ok()?;
    since_epoch
        .as_secs()
        .checked_add(u64::from(since_epoch.subsec_nanos() > 0))
}

// ========== RateLimitRedis Plugin ==========

/// Redis-based precise rate limiting plugin.
///
/// Supports multiple rate policies, multiple algorithms (sliding window,
/// fixed window, token bucket), and true cluster-wide enforcement via Redis.
pub struct RateLimitRedis {
    config: RateLimitRedisConfig,
    config_valid: bool,
    reject_body: Bytes,
    /// `config.key_prefix` scoped by the owning entry's identity
    /// (`<namespace>/<name>/<alias or #index>`). Resolved once at build time;
    /// the request path never recomputes it.
    ///
    /// The bucket namespace has to name the *limiter*, not just the key shape:
    /// every instance defaults to the same `keyPrefix`, so without this two
    /// unrelated EdgionPlugins resources would index the same buckets — merging
    /// their quotas outright when their key sources match, and letting a client
    /// that controls one plugin's key source craft a value matching another
    /// plugin's composed key when they do not. Scoping by resource identity also
    /// mirrors the in-memory `RateLimitLocal`, whose `Rate` is shared exactly by the
    /// routes that reference one resource, and keeps cluster-wide sharing intact
    /// because every Gateway instance derives the same string from conf_sync.
    scoped_key_prefix: String,
    core: RedisRateLimiter,
}

impl RateLimitRedis {
    /// Create a new RateLimitRedis plugin from configuration.
    ///
    /// `entry_scope` is the owning EdgionPlugins entry's identity; see
    /// `scoped_key_prefix`.
    pub fn create(config: &RateLimitRedisConfig, entry_scope: &str) -> Box<dyn RequestFilter> {
        Box::new(Self::new(config, entry_scope))
    }

    fn new(config: &RateLimitRedisConfig, entry_scope: &str) -> RateLimitRedis {
        let mut validated = config.clone();
        let config_valid = validated.validate().is_ok();
        let message = validated
            .reject_message
            .take()
            .unwrap_or_else(|| "Rate limit exceeded".to_string());
        let reject_body = Bytes::from(serde_json::json!({"message": message}).to_string());
        // A valid key_prefix ends with ':' (enforced by check_fields). A config
        // that fails validation still builds a prefix here, but never reaches
        // Redis: run_request returns 500 when `config_valid` is false.
        let scoped_key_prefix = format!("{}{}:", validated.key_prefix, entry_scope);
        RateLimitRedis {
            config: validated,
            config_valid,
            reject_body,
            scoped_key_prefix,
            core: RedisRateLimiter::new(),
        }
    }

    /// Check all policies against Redis. Returns aggregate result.
    async fn check_rate_limit(&self, client: &RedisLinkClient, limit_key: &str) -> anyhow::Result<RateLimitResult> {
        self.core
            .check(
                client,
                &self.config.policies,
                &self.scoped_key_prefix,
                self.config.cost,
                self.config.max_key_len,
                limit_key,
            )
            .await
    }

    /// Add rate limit headers to response.
    fn add_headers(&self, session: &mut dyn PluginSession, remaining: i64, limit: u64) {
        if !self.config.show_limit_headers {
            return;
        }

        let remaining = remaining.max(0) as u64;

        // Note: the success path does not emit a reset header. The Lua scripts
        // return only {allowed, current, remaining, retry_ms}, so there is no
        // precise window-reset timestamp available for an allowed request.
        // A reset header is only emitted on the rejection path, where
        // `retry_after` gives a meaningful value.
        if let Some(headers) = self.config.get_header_names() {
            // Custom headers: only show what's explicitly configured
            if let Some(limit_header) = headers.limit_header() {
                let _ = session.set_response_header(limit_header, &limit.to_string());
            }
            if let Some(remaining_header) = headers.remaining_header() {
                let _ = session.set_response_header(remaining_header, &remaining.to_string());
            }
        } else {
            // Default: use X-RateLimit-* style
            let _ = session.set_response_header("X-RateLimit-Limit", &limit.to_string());
            let _ = session.set_response_header("X-RateLimit-Remaining", &remaining.to_string());
        }
    }

    /// Build a configured rejection response with rate limit headers.
    async fn reject_request(
        &self,
        session: &mut dyn PluginSession,
        retry_after: Duration,
        result: Option<&RateLimitResult>,
    ) -> PluginRunningResult {
        // Resource validation restricts reject_status to final 200..=599 values;
        // 429 remains a defensive fallback if that invariant is ever bypassed.
        let mut resp = Box::new(
            ResponseHeader::build(self.config.reject_status, None)
                .unwrap_or_else(|_| ResponseHeader::build(429, None).expect("429 is valid")),
        );
        resp.insert_header("Content-Type", "application/json").ok();
        // RFC 7231 §7.1.3: Retry-After is a delta-seconds integer; "0" tells
        // RFC-compliant clients to retry immediately and risks a hot-loop under
        // pressure. Omit the header for Duration::ZERO (Redis-failure Deny and
        // OnKeyMiss::Deny paths) and ceil any positive sub-second value to 1.
        if !retry_after.is_zero() {
            let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
            resp.insert_header("Retry-After", secs.to_string()).ok();
        }

        // Add rate limit headers (all controlled by show_limit_headers)
        if self.config.show_limit_headers {
            if let Some(r) = result {
                let reset_timestamp = absolute_reset_timestamp_secs(SystemTime::now(), retry_after);

                if let Some(headers) = self.config.get_header_names() {
                    if let Some(limit_header) = headers.limit_header() {
                        resp.insert_header(limit_header.to_string(), r.limit.to_string()).ok();
                    }
                    if let Some(remaining_header) = headers.remaining_header() {
                        resp.insert_header(remaining_header.to_string(), "0").ok();
                    }
                    if let (Some(reset_header), Some(reset_timestamp)) = (headers.reset_header(), reset_timestamp) {
                        resp.insert_header(reset_header.to_string(), reset_timestamp.to_string())
                            .ok();
                    }
                    if let Some(retry_in_header) = headers.retry_in_header() {
                        resp.insert_header(retry_in_header.to_string(), format_duration(retry_after))
                            .ok();
                    }
                } else {
                    resp.insert_header("X-RateLimit-Limit", r.limit.to_string()).ok();
                    resp.insert_header("X-RateLimit-Remaining", "0").ok();
                    if let Some(reset_timestamp) = reset_timestamp {
                        resp.insert_header("X-RateLimit-Reset", reset_timestamp.to_string())
                            .ok();
                    }
                }
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

    /// Apply the configured failure policy when Redis is unavailable or errors.
    async fn apply_redis_failure_policy(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        match self.config.on_redis_failure {
            OnRedisFailure::Allow => {
                session.record_outcome(DyeOutcome::FailOpen, "redis-unreachable");
                PluginRunningResult::GoodNext
            }
            OnRedisFailure::Deny => {
                // Record BEFORE `reject_request`'s self-write (`write_response_header`)
                // so the queued dye header op is drained by that write.
                session.record_outcome(DyeOutcome::FailClose, "redis-unreachable");
                self.reject_request(session, Duration::ZERO, None).await
            }
        }
    }
}

// ========== RequestFilter Implementation ==========

#[async_trait]
impl RequestFilter for RateLimitRedis {
    fn name(&self) -> &str {
        "RateLimitRedis"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // PluginLog must stay short (≤40B) and must never contain the resolved
        // limit_key, the redis_ref config value, or raw Redis error text — they
        // can carry user-identifying values or deployment details. Detailed
        // diagnostics go to metrics instead.

        // 1. Validate config
        if !self.config_valid {
            plugin_log.push_err("cfg load fail");
            // invariant: 500 is a compile-time-literal HTTP status code; ResponseHeader::build cannot fail.
            let mut resp = Box::new(ResponseHeader::build(500, None).expect("500 is a valid status code"));
            resp.insert_header("Content-Type", "application/json").ok();
            let body = Bytes::from(r#"{"message":"Internal rate limiter error"}"#);
            if session.write_response_header(resp, false).await.is_err() {
                return PluginRunningResult::ErrTerminateRequest;
            }
            let _ = session.write_response_body(Some(body), true).await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        // 2. Extract key (shared with RateLimitLocal via resolve_rate_limit_key)
        let limit_key = match resolve_rate_limit_key(
            session,
            &self.config.key,
            self.config.default_key.as_deref(),
            &self.config.on_key_miss,
            plugin_log,
        ) {
            RateLimitKey::Resolved(k) => k,
            RateLimitKey::Allow => return PluginRunningResult::GoodNext,
            RateLimitKey::Deny => return self.reject_request(session, Duration::ZERO, None).await,
        };

        let kl = limit_key.len();

        // 3. Get Redis client (bind once) and fast-eject if unavailable.
        let client: Arc<RedisLinkClient> = match get_redis_client(&self.config.redis_ref) {
            Some(c) => c,
            None => {
                plugin_log.push_err("redis unavail");
                return self.apply_redis_failure_policy(session).await;
            }
        };
        if !client.healthy() {
            plugin_log.push_err("redis unavail");
            return self.apply_redis_failure_policy(session).await;
        }

        // 4. Execute rate limit check (optionally bounded by request_timeout).
        let check_result = match self.config.request_timeout_duration {
            Some(timeout) => match tokio::time::timeout(timeout, self.check_rate_limit(&client, &limit_key)).await {
                Ok(r) => r,
                Err(_elapsed) => Err(anyhow::anyhow!("request_timeout elapsed")),
            },
            None => self.check_rate_limit(&client, &limit_key).await,
        };

        match check_result {
            Ok(result) if result.allowed => {
                self.add_headers(session, result.remaining, result.limit);
                plugin_log.push_fmt(
                    "rl:ok,kl={},r={}",
                    la![kl.to_string().as_str(), result.remaining.to_string().as_str()],
                );
                PluginRunningResult::GoodNext
            }
            Ok(result) => {
                let retry_after = Duration::from_millis(result.retry_ms.max(0) as u64);
                plugin_log.push_fmt(
                    "rl:lim,kl={},rms={}",
                    la![kl.to_string().as_str(), result.retry_ms.to_string().as_str()],
                );
                self.reject_request(session, retry_after, Some(&result)).await
            }
            Err(_e) => {
                plugin_log.push_err("redis rt fail");
                self.apply_redis_failure_policy(session).await
            }
        }
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::common::redis_rate_limiter::{build_redis_key, truncate_limit_key, RateLimitResult};
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::OnKeyMiss;
    use mockall::Sequence;
    use parking_lot::Mutex;

    #[test]
    fn test_truncate_limit_key_utf8_boundary() {
        // "" = 12 bytes in UTF-8 (3 bytes per char)
        let key = "";
        // Truncating at 7 bytes should not split a 3-byte char
        let truncated = truncate_limit_key(key, 7);
        assert!(truncated.len() <= 7);
        assert_eq!(truncated, ""); // 6 bytes, safe boundary
    }

    #[test]
    fn test_truncate_limit_key_over() {
        let key = "a".repeat(5000);
        assert_eq!(truncate_limit_key(&key, 4096).len(), 4096);
    }

    #[test]
    fn test_build_redis_key_sliding_window() {
        let key = build_redis_key("edgion:rl:", "sw", 0, "192.168.1.1", "");
        assert_eq!(key, "edgion:rl:sw:0:192.168.1.1");
    }

    #[test]
    fn test_rate_limit_result_from_lua_denied() {
        let result = RateLimitResult::from_lua_result(&[0, 100, 0, 500], 100).unwrap();
        assert!(!result.allowed);
        assert_eq!(result.current, 100);
        assert_eq!(result.remaining, 0);
        assert_eq!(result.retry_ms, 500);
    }

    #[test]
    fn test_merge_result_one_denied() {
        let r1 = RateLimitResult {
            allowed: true,
            current: 5,
            remaining: 95,
            limit: 100,
            retry_ms: 0,
        };
        let r2 = RateLimitResult {
            allowed: false,
            current: 1001,
            remaining: 0,
            limit: 1000,
            retry_ms: 3000,
        };
        let merged = RateLimitResult::merge(Some(r1), r2);
        assert!(!merged.allowed);
        assert_eq!(merged.remaining, 0);
        assert_eq!(merged.retry_ms, 3000);
    }

    #[test]
    fn scoped_key_prefix_namespaces_by_entry_identity() {
        let config = RateLimitRedisConfig::default();

        let a = RateLimitRedis::new(&config, "default/limiter-a/#0");
        let b = RateLimitRedis::new(&config, "default/limiter-b/#0");
        let a_second_entry = RateLimitRedis::new(&config, "default/limiter-a/#1");

        // The bucket namespace names the limiter, so an identical config in two
        // resources (or twice in one resource) never shares Redis keys.
        assert_eq!(a.scoped_key_prefix, "edgion:rl:default/limiter-a/#0:");
        assert_ne!(a.scoped_key_prefix, b.scoped_key_prefix);
        assert_ne!(a.scoped_key_prefix, a_second_entry.scoped_key_prefix);
    }

    #[test]
    fn test_format_duration_milliseconds() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
    }

    #[test]
    fn absolute_reset_timestamp_ceils_sub_second_deadline() {
        assert_eq!(
            absolute_reset_timestamp_secs(UNIX_EPOCH, Duration::from_millis(500)),
            Some(1)
        );
    }

    #[test]
    fn absolute_reset_timestamp_includes_fractional_current_second() {
        let now = UNIX_EPOCH + Duration::from_millis(1_750);
        assert_eq!(absolute_reset_timestamp_secs(now, Duration::from_millis(500)), Some(3));
    }

    #[test]
    fn absolute_reset_timestamp_preserves_exact_second_deadline() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        assert_eq!(absolute_reset_timestamp_secs(now, Duration::from_secs(2)), Some(12));
    }

    #[test]
    fn absolute_reset_timestamp_omits_pre_epoch_deadline() {
        let now = UNIX_EPOCH - Duration::from_nanos(1);
        assert_eq!(absolute_reset_timestamp_secs(now, Duration::ZERO), None);
    }

    #[test]
    fn absolute_reset_timestamp_omits_unrepresentable_deadline() {
        assert_eq!(absolute_reset_timestamp_secs(SystemTime::now(), Duration::MAX), None);
    }

    #[tokio::test]
    async fn construction_freezes_config_validity_and_invalid_runs_fail_before_dependencies() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let valid_config = unreachable_redis_config("default/rlr-valid-config-test", OnRedisFailure::Allow);
        let valid_plugin = RateLimitRedis::new(&valid_config, "default/test-plugins/#0");
        assert!(valid_plugin.config_valid);

        // Invalid redis_ref (missing namespace slash) makes the config invalid.
        let config = RateLimitRedisConfig {
            redis_ref: "invalid".to_string(),
            ..Default::default()
        };
        let plugin = RateLimitRedis::new(&config, "default/test-plugins/#0");
        assert!(!plugin.config_valid);

        let mut mock_session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        for _ in 0..2 {
            mock_session
                .expect_write_response_header()
                .withf(|response, end_of_stream| {
                    !*end_of_stream
                        && response.status.as_u16() == 500
                        && response
                            .headers
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            == Some("application/json")
                })
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_, _| Ok(()));
            mock_session
                .expect_write_response_body()
                .withf(|body, end_of_stream| {
                    *end_of_stream
                        && body.as_deref() == Some(br#"{"message":"Internal rate limiter error"}"#.as_slice())
                })
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_, _| Ok(()));
        }

        // No key/Redis/outcome expectations are installed: either dependency
        // path would panic the mock before the fixed defensive response.
        for _ in 0..2 {
            let mut plugin_log = PluginLog::new("RateLimitRedis");
            assert_eq!(
                plugin.run_request(&mut mock_session, &mut plugin_log).await,
                PluginRunningResult::ErrTerminateRequest
            );
            assert_eq!(plugin_log.render_for_test(), "err: cfg load fail");
        }
    }

    // ── Task 9: dye-suite record_outcome wiring ─────────────────────────────

    /// A validly-shaped config that is guaranteed to be `client.healthy() ==
    /// false` / `get_redis_client == None` in the unit-test process (no
    /// LinkSys runtime is ever registered under these names), so `run_request`
    /// falls through to `apply_redis_failure_policy` deterministically.
    fn unreachable_redis_config(redis_ref: &str, on_redis_failure: OnRedisFailure) -> RateLimitRedisConfig {
        RateLimitRedisConfig {
            redis_ref: redis_ref.to_string(),
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            on_redis_failure,
            ..Default::default()
        }
    }

    fn missing_key_deny_config(redis_ref: &str, reject_message: Option<&str>) -> RateLimitRedisConfig {
        let mut config = unreachable_redis_config(redis_ref, OnRedisFailure::Allow);
        config.key = vec![KeyGet::Header {
            name: "x-missing".to_string(),
        }];
        config.on_key_miss = OnKeyMiss::Deny;
        config.reject_message = reject_message.map(str::to_string);
        config
    }

    #[test]
    fn plugin_name_is_shared_static_storage() {
        let config = unreachable_redis_config("default/rlr-name-test", OnRedisFailure::Allow);
        let plugin1 = RateLimitRedis::create(&config, "default/test-plugins/#0");
        let plugin2 = RateLimitRedis::create(&config, "default/test-plugins/#1");

        assert_eq!(plugin1.name(), "RateLimitRedis");
        assert!(std::ptr::eq(plugin1.name(), plugin2.name()));
    }

    #[tokio::test]
    async fn reject_body_matches_previous_json_encoding_without_mutating_caller_config() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let message = "Too \"many\"\nrequests";
        let config = missing_key_deny_config("default/rlr-message-test", Some(message));
        let caller_snapshot = serde_json::to_value(&config).expect("config should serialize");
        let expected = Bytes::from(serde_json::json!({"message": message}).to_string());
        let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

        assert_eq!(serde_json::to_value(&config).unwrap(), caller_snapshot);
        assert_eq!(config.reject_message.as_deref(), Some(message));
        assert!(config.policies[0].interval_duration.is_none());

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

        let mut plugin_log = PluginLog::new("RateLimitRedis");
        assert_eq!(
            plugin.run_request(&mut session, &mut plugin_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(plugin_log.render_for_test(), "Deny key miss");
        assert_eq!(captured_body.lock().as_ref(), Some(&expected));
    }

    #[tokio::test]
    async fn repeated_missing_key_denials_clone_the_precomputed_body() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let config = missing_key_deny_config("default/rlr-body-clone-test", None);
        let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");
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
            let mut plugin_log = PluginLog::new("RateLimitRedis");
            assert_eq!(
                plugin.run_request(&mut session, &mut plugin_log).await,
                PluginRunningResult::ErrTerminateRequest
            );
            assert_eq!(plugin_log.render_for_test(), "Deny key miss");
        }

        let bodies = captured_bodies.lock();
        assert_eq!(bodies.len(), 2);
        assert!(!bodies[0].is_empty());
        assert_eq!(bodies[0], bodies[1]);
        assert!(std::ptr::eq(bodies[0].as_ptr(), bodies[1].as_ptr()));
    }

    #[tokio::test]
    async fn redis_unreachable_fail_open_records_outcome() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        let config = unreachable_redis_config("default/rlr-fail-open-test", OnRedisFailure::Allow);
        let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

        let mut mock_session = MockPluginSession::new();
        mock_session.expect_key_get().return_const(Some("1.2.3.4".to_string()));
        mock_session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "redis-unreachable")
            .times(1)
            .return_const(());

        let mut plugin_log = PluginLog::new("RateLimitRedis");
        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn redis_unreachable_fail_close_records_outcome_before_response_write() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use mockall::Sequence;

        let config = unreachable_redis_config("default/rlr-fail-close-test", OnRedisFailure::Deny);
        let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

        let mut mock_session = MockPluginSession::new();
        mock_session.expect_key_get().return_const(Some("1.2.3.4".to_string()));

        // Ordering is the whole point of Task 9/10: the dye record must
        // precede the plugin's own `write_response_header` self-write, or
        // the queued header op never reaches the response.
        let mut seq = Sequence::new();
        mock_session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "redis-unreachable")
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());
        mock_session
            .expect_write_response_header()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let mut plugin_log = PluginLog::new("RateLimitRedis");
        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    /// A normal rate-limit rejection (Redis reachable, over-limit) is a policy
    /// decision, not a dependency failure — it must NOT call `record_outcome`.
    /// Any call panics the mock (no `expect_record_outcome` installed).
    #[tokio::test]
    async fn normal_rate_limit_rejection_does_not_record_outcome() {
        use crate::plugins::runtime::traits::session::MockPluginSession;

        // redis_ref left invalid on purpose is not usable here (that's the
        // cfg-load-fail path, also un-recorded but for a different reason);
        // instead we exercise the `RateLimitKey::Deny` short-circuit, which
        // never reaches Redis and is unambiguously a policy decision.
        let config = RateLimitRedisConfig {
            redis_ref: "default/rlr-deny-key-test".to_string(),
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            key: vec![edgion_resources::common::KeyGet::Header {
                name: "x-missing".to_string(),
            }],
            on_key_miss: edgion_resources::resources::edgion_plugins::OnKeyMiss::Deny,
            ..Default::default()
        };
        let plugin = RateLimitRedis::create(&config, "default/test-plugins/#0");

        let mut mock_session = MockPluginSession::new();
        mock_session.expect_key_get().return_const(None::<String>);
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));
        // No `expect_record_outcome` — any call panics.

        let mut plugin_log = PluginLog::new("RateLimitRedis");
        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }
}
