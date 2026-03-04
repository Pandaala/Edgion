//! RateLimitRedis plugin implementation
//!
//! Redis-based precise rate limiting with cluster-wide enforcement.
//! Uses Lua scripts for atomic rate limit checks via EVALSHA with EVAL fallback.
//!
//! ## Architecture:
//! - Each plugin instance holds a `tokio::sync::OnceCell<ScriptHashes>` for lazy script loading
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

use crate::core::link_sys::get_redis_client;
use crate::core::link_sys::redis::RedisLinkClient;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{OnMissingKey, OnRedisFailure, RateLimitAlgorithm, RateLimitRedisConfig};

use super::scripts::{FIXED_WINDOW_SCRIPT, SLIDING_WINDOW_SCRIPT, TOKEN_BUCKET_SCRIPT};

use fred::interfaces::LuaInterface;

// ========== Script Hashes ==========

/// Pre-loaded SHA1 hashes for Lua scripts (EVALSHA optimization).
struct ScriptHashes {
    sliding_window: String,
    fixed_window: String,
    token_bucket: String,
}

// ========== Rate Limit Result ==========

/// Result from a single policy evaluation via Lua script.
///
/// Parsed from Lua return value: {allowed(0/1), current, remaining, retry_ms}
struct RateLimitResult {
    allowed: bool,
    current: i64,
    remaining: i64,
    limit: u64,
    retry_ms: i64,
}

impl RateLimitResult {
    /// Parse Lua script return values into a structured result.
    ///
    /// Lua returns an array of 4 integers: [allowed, current, remaining, retry_ms]
    /// Missing values default to 0; `limit` comes from policy config, not Lua.
    fn from_lua_result(values: &[i64], limit: u64) -> Self {
        Self {
            allowed: values.first().copied().unwrap_or(0) == 1,
            current: values.get(1).copied().unwrap_or(0),
            remaining: values.get(2).copied().unwrap_or(0),
            limit,
            retry_ms: values.get(3).copied().unwrap_or(0),
        }
    }
}

// ========== Helper Functions ==========

/// Truncate a limit key to the configured max length.
///
/// Since we don't use hash tags, the limit_key is just a plain segment
/// in the Redis key string. Redis keys are binary-safe, so any content
/// is valid. We only truncate to prevent OOM from malicious long values.
///
/// This matches Kong/APISIX's approach: direct pass-through with no encoding.
fn truncate_limit_key(raw: &str, max_len: usize) -> &str {
    if raw.len() > max_len {
        // Truncate at char boundary to avoid splitting multi-byte UTF-8
        let mut end = max_len;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        &raw[..end]
    } else {
        raw
    }
}

/// Build Redis key. No hash tag needed — all algorithms use single KEYS.
///
/// Format: `{prefix}{algo}:{policy_idx}:{limit_key}[:{suffix}]`
fn build_redis_key(prefix: &str, algo: &str, policy_idx: usize, limit_key: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}")
    } else {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}:{suffix}")
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

// ========== RateLimitRedis Plugin ==========

/// Redis-based precise rate limiting plugin.
///
/// Supports multiple rate policies, multiple algorithms (sliding window,
/// fixed window, token bucket), and true cluster-wide enforcement via Redis.
pub struct RateLimitRedis {
    name: String,
    config: RateLimitRedisConfig,
    /// Lazily loaded script hashes (loaded on first request, async-safe).
    /// tokio::sync::OnceCell supports async init with guaranteed single-execution,
    /// unlike std::sync::OnceLock which requires sync init (can't await inside).
    script_hashes: tokio::sync::OnceCell<ScriptHashes>,
}

impl RateLimitRedis {
    /// Create a new RateLimitRedis plugin from configuration.
    pub fn create(config: &RateLimitRedisConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();
        Box::new(RateLimitRedis {
            name: "RateLimitRedis".to_string(),
            config: validated,
            script_hashes: tokio::sync::OnceCell::new(),
        })
    }

