//! Shared Redis rate-limit execution core.
//!
//! Extracted from the HTTP `rate_limit_redis` plugin so both the HTTP request
//! limiter and the L4 connection-rate limiter share one implementation of the
//! SCRIPT LOAD / EVALSHA-with-EVAL-fallback machinery and the Lua algorithms.
//! This core is protocol-agnostic: it never touches `PluginSession` /
//! `PluginLog` / HTTP responses. Callers map `RateLimitResult.allowed` to their
//! own allow/deny action.

pub mod scripts;

use crate::link_sys::providers::redis::RedisLinkClient;
use edgion_resources::resources::edgion_plugins::{RateLimitAlgorithm, RateLimitPolicy};
use fred::interfaces::LuaInterface;
use scripts::{FIXED_WINDOW_SCRIPT, SLIDING_WINDOW_SCRIPT, TOKEN_BUCKET_SCRIPT};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Parsed result of a single Lua rate-limit reply `{allowed, current, remaining, retry_ms}`.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub current: i64,
    pub remaining: i64,
    pub limit: u64,
    pub retry_ms: i64,
}

impl RateLimitResult {
    pub fn from_lua_result(values: &[i64], limit: u64) -> Self {
        Self {
            allowed: values.first().copied().unwrap_or(0) == 1,
            current: values.get(1).copied().unwrap_or(0),
            remaining: values.get(2).copied().unwrap_or(0),
            limit,
            retry_ms: values.get(3).copied().unwrap_or(0),
        }
    }

    /// Aggregate across policies: deny wins; among allows keep the tightest remaining.
    pub fn merge(existing: Option<Self>, new: Self) -> Self {
        match existing {
            None => new,
            Some(prev) => {
                if !prev.allowed || !new.allowed {
                    RateLimitResult {
                        allowed: false,
                        current: prev.current.max(new.current),
                        remaining: 0,
                        limit: prev.limit.min(new.limit),
                        retry_ms: prev.retry_ms.max(new.retry_ms),
                    }
                } else if new.remaining < prev.remaining {
                    new
                } else {
                    prev
                }
            }
        }
    }
}

/// Build a single Redis key: `{prefix}{algo}:{policy_idx}:{limit_key}[:{suffix}]`.
pub fn build_redis_key(prefix: &str, algo: &str, policy_idx: usize, limit_key: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}")
    } else {
        format!("{prefix}{algo}:{policy_idx}:{limit_key}:{suffix}")
    }
}

/// Truncate the variable key segment on a UTF-8 boundary.
pub fn truncate_limit_key(raw: &str, max_len: usize) -> &str {
    if raw.len() > max_len {
        let mut end = max_len;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        &raw[..end]
    } else {
        raw
    }
}

struct ScriptHashes {
    sliding_window: String,
    fixed_window: String,
    token_bucket: String,
}

/// Owns the lazily-loaded SCRIPT LOAD hashes so EVALSHA caching is shared per instance.
pub struct RedisRateLimiter {
    script_hashes: tokio::sync::OnceCell<ScriptHashes>,
}

