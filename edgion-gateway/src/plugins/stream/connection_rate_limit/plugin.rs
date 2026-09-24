//! L4 ConnectionRateLimit stream plugin.
//!
//! Two-step admission whenever the plugin's owning stream lifecycle invokes it:
//!   Step 1 — local per-listener token-bucket gate (no Redis); always enforced
//!             when `perListener` is configured. Flood-robust backstop.
//!   Step 2 — optional Redis windowed check (perSourceIp always; perListener
//!             only when redisDistributed), bounded by the configured
//!             redis_timeout (default 100ms, capped at 1s). On Redis error/timeout the per-dimension onRedisFailure
//!             policy applies (perListener default Deny, perSourceIp default Allow).
//!
//! Evaluation order: perListener before perSourceIp; deny on first deny.
//! NO tracing on this data path. Observability: the rejection metric
//! (`record_connection_rate_limit_reject`) on Deny, plus the Redis-failure
//! metric (`record_connection_rate_limit_redis_failure`) on every Redis failure
//! (so fail-open is alertable).
//! Listener attachment runs before inbound protocol parsing; TCP/UDP route
//! attachments run at their respective stream lifecycles.

use async_trait::async_trait;
use std::time::Duration;

use super::local_gate::LocalGateBinding;
use crate::link_sys::get_redis_client;
use crate::observe::metrics::{record_connection_rate_limit_redis_failure, record_connection_rate_limit_reject};
use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::edgion_plugins::{
    OnRedisFailure, PluginValidation, RateLimitAlgorithm, RateLimitPolicy,
};
use edgion_resources::resources::edgion_stream_plugins::ConnectionRateLimitConfig;

const PLUGIN_NAME: &str = "ConnectionRateLimit";

pub struct ConnectionRateLimitPlugin {
    init_failed: bool,
    algorithm: RateLimitAlgorithm,
    /// `config.key_prefix` scoped by the owning entry's identity
    /// (`<namespace>/<name>/#<index>`; stream entries carry no alias). Resolved
    /// once at build time.
    ///
    /// Every instance defaults to the same `keyPrefix`, and the limit keys carry
    /// only the listener port, so without this two `ConnectionRateLimit` entries
    /// active on one port — a listener-level plus a TCPRoute-level
    /// attachment, or two entries in one resource — would index the same Redis
    /// counter while each checked it against its own `rate`, and with differing
    /// `interval` would corrupt each other's window rotation.
    scoped_key_prefix: String,
    /// Persistent entry budget and this runtime's immutable local generation.
    /// Identity excludes Redis keyPrefix; each port activates a new generation
    /// on its first local admission, without resetting the shared budget.
    local_gate: Option<LocalGateBinding>,
    redis_ref: Option<String>,
    // perListener
    pl_rate: Option<u64>,
    pl_interval_ms: u64,
    pl_redis_distributed: bool,
    pl_on_redis_failure: OnRedisFailure,
    // perSourceIp
    ip_rate: Option<u64>,
    ip_interval_ms: u64,
    ip_on_redis_failure: OnRedisFailure,
    redis_timeout: Duration,
    core: RedisRateLimiter,
}

impl ConnectionRateLimitPlugin {
    /// `entry_scope` is the owning EdgionStreamPlugins entry's identity; see
    /// `scoped_key_prefix`.
    pub fn new(cfg: &ConnectionRateLimitConfig, entry_scope: &str) -> Self {
        let mut init_failed = if cfg.validation_error().is_some() {
            tracing::error!(
                component = "stream_connection_rate_limit",
                entry_scope,
                error = "invalid configuration",
                "Failed to initialize ConnectionRateLimitConfig; all connections will be denied",
            );
            true
        } else {
            false
        };
        let parse_ms = |s: &str| {
            edgion_resources::duration::parse_duration(s)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(1000)
                .max(1)
        };
        let (pl_rate, pl_interval_ms, pl_redis_distributed, pl_fail) = match &cfg.per_listener {
            Some(l) => (
                Some(l.rate),
                parse_ms(&l.interval),
                l.redis_distributed,
                l.on_redis_failure.clone(),
            ),
            None => (None, 1000, false, OnRedisFailure::Deny),
        };
        let (ip_rate, ip_interval_ms, ip_fail) = match &cfg.per_source_ip {
            Some(s) => (Some(s.rate), parse_ms(&s.interval), s.on_redis_failure.clone()),
            None => (None, 1000, OnRedisFailure::Allow),
        };
        let redis_timeout_ms = parse_duration(&cfg.redis_timeout)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(100)
            .clamp(
                1,
                edgion_resources::resources::edgion_stream_plugins::MAX_REDIS_ACCEPT_TIMEOUT_MS,
            );
        let local_gate = if !init_failed {
            pl_rate.and_then(|rate| {
                let binding = LocalGateBinding::new(entry_scope, rate, pl_interval_ms);
                if binding.is_none() {
                    init_failed = true;
                    tracing::error!(
                        component = "stream_connection_rate_limit",
                        entry_scope,
                        error = "local gate initialization failed",
                        "Failed to initialize ConnectionRateLimitConfig; all connections will be denied",
                    );
                }
                binding
            })
        } else {
            None
        };
        Self {
            init_failed,
            algorithm: cfg.algorithm.clone(),
            scoped_key_prefix: format!("{}{}:conn:", cfg.key_prefix, entry_scope),
            local_gate,
            redis_ref: cfg.redis_ref.clone(),
            pl_rate,
            pl_interval_ms,
            pl_redis_distributed,
            pl_on_redis_failure: pl_fail,
            ip_rate,
            ip_interval_ms,
            ip_on_redis_failure: ip_fail,
            redis_timeout: Duration::from_millis(redis_timeout_ms),
            core: RedisRateLimiter::new(),
        }
    }

