//! L4 ConnectionRateLimit stream plugin.
//!
//! Two-stage admission per accepted connection (pre-TLS):
//!   Stage 1 — local per-listener token-bucket gate (no Redis); always enforced
//!             when `perListener` is configured. Flood-robust backstop.
//!   Stage 2 — optional Redis windowed check (perSourceIp always; perListener
//!             only when redisDistributed), bounded by the configured
//!             redis_timeout (default 100ms, capped at 1s). On Redis error/timeout the per-dimension onRedisFailure
//!             policy applies (perListener default Deny, perSourceIp default Allow).
//!
//! Evaluation order: perListener before perSourceIp; deny on first deny.
//! NO tracing on this data path. Observability: the rejection metric
//! (`record_connection_rate_limit_reject`) on Deny, plus the Redis-failure
//! metric (`record_connection_rate_limit_redis_failure`) on every Redis failure
//! (so fail-open is alertable).

use async_trait::async_trait;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::local_gate::get_or_update_local_gate;
use crate::link_sys::get_redis_client;
use crate::observe::metrics::{record_connection_rate_limit_redis_failure, record_connection_rate_limit_reject};
use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::edgion_plugins::{OnRedisFailure, RateLimitAlgorithm, RateLimitPolicy};
use edgion_resources::resources::edgion_stream_plugins::ConnectionRateLimitConfig;

const PLUGIN_NAME: &str = "ConnectionRateLimit";

pub struct ConnectionRateLimitPlugin {
    algorithm: RateLimitAlgorithm,
    /// `config.key_prefix` scoped by the owning entry's identity
    /// (`<namespace>/<name>/#<index>`; stream entries carry no alias). Resolved
    /// once at build time.
    ///
    /// Every instance defaults to the same `keyPrefix`, and the limit keys carry
    /// only the listener port, so without this two `ConnectionRateLimit` entries
    /// active on one port — two TLSRoutes with different SNIs referencing
    /// different EdgionStreamPlugins, a listener-level plus a route-level
    /// attachment, or two entries in one resource — would index the same Redis
    /// counter while each checked it against its own `rate`, and with differing
    /// `interval` would corrupt each other's window rotation.
    scoped_key_prefix: String,
    /// The entry identity alone (`<namespace>/<name>/#<index>`), without the
    /// operator's `keyPrefix`.
    ///
    /// The local token bucket is a Redis-free backstop, so its registry slot must
    /// not move when a Redis-only knob changes: keying it by `scoped_key_prefix`
    /// would reset the bucket whenever `keyPrefix` is edited — including in pure
    /// `perListener` mode where no `redisRef` is configured at all.
    entry_scope: String,
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
            .min(edgion_resources::resources::edgion_stream_plugins::MAX_REDIS_ACCEPT_TIMEOUT_MS)
            .max(1);
        Self {
            algorithm: cfg.algorithm.clone(),
            scoped_key_prefix: format!("{}{}:conn:", cfg.key_prefix, entry_scope),
            entry_scope: entry_scope.to_string(),
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
        let port = ctx.listener_port;

        // ---- Stage 1+2: perListener (evaluated first) ----
        if let Some(rate) = self.pl_rate {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let gate = get_or_update_local_gate(&self.entry_scope, port, rate, self.pl_interval_ms);
            if !gate.try_acquire_at(now_ms) {
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

        // ---- Stage 2: perSourceIp (Redis) ----
        if let Some(rate) = self.ip_rate {
            let key = format!("ip:{}:{}", port, ctx.client_ip);
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
mod tests {
    use super::*;
    use crate::plugins::stream::stream_plugin_trait::StreamLayer;
    use edgion_resources::resources::edgion_stream_plugins::{ConnectionRateLimitConfig, PerListenerLimit};
    use std::net::IpAddr;

    fn cfg_local_only(rate: u64) -> ConnectionRateLimitConfig {
        ConnectionRateLimitConfig {
            redis_ref: None,
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: Some(PerListenerLimit {
                rate,
                interval: "1s".to_string(),
                redis_distributed: false,
                on_redis_failure: OnRedisFailure::Deny,
                interval_duration: None,
            }),
            per_source_ip: None,
            redis_timeout: "100ms".to_string(),
        }
    }

    #[test]
    fn redis_timeout_is_parsed_from_config() {
        let mut cfg = cfg_local_only(10);
        cfg.redis_timeout = "750ms".to_string();
        let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/test-esp/#0");
        assert_eq!(plugin.redis_timeout, std::time::Duration::from_millis(750));
    }

    #[test]
    fn redis_timeout_is_clamped_to_cap() {
        let mut cfg = cfg_local_only(10);
        cfg.redis_timeout = "30s".to_string();
        let plugin = ConnectionRateLimitPlugin::new(&cfg, "default/test-esp/#0");
        assert_eq!(plugin.redis_timeout, std::time::Duration::from_millis(1000));
    }

    #[tokio::test]
    async fn local_gate_denies_over_rate() {
        // Use a unique port so the global registry slot is isolated from other tests.
        let port = 64111u16;
        let plugin = ConnectionRateLimitPlugin::new(&cfg_local_only(2), "default/test-esp/#0");
        let ctx = StreamContext::new(IpAddr::from([10, 0, 0, 1]), port, StreamLayer::Stage1);
        let mut admitted = 0;
        for _ in 0..5 {
            if matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow) {
                admitted += 1;
            }
        }
        // capacity 2 in a 1s window, all within the same test instant
        assert!(admitted <= 2, "expected <=2 admits, got {admitted}");
    }
}