impl Default for RedisRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RedisRateLimiter {
    pub fn new() -> Self {
        Self {
            script_hashes: tokio::sync::OnceCell::new(),
        }
    }

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
            Err(ref e) if e.details().contains("NOSCRIPT") => client
                .pool()
                .eval(script, keys, args)
                .await
                .map_err(|e| anyhow::anyhow!("Redis EVAL fallback failed: {:?}", e)),
            Err(e) => Err(anyhow::anyhow!("Redis EVALSHA failed: {:?}", e)),
        }
    }

    /// Evaluate one policy against `limit_key` (already truncated). `now_ms` is
    /// passed explicitly for testability.
    #[allow(clippy::too_many_arguments)]
    pub async fn eval_policy(
        &self,
        client: &RedisLinkClient,
        algorithm: &RateLimitAlgorithm,
        key_prefix: &str,
        policy_idx: usize,
        limit_key: &str,
        rate: u64,
        window_ms: i64,
        now_ms: i64,
        cost: u32,
    ) -> anyhow::Result<RateLimitResult> {
        let hashes = self.ensure_scripts_loaded(client).await?;
        match algorithm {
            RateLimitAlgorithm::SlidingWindow => {
                let key = build_redis_key(key_prefix, "sw", policy_idx, limit_key, "");
                let args = vec![
                    rate.to_string(),
                    window_ms.to_string(),
                    now_ms.to_string(),
                    cost.to_string(),
                ];
                let v = self
                    .eval_script(client, &hashes.sliding_window, SLIDING_WINDOW_SCRIPT, vec![key], args)
                    .await?;
                Ok(RateLimitResult::from_lua_result(&v, rate))
            }
            RateLimitAlgorithm::FixedWindow => {
                let window_id = now_ms / window_ms;
                let key = build_redis_key(key_prefix, "fw", policy_idx, limit_key, &window_id.to_string());
                let args = vec![rate.to_string(), window_ms.to_string(), cost.to_string()];
                let v = self
                    .eval_script(client, &hashes.fixed_window, FIXED_WINDOW_SCRIPT, vec![key], args)
                    .await?;
                Ok(RateLimitResult::from_lua_result(&v, rate))
            }
            RateLimitAlgorithm::TokenBucket => {
                let key = build_redis_key(key_prefix, "tb", policy_idx, limit_key, "");
                let args = vec![
                    rate.to_string(),
                    rate.to_string(),
                    window_ms.to_string(),
                    now_ms.to_string(),
                    cost.to_string(),
                ];
                let v = self
                    .eval_script(client, &hashes.token_bucket, TOKEN_BUCKET_SCRIPT, vec![key], args)
                    .await?;
                Ok(RateLimitResult::from_lua_result(&v, rate))
            }
        }
    }

    /// Multi-policy orchestration: truncate the key once, compute `now_ms`, run
    /// every policy, return the most restrictive result.
    pub async fn check(
        &self,
        client: &RedisLinkClient,
        policies: &[RateLimitPolicy],
        key_prefix: &str,
        cost: u32,
        max_key_len: usize,
        limit_key: &str,
    ) -> anyhow::Result<RateLimitResult> {
        if !client.healthy() {
            anyhow::bail!("Redis client '{}' is not healthy", client.name());
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let safe_key = truncate_limit_key(limit_key, max_key_len);

        let mut most_restrictive: Option<RateLimitResult> = None;
        for (idx, policy) in policies.iter().enumerate() {
            let window_ms = policy.interval_duration.unwrap_or(Duration::from_secs(1)).as_millis() as i64;
            let result = self
                .eval_policy(
                    client,
                    &policy.algorithm,
                    key_prefix,
                    idx,
                    safe_key,
                    policy.rate,
                    window_ms,
                    now_ms,
                    cost,
                )
                .await?;
            most_restrictive = Some(RateLimitResult::merge(most_restrictive, result));
        }
        most_restrictive
            .ok_or_else(|| anyhow::anyhow!("redis_rate_limiter: no policy after iteration — invariant violated"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_lua_result_parses_fields() {
        let r = RateLimitResult::from_lua_result(&[1, 7, 3, 0], 10);
        assert!(r.allowed);
        assert_eq!(r.current, 7);
        assert_eq!(r.remaining, 3);
        assert_eq!(r.limit, 10);
    }

    #[test]
    fn merge_denies_when_any_denies() {
        let allow = RateLimitResult::from_lua_result(&[1, 1, 9, 0], 10);
        let deny = RateLimitResult::from_lua_result(&[0, 11, 0, 500], 10);
        let merged = RateLimitResult::merge(Some(allow), deny);
        assert!(!merged.allowed);
        assert_eq!(merged.retry_ms, 500);
    }

    #[test]
    fn build_key_with_and_without_suffix() {
        assert_eq!(build_redis_key("p:", "sw", 0, "ip:1:2", ""), "p:sw:0:ip:1:2");
        assert_eq!(build_redis_key("p:", "fw", 1, "lst:80", "42"), "p:fw:1:lst:80:42");
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        assert_eq!(truncate_limit_key("abcdef", 3), "abc");
        assert_eq!(truncate_limit_key("ab", 10), "ab");
    }
}