    /// Load scripts into Redis and cache SHA1 hashes.
    ///
    /// Called lazily on first request. tokio::sync::OnceCell guarantees only one
    /// concurrent caller runs the init closure; others await the result.
    /// If the closure fails, the error is NOT cached — next call retries.
    async fn ensure_scripts_loaded(&self, client: &RedisLinkClient) -> anyhow::Result<&ScriptHashes> {
        self.script_hashes
            .get_or_try_init(|| async {
                let sw: String = client
                    .pool()
                    .script_load(SLIDING_WINDOW_SCRIPT)
                    .await
                    .map_err(|e| anyhow::anyhow!("script_load sliding_window: {:?}", e))?;
                let fw: String = client
                    .pool()
                    .script_load(FIXED_WINDOW_SCRIPT)
                    .await
                    .map_err(|e| anyhow::anyhow!("script_load fixed_window: {:?}", e))?;
                let tb: String = client
                    .pool()
                    .script_load(TOKEN_BUCKET_SCRIPT)
                    .await
                    .map_err(|e| anyhow::anyhow!("script_load token_bucket: {:?}", e))?;
                Ok(ScriptHashes {
                    sliding_window: sw,
                    fixed_window: fw,
                    token_bucket: tb,
                })
            })
            .await
    }

    /// Execute Lua script via EVALSHA with automatic EVAL fallback on NOSCRIPT.
    ///
    /// NOSCRIPT occurs when Redis restarts or SCRIPT FLUSH is called, clearing
    /// the script cache. We detect it via error details containing "NOSCRIPT"
    /// and fall back to full EVAL (which re-caches the script).
    async fn eval_script(
        &self,
        client: &RedisLinkClient,
        hash: &str,
        script: &str,
        keys: Vec<String>,
        args: Vec<String>,
    ) -> anyhow::Result<Vec<i64>> {
        match client
            .pool()
            .evalsha::<Vec<i64>, _, _, _>(hash, keys.clone(), args.clone())
            .await
        {
            Ok(result) => Ok(result),
            Err(ref e) if e.details().contains("NOSCRIPT") => {
                // Redis restarted or SCRIPT FLUSH — fall back to EVAL (sends full script body)
                tracing::debug!("EVALSHA NOSCRIPT for hash {}, falling back to EVAL", hash);
                client
                    .pool()
                    .eval(script, keys, args)
                    .await
                    .map_err(|e| anyhow::anyhow!("Redis EVAL fallback failed: {:?}", e))
            }
            Err(e) => Err(anyhow::anyhow!("Redis EVALSHA failed: {:?}", e)),
        }
    }

