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
use smallvec::SmallVec;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::link_sys::get_redis_client;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::plugins::common::redis_rate_limiter::{truncate_limit_key, RateLimitResult, RedisRateLimiter};
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
    force_json_rejection: bool,
    /// `config.key_prefix` scoped by the owning entry's identity and quota name
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
    quota_prefixes: Vec<String>,
    response_quota: Option<usize>,
    core: RedisRateLimiter,
}

impl RateLimitRedis {
    /// Create a new RateLimitRedis plugin from configuration.
    ///
    /// `entry_scope` is the owning EdgionPlugins entry's identity; see
    /// `quota_prefixes`.
    pub fn create(config: &RateLimitRedisConfig, entry_scope: &str) -> Box<dyn RequestFilter> {
        Box::new(Self::new(config, entry_scope))
    }

    pub(crate) fn new(config: &RateLimitRedisConfig, entry_scope: &str) -> RateLimitRedis {
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
        let quota_prefixes = validated
            .quotas
            .iter()
            .map(|quota| format!("{scoped_key_prefix}q:{}:{}:", quota.name.len(), quota.name))
            .collect();
        let response_quota = validated
            .response_quota
            .as_ref()
            .and_then(|name| validated.quotas.iter().position(|quota| &quota.name == name));
        RateLimitRedis {
            config: validated,
            config_valid,
            reject_body,
            force_json_rejection: false,
            quota_prefixes,
            response_quota,
            core: RedisRateLimiter::new(),
        }
    }

    /// Select a precomputed caller-specific body at cold construction time.
    /// Rejection headers and writes remain owned by the canonical limiter.
    pub(crate) fn with_json_rejection_body(mut self, body: Bytes) -> Self {
        self.reject_body = body;
        self.force_json_rejection = true;
        self
    }

    /// Prepare every identity before any debit. Oversized backing allocations
    /// are released here rather than retained through a slow Redis exchange.
    fn prepare(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        costs: Option<&[u32]>,
    ) -> Result<SmallVec<[PreparedQuota; 1]>, PreparationError> {
        if costs.is_some_and(|costs| costs.len() != self.config.quotas.len()) {
            return Err(PreparationError::InvalidCost);
        }
        let mut jobs = SmallVec::new();
        for (index, quota) in self.config.quotas.iter().enumerate() {
            let cost = costs.map_or(quota.cost, |costs| costs[index]);
            // Only computed root slots can be absent. Total must remain nonzero.
            if costs.is_some() && index > 0 && cost == 0 {
                continue;
            }
            if !(1..=1_000_000).contains(&cost) {
                return Err(PreparationError::InvalidCost);
            }
            let key = match resolve_rate_limit_key(
                session,
                &quota.key,
                quota.default_key.as_deref(),
                &quota.on_key_miss,
                log,
            ) {
                RateLimitKey::Resolved(key) => key,
                RateLimitKey::Allow => continue,
                RateLimitKey::Deny => return Err(PreparationError::KeyMiss),
            };
            let original_len = key.len();
            let key = bound_prepared_key(key, self.config.max_key_len);
            jobs.push(PreparedQuota {
                index,
                key,
                original_len,
                cost,
            });
        }
        Ok(jobs)
    }

