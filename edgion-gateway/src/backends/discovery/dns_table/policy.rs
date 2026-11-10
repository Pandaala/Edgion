//! Refresh policy of the Gateway DNS endpoint table (pure functions).

use std::time::Duration;

use edgion_resources::resources::edgion_gateway_config::{
    DnsResolverConfig, DNS_DEFAULT_FALLBACK_REFRESH_SECONDS, DNS_MAX_ENTRIES_LIMIT, DNS_MAX_TTL_SECONDS,
    DNS_MIN_REFRESH_FLOOR_SECONDS, DNS_MIN_TTL_FACTOR,
};

/// Longest back-off between retries of a failing target.
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(60);
/// Relative jitter applied to every scheduled refresh (+/- 10 %).
const JITTER_SPREAD: f64 = 0.2;

/// Hard floor of every refresh interval.
fn floor() -> Duration {
    Duration::from_secs(u64::from(DNS_MIN_REFRESH_FLOOR_SECONDS))
}

/// Largest DNS TTL (RFC 2181); also the largest interval ever scheduled.
fn ceiling() -> Duration {
    Duration::from_secs(u64::from(DNS_MAX_TTL_SECONDS))
}

/// Resolved `EdgionGatewayConfig.spec.dnsResolver` refresh settings.
#[derive(Clone, Debug, PartialEq)]
pub struct DnsRefreshPolicy {
    pub endpoint_table: bool,
    pub ttl_factor: f64,
    pub min_refresh: Duration,
    pub max_refresh: Duration,
    pub fallback_refresh: Duration,
    pub refresh_interval: Option<Duration>,
    pub max_stale: Duration,
    pub max_entries: usize,
}

impl Default for DnsRefreshPolicy {
    fn default() -> Self {
        Self::from_config(&DnsResolverConfig::default())
    }
}

impl DnsRefreshPolicy {
    /// Resolve the policy from a config. Values are clamped again so an
    /// unvalidated source can never produce a zero interval, an inverted range
    /// or an out-of-range factor.
    pub fn from_config(config: &DnsResolverConfig) -> Self {
        let floor = u64::from(DNS_MIN_REFRESH_FLOOR_SECONDS);
        let ceiling = u64::from(DNS_MAX_TTL_SECONDS);
        let min = u64::from(config.min_refresh_seconds).clamp(floor, ceiling);
        let max = u64::from(config.max_refresh_seconds).clamp(min, ceiling);
        let fallback = u64::from(
            config
                .fallback_refresh_seconds
                .unwrap_or(DNS_DEFAULT_FALLBACK_REFRESH_SECONDS),
        )
        .clamp(min, max);
        let ttl_factor = if config.ttl_factor.is_finite() {
            config.ttl_factor.clamp(DNS_MIN_TTL_FACTOR, 1.0)
        } else {
            0.5
        };
        Self {
            endpoint_table: config.endpoint_table,
            ttl_factor,
            min_refresh: Duration::from_secs(min),
            max_refresh: Duration::from_secs(max),
            fallback_refresh: Duration::from_secs(fallback),
            refresh_interval: config
                .refresh_interval_seconds
                .map(|seconds| Duration::from_secs(u64::from(seconds).clamp(floor, ceiling))),
            max_stale: Duration::from_secs(u64::from(config.max_stale_seconds).min(ceiling)),
            max_entries: (config.max_entries as usize).clamp(1, DNS_MAX_ENTRIES_LIMIT as usize),
        }
    }

    /// Interval until the next refresh after a successful answer, before jitter.
    ///
    /// A fixed `refreshInterval` wins and is exempt from `[minRefresh,
    /// maxRefresh]` (only the hard 2 s floor applies). Otherwise `ttl *
    /// ttlFactor` clamped to `[minRefresh, maxRefresh]`; a TTL-less answer uses
    /// `fallbackRefresh`. A reported TTL is first capped at the RFC 2181
    /// maximum, so the multiplication can never overflow.
    pub fn success_interval(&self, ttl: Option<Duration>) -> Duration {
        if let Some(fixed) = self.refresh_interval {
            return fixed;
        }
        match ttl {
            Some(ttl) => ttl
                .min(ceiling())
                .mul_f64(self.ttl_factor)
                .clamp(self.min_refresh, self.max_refresh),
            None => self.fallback_refresh,
        }
    }

    /// Jittered delay after a successful answer: a fixed interval stays above
    /// the hard floor, a TTL-derived one stays within `[minRefresh, maxRefresh]`.
    pub fn success_delay(&self, ttl: Option<Duration>, unit: f64) -> Duration {
        let base = self.success_interval(ttl);
        if self.refresh_interval.is_some() {
            jitter(base, unit).clamp(floor(), ceiling())
        } else {
            jitter(base, unit).clamp(self.min_refresh, self.max_refresh)
        }
    }

    /// Latest delay any successful answer can schedule under this policy; a
    /// policy change pulls existing refreshes in to at most this far out.
    pub fn latest_success_delay(&self) -> Duration {
        match self.refresh_interval {
            Some(fixed) => jitter(fixed, 1.0).clamp(floor(), ceiling()),
            None => self.max_refresh,
        }
    }

    /// Back-off after `failures` consecutive failures (1-based): `minRefresh`
    /// doubling, capped at `min(60 s, maxRefresh)`.
    pub fn failure_backoff(&self, failures: u32) -> Duration {
        let cap = self.failure_cap();
        let shift = failures.saturating_sub(1).min(16);
        self.min_refresh.saturating_mul(1u32 << shift).min(cap)
    }

    /// Jittered failure back-off, kept within `[minRefresh, min(60 s, maxRefresh)]`.
    pub fn failure_delay(&self, failures: u32, unit: f64) -> Duration {
        jitter(self.failure_backoff(failures), unit).clamp(self.min_refresh, self.failure_cap())
    }

    fn failure_cap(&self) -> Duration {
        MAX_FAILURE_BACKOFF.min(self.max_refresh).max(self.min_refresh)
    }
}

/// Spread `base` by +/- 10 % using `unit` in `[0, 1)`.
fn jitter(base: Duration, unit: f64) -> Duration {
    let unit = if unit.is_finite() { unit.clamp(0.0, 1.0) } else { 0.5 };
    let factor = 1.0 - JITTER_SPREAD / 2.0 + JITTER_SPREAD * unit;
    base.min(ceiling()).mul_f64(factor)
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/dns_table/policy_test.rs"]
mod tests;