    fn one_policy(&self, rate: u64, interval_ms: u64) -> RateLimitPolicy {
        RateLimitPolicy {
            rate,
            interval: String::new(), // unused at runtime; interval_duration drives window
            algorithm: self.algorithm.clone(),
            interval_duration: Some(Duration::from_millis(interval_ms)),
        }
    }

    /// Run one Redis dimension with the configured accept-path timeout.
    /// Returns true=allow, false=deny. On any Redis failure it records the
    /// failure metric (so fail-open is observable) and applies `on_failure`.
    async fn redis_dimension(
        &self,
        dimension: &'static str,
        limit_key: &str,
        rate: u64,
        interval_ms: u64,
        on_failure: &OnRedisFailure,
    ) -> bool {
        let Some(redis_ref) = &self.redis_ref else {
            record_connection_rate_limit_redis_failure(dimension, "unavailable");
            return matches!(on_failure, OnRedisFailure::Allow);
        };
        // Bind the client Arc once; reuse it for the pre-check and the check.
        let Some(client) = get_redis_client(redis_ref) else {
            record_connection_rate_limit_redis_failure(dimension, "unavailable");
            return matches!(on_failure, OnRedisFailure::Allow);
        };
        // Fast-eject + classify: a known-down client never issues a command.
        if !client.healthy() {
            record_connection_rate_limit_redis_failure(dimension, "unavailable");
            return matches!(on_failure, OnRedisFailure::Allow);
        }
        let policies = [self.one_policy(rate, interval_ms)];
        let fut = self
            .core
            .check(&client, &policies, &self.scoped_key_prefix, 1, 4096, limit_key);
        match tokio::time::timeout(self.redis_timeout, fut).await {
            Ok(Ok(result)) => result.allowed,
            _ => {
                record_connection_rate_limit_redis_failure(dimension, "runtime");
                matches!(on_failure, OnRedisFailure::Allow)
            }
        }
    }
}

#[async_trait]
impl StreamPlugin for ConnectionRateLimitPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        if self.init_failed {
            return StreamPluginResult::Deny("connrate:deny:invalid-config".to_string());
        }
        let port = ctx.listener_port;

        // ---- Steps 1+2: perListener (evaluated first) ----
        if let Some(rate) = self.pl_rate {
            if !self.local_gate.as_ref().is_some_and(|gate| gate.try_acquire(port)) {
                record_connection_rate_limit_reject("listener", "local");
                return StreamPluginResult::Deny("connrate:deny:listener:local".to_string());
            }
            if self.pl_redis_distributed {
                let key = format!("lst:{port}");
                let allowed = self
                    .redis_dimension("listener", &key, rate, self.pl_interval_ms, &self.pl_on_redis_failure)
                    .await;
                if !allowed {
                    record_connection_rate_limit_reject("listener", "redis");
                    return StreamPluginResult::Deny("connrate:deny:listener:redis".to_string());
                }
            }
        }

        // ---- Step 2: perSourceIp (Redis) ----
        if let Some(rate) = self.ip_rate {
            let key = format!("ip:{}:{}", port, ctx.direct_peer_ip);
            let allowed = self
                .redis_dimension("source_ip", &key, rate, self.ip_interval_ms, &self.ip_on_redis_failure)
                .await;
            if !allowed {
                record_connection_rate_limit_reject("source_ip", "redis");
                return StreamPluginResult::Deny("connrate:deny:source_ip:redis".to_string());
            }
        }

        StreamPluginResult::Allow
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/connection_rate_limit/plugin_test.rs"]
mod tests;