    /// Execute sliding window rate limit check for a single policy.
    #[allow(clippy::too_many_arguments)]
    async fn eval_sliding_window(
        &self,
        client: &RedisLinkClient,
        hash: &str,
        idx: usize,
        limit_key: &str,
        rate: u64,
        window_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<RateLimitResult> {
        let redis_key = build_redis_key(&self.config.key_prefix, "sw", idx, limit_key, "");
        let keys = vec![redis_key];
        let args = vec![
            rate.to_string(),
            window_ms.to_string(),
            now_ms.to_string(),
            self.config.cost.to_string(),
        ];
        let values = self
            .eval_script(client, hash, SLIDING_WINDOW_SCRIPT, keys, args)
            .await?;
        Ok(RateLimitResult::from_lua_result(&values, rate))
    }

    /// Execute fixed window rate limit check for a single policy.
    #[allow(clippy::too_many_arguments)]
    async fn eval_fixed_window(
        &self,
        client: &RedisLinkClient,
        hash: &str,
        idx: usize,
        limit_key: &str,
        rate: u64,
        window_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<RateLimitResult> {
        // Fixed window includes window_id in the key for automatic rotation
        let window_id = now_ms / window_ms;
        let redis_key = build_redis_key(&self.config.key_prefix, "fw", idx, limit_key, &window_id.to_string());
        let keys = vec![redis_key];
        let args = vec![rate.to_string(), window_ms.to_string(), self.config.cost.to_string()];
        let values = self.eval_script(client, hash, FIXED_WINDOW_SCRIPT, keys, args).await?;
        Ok(RateLimitResult::from_lua_result(&values, rate))
    }

    /// Execute token bucket rate limit check for a single policy.
    #[allow(clippy::too_many_arguments)]
    async fn eval_token_bucket(
        &self,
        client: &RedisLinkClient,
        hash: &str,
        idx: usize,
        limit_key: &str,
        rate: u64,
        window_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<RateLimitResult> {
        let redis_key = build_redis_key(&self.config.key_prefix, "tb", idx, limit_key, "");
        let keys = vec![redis_key];
        let args = vec![
            rate.to_string(),             // max_tokens
            rate.to_string(),             // refill_rate (same as max_tokens)
            window_ms.to_string(),        // interval_ms
            now_ms.to_string(),           // now
            self.config.cost.to_string(), // cost
        ];
        let values = self.eval_script(client, hash, TOKEN_BUCKET_SCRIPT, keys, args).await?;
        Ok(RateLimitResult::from_lua_result(&values, rate))
    }

    /// Merge two rate limit results, keeping the most restrictive.
    ///
    /// - If either is denied → result is denied (use largest retry_ms)
    /// - If both allowed → use lowest remaining and lowest limit
    fn merge_result(existing: Option<RateLimitResult>, new: RateLimitResult) -> RateLimitResult {
        match existing {
            None => new,
            Some(prev) => {
                if !prev.allowed || !new.allowed {
                    // At least one denied — merge as denied
                    RateLimitResult {
                        allowed: false,
                        current: prev.current.max(new.current),
                        remaining: 0,
                        limit: prev.limit.min(new.limit),
                        retry_ms: prev.retry_ms.max(new.retry_ms),
                    }
                } else {
                    // Both allowed — use most restrictive remaining
                    if new.remaining < prev.remaining {
                        RateLimitResult {
                            allowed: true,
                            current: new.current,
                            remaining: new.remaining,
                            limit: new.limit,
                            retry_ms: 0,
                        }
                    } else {
                        prev
                    }
                }
            }
        }
    }

    /// Check all policies against Redis. Returns aggregate result.
    async fn check_rate_limit(&self, client: &RedisLinkClient, limit_key: &str) -> anyhow::Result<RateLimitResult> {
        // Check Redis health before expensive operations.
        // healthy() is an AtomicBool — zero-cost check, NOT a PING.
        if !client.healthy() {
            anyhow::bail!("Redis client '{}' is not healthy", client.name());
        }

        let hashes = self.ensure_scripts_loaded(client).await?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Truncate limit_key ONCE before all policy evaluations.
        let safe_key = truncate_limit_key(limit_key, self.config.max_key_len);

        let mut most_restrictive: Option<RateLimitResult> = None;

        for (idx, policy) in self.config.policies.iter().enumerate() {
            let window_ms = policy.interval_duration.unwrap_or(Duration::from_secs(1)).as_millis() as i64;

            let result = match policy.algorithm {
                RateLimitAlgorithm::SlidingWindow => {
                    self.eval_sliding_window(
                        client,
                        &hashes.sliding_window,
                        idx,
                        safe_key,
                        policy.rate,
                        window_ms,
                        now_ms,
                    )
                    .await?
                }
                RateLimitAlgorithm::FixedWindow => {
                    self.eval_fixed_window(
                        client,
                        &hashes.fixed_window,
                        idx,
                        safe_key,
                        policy.rate,
                        window_ms,
                        now_ms,
                    )
                    .await?
                }
                RateLimitAlgorithm::TokenBucket => {
                    self.eval_token_bucket(
                        client,
                        &hashes.token_bucket,
                        idx,
                        safe_key,
                        policy.rate,
                        window_ms,
                        now_ms,
                    )
                    .await?
                }
            };

            most_restrictive = Some(Self::merge_result(most_restrictive, result));
        }

        // At least one policy is guaranteed by validation
        Ok(most_restrictive.unwrap())
    }

    /// Add rate limit headers to response.
    fn add_headers(&self, session: &mut dyn PluginSession, remaining: i64, limit: u64) {
        if !self.config.show_limit_headers {
            return;
        }

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
                // For Redis-based rate limiting, we don't have a precise reset timestamp
                // Use current time + retry estimate (0 for allowed requests)
                let reset_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = session.set_response_header(reset_header, &reset_timestamp.to_string());
            }
        } else {
            // Default: use X-RateLimit-* style
            let _ = session.set_response_header("X-RateLimit-Limit", &limit.to_string());
            let _ = session.set_response_header("X-RateLimit-Remaining", &remaining.to_string());
        }
    }

    /// Build rejection response with 429 status and rate limit headers.
    async fn reject_request(
        &self,
        session: &mut dyn PluginSession,
        retry_after: Duration,
        result: Option<&RateLimitResult>,
    ) -> PluginRunningResult {
        let message = self.config.reject_message.as_deref().unwrap_or("Rate limit exceeded");

        let mut resp = Box::new(
            ResponseHeader::build(self.config.reject_status, None)
                .unwrap_or_else(|_| ResponseHeader::build(429, None).expect("429 is valid")),
        );
        resp.insert_header("Content-Type", "application/json").ok();
        resp.insert_header("Retry-After", retry_after.as_secs().to_string())
            .ok();

        // Add rate limit headers (all controlled by show_limit_headers)
        if self.config.show_limit_headers {
            if let Some(r) = result {
                let reset_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() + retry_after.as_secs())
                    .unwrap_or(0);

                if let Some(headers) = self.config.get_header_names() {
                    if let Some(limit_header) = headers.limit_header() {
                        resp.insert_header(limit_header.to_string(), r.limit.to_string()).ok();
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
                    resp.insert_header("X-RateLimit-Limit", r.limit.to_string()).ok();
                    resp.insert_header("X-RateLimit-Remaining", "0").ok();
                    resp.insert_header("X-RateLimit-Reset", reset_timestamp.to_string())
                        .ok();
                }
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

    /// Apply the configured failure policy when Redis is unavailable or errors.
    async fn apply_redis_failure_policy(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        match self.config.on_redis_failure {
            OnRedisFailure::Allow => PluginRunningResult::GoodNext,
            OnRedisFailure::Deny => self.reject_request(session, Duration::ZERO, None).await,
        }
    }
}

// ========== RequestFilter Implementation ==========

#[async_trait]
impl RequestFilter for RateLimitRedis {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // 1. Validate config
        if !self.config.is_valid() {
            let err = self.config.get_validation_error().unwrap_or("Unknown error");
            tracing::error!(plugin = "RateLimitRedis", error = err, "Config invalid — fail-close");
            plugin_log.push(&format!("Config error (fail-close): {}; ", err));
            let resp = Box::new(
                ResponseHeader::build(500, None)
                    .unwrap_or_else(|_| ResponseHeader::build(429, None).expect("429 is valid")),
            );
            let _ = session.write_response_header(resp, false).await;
            let _ = session
                .write_response_body(
                    Some(bytes::Bytes::from(r#"{"message":"Internal rate limiter error"}"#)),
                    true,
                )
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        // 2. Extract key (identical logic to RateLimit)
        let limit_key = {
            let mut parts: Vec<String> = Vec::new();
            for k in &self.config.key {
                if let Some(v) = session.key_get(k).await {
                    parts.push(v);
                }
            }
            if parts.is_empty() {
                if let Some(ref dk) = self.config.default_key {
                    plugin_log.push(&format!("Using default key: {}; ", dk));
                    dk.clone()
                } else {
                    match self.config.on_missing_key {
                        OnMissingKey::Allow => {
                            plugin_log.push("No limit key, allowing (fail-open); ");
                            return PluginRunningResult::GoodNext;
                        }
                        OnMissingKey::Deny => {
                            plugin_log.push("No limit key, denying; ");
                            return self.reject_request(session, Duration::ZERO, None).await;
                        }
                    }
                }
            } else {
                parts.join("_")
            }
        };

        // 3. Get Redis client
        let client: Arc<RedisLinkClient> = match get_redis_client(&self.config.redis_ref) {
            Some(c) => c,
            None => {
                plugin_log.push(&format!(
                    "Redis '{}' not found in LinkSys store; ",
                    self.config.redis_ref
                ));
                return self.apply_redis_failure_policy(session).await;
            }
        };

        // 4. Execute rate limit check
        match self.check_rate_limit(&client, &limit_key).await {
            Ok(result) if result.allowed => {
                self.add_headers(session, result.remaining, result.limit);
                plugin_log.push(&format!("Allowed (remaining: {}); ", result.remaining));
                PluginRunningResult::GoodNext
            }
            Ok(result) => {
                let retry_after = Duration::from_millis(result.retry_ms.max(0) as u64);
                plugin_log.push(&format!("Rate limited (retry_ms: {}); ", result.retry_ms));
                self.reject_request(session, retry_after, Some(&result)).await
            }
            Err(e) => {
                plugin_log.push(&format!("Redis error ({}): {}; ", self.config.redis_ref, e));
                self.apply_redis_failure_policy(session).await
            }
        }
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_limit_key_normal() {
        assert_eq!(truncate_limit_key("192.168.1.1", 4096), "192.168.1.1");
    }

    #[test]
    fn test_truncate_limit_key_exact() {
        let key = "a".repeat(4096);
        assert_eq!(truncate_limit_key(&key, 4096).len(), 4096);
    }

    #[test]
    fn test_truncate_limit_key_over() {
        let key = "a".repeat(5000);
        assert_eq!(truncate_limit_key(&key, 4096).len(), 4096);
    }

    #[test]
    fn test_truncate_limit_key_empty() {
        assert_eq!(truncate_limit_key("", 4096), "");
    }

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
    fn test_truncate_limit_key_custom_max_len() {
        let key = "a".repeat(200);
        assert_eq!(truncate_limit_key(&key, 64).len(), 64);
    }

    #[test]
    fn test_build_redis_key_sliding_window() {
        let key = build_redis_key("edgion:rl:", "sw", 0, "192.168.1.1", "");
        assert_eq!(key, "edgion:rl:sw:0:192.168.1.1");
    }

    #[test]
    fn test_build_redis_key_fixed_window() {
        let key = build_redis_key("edgion:rl:", "fw", 1, "192.168.1.1", "1707900");
        assert_eq!(key, "edgion:rl:fw:1:192.168.1.1:1707900");
    }

    #[test]
    fn test_build_redis_key_token_bucket() {
        let key = build_redis_key("edgion:rl:", "tb", 0, "api-key-123", "");
        assert_eq!(key, "edgion:rl:tb:0:api-key-123");
    }

    #[test]
    fn test_build_redis_key_custom_prefix() {
        let key = build_redis_key("myapp:rl:", "sw", 0, "test", "");
        assert_eq!(key, "myapp:rl:sw:0:test");
    }

    #[test]
    fn test_rate_limit_result_from_lua_allowed() {
        let result = RateLimitResult::from_lua_result(&[1, 5, 95, 0], 100);
        assert!(result.allowed);
        assert_eq!(result.current, 5);
        assert_eq!(result.remaining, 95);
        assert_eq!(result.limit, 100);
        assert_eq!(result.retry_ms, 0);
    }

    #[test]
    fn test_rate_limit_result_from_lua_denied() {
        let result = RateLimitResult::from_lua_result(&[0, 100, 0, 500], 100);
        assert!(!result.allowed);
        assert_eq!(result.current, 100);
        assert_eq!(result.remaining, 0);
        assert_eq!(result.retry_ms, 500);
    }

    #[test]
    fn test_rate_limit_result_from_lua_empty() {
        let result = RateLimitResult::from_lua_result(&[], 100);
        assert!(!result.allowed); // 0 != 1
        assert_eq!(result.current, 0);
        assert_eq!(result.remaining, 0);
        assert_eq!(result.retry_ms, 0);
    }

    #[test]
    fn test_merge_result_both_allowed() {
        let r1 = RateLimitResult {
            allowed: true,
            current: 5,
            remaining: 95,
            limit: 100,
            retry_ms: 0,
        };
        let r2 = RateLimitResult {
            allowed: true,
            current: 800,
            remaining: 200,
            limit: 1000,
            retry_ms: 0,
        };
        let merged = RateLimitRedis::merge_result(Some(r1), r2);
        assert!(merged.allowed);
        // r1 has lower remaining (95 < 200), so r1 is most restrictive
        assert_eq!(merged.remaining, 95);
        assert_eq!(merged.limit, 100);
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
        let merged = RateLimitRedis::merge_result(Some(r1), r2);
        assert!(!merged.allowed);
        assert_eq!(merged.remaining, 0);
        assert_eq!(merged.retry_ms, 3000);
    }

    #[test]
    fn test_merge_result_both_denied() {
        let r1 = RateLimitResult {
            allowed: false,
            current: 101,
            remaining: 0,
            limit: 100,
            retry_ms: 500,
        };
        let r2 = RateLimitResult {
            allowed: false,
            current: 1001,
            remaining: 0,
            limit: 1000,
            retry_ms: 3000,
        };
        let merged = RateLimitRedis::merge_result(Some(r1), r2);
        assert!(!merged.allowed);
        assert_eq!(merged.retry_ms, 3000); // Largest retry_ms
        assert_eq!(merged.limit, 100); // Smallest limit
    }

    #[test]
    fn test_merge_result_none() {
        let r = RateLimitResult {
            allowed: true,
            current: 5,
            remaining: 95,
            limit: 100,
            retry_ms: 0,
        };
        let merged = RateLimitRedis::merge_result(None, r);
        assert!(merged.allowed);
        assert_eq!(merged.remaining, 95);
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(2)), "2.0s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
    }

    #[test]
    fn test_format_duration_milliseconds() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
    }
}