    /// Data-only checks; response operations are deliberately outside this future.
    async fn check_prepared(&self, client: &RedisLinkClient, jobs: &[PreparedQuota]) -> anyhow::Result<QuotaDecision> {
        let mut selected = None;
        for job in jobs {
            let result = self
                .core
                .check(
                    client,
                    &self.config.quotas[job.index].policies,
                    &self.quota_prefixes[job.index],
                    job.cost,
                    self.config.max_key_len,
                    &job.key,
                )
                .await?;
            let checked = CheckedQuota {
                original_len: job.original_len,
                result,
            };
            if !checked.result.allowed {
                return Ok(QuotaDecision::Limited(checked));
            }
            if self
                .response_quota
                .map_or(selected.is_none(), |index| index == job.index)
            {
                selected = Some(checked);
            }
        }
        Ok(QuotaDecision::Allowed(selected))
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

        if self.force_json_rejection {
            // The adapter drains queued operations after this response header.
            // Append last so earlier plugins cannot mislabel the fixed JSON body.
            let _ = session.set_response_header("Content-Type", "application/json");
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

struct PreparedQuota {
    index: usize,
    key: String,
    original_len: usize,
    cost: u32,
}

enum PreparationError {
    KeyMiss,
    InvalidCost,
}

struct CheckedQuota {
    original_len: usize,
    result: RateLimitResult,
}

enum QuotaDecision {
    Allowed(Option<CheckedQuota>),
    Limited(CheckedQuota),
}

fn bound_prepared_key(key: String, max_len: usize) -> String {
    if key.capacity() > max_len {
        truncate_limit_key(&key, max_len).to_owned()
    } else {
        key
    }
}

impl RateLimitRedis {
    /// Computed charges use the same identity preparation and final renderer.
    pub(crate) async fn run_request_with_costs(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        costs: &[u32],
    ) -> PluginRunningResult {
        self.run_quotas(session, log, Some(costs)).await
    }

    async fn invalid_config(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        log.push_err("cfg load fail");
        let mut resp = Box::new(ResponseHeader::build(500, None).expect("500 is a valid status code"));
        resp.insert_header("Content-Type", "application/json").ok();
        let body = Bytes::from_static(br#"{"message":"Internal rate limiter error"}"#);
        if session.write_response_header(resp, false).await.is_err() {
            return PluginRunningResult::ErrTerminateRequest;
        }
        let _ = session.write_response_body(Some(body), true).await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn run_quotas(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        costs: Option<&[u32]>,
    ) -> PluginRunningResult {
        if !self.config_valid {
            return self.invalid_config(session, log).await;
        }
        let jobs = match self.prepare(session, log, costs) {
            Ok(jobs) => jobs,
            Err(PreparationError::KeyMiss) => return self.reject_request(session, Duration::ZERO, None).await,
            Err(PreparationError::InvalidCost) => return self.invalid_config(session, log).await,
        };
        if jobs.is_empty() {
            return PluginRunningResult::GoodNext;
        }
        let client: Arc<RedisLinkClient> = match get_redis_client(&self.config.redis_ref) {
            Some(client) if client.healthy() => client,
            _ => {
                log.push_err("redis unavail");
                return self.apply_redis_failure_policy(session).await;
            }
        };
        let decision = match self.config.request_timeout_duration {
            Some(timeout) => match tokio::time::timeout(timeout, self.check_prepared(&client, &jobs)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("request_timeout elapsed")),
            },
            None => self.check_prepared(&client, &jobs).await,
        };
        // Drop all prepared identity ownership before response awaits.
        drop(jobs);
        match decision {
            Ok(QuotaDecision::Allowed(selected)) => {
                if let Some(checked) = selected {
                    self.add_headers(session, checked.result.remaining, checked.result.limit);
                    log.push_fmt(
                        "rl:ok,kl={},r={}",
                        la![
                            checked.original_len.to_string().as_str(),
                            checked.result.remaining.to_string().as_str()
                        ],
                    );
                }
                PluginRunningResult::GoodNext
            }
            Ok(QuotaDecision::Limited(checked)) => {
                let retry_after = Duration::from_millis(checked.result.retry_ms.max(0) as u64);
                log.push_fmt(
                    "rl:lim,kl={},rms={}",
                    la![
                        checked.original_len.to_string().as_str(),
                        checked.result.retry_ms.to_string().as_str()
                    ],
                );
                self.reject_request(session, retry_after, Some(&checked.result)).await
            }
            Err(_) => {
                log.push_err("redis rt fail");
                self.apply_redis_failure_policy(session).await
            }
        }
    }
}

#[async_trait]
impl RequestFilter for RateLimitRedis {
    fn name(&self) -> &str {
        "RateLimitRedis"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.run_quotas(session, log, None).await
    }
}

// ========== Tests ==========

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/rate_limit_redis/plugin_test.rs"]
mod tests;
