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
#[path = "../../../../tests/unit/plugins/http/rate_limit_redis/plugin_test.rs"]
mod tests;
