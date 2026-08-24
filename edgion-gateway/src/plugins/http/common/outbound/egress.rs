//! SSRF-guarded DNS resolution for outbound HTTP.
//!
//! [`EgressResolver`] implements [`reqwest::dns::Resolve`], so it classifies the
//! exact addresses reqwest is about to connect to. reqwest does not re-resolve,
//! which closes the TOCTOU / DNS-rebinding window that a config-time or
//! dial-time-only IP check leaves open.
//!
//! The address memo is owned by the resolver, not shared. Plugin callouts keep
//! it for the lifetime of their `ManagedOutboundClient`. Its cardinality is
//! bounded, and because the egress policy is a constant of the cache,
//! classified results can be stored directly. See the design spec, section 5.3.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use edgion_common::dns::{resolve as dns_resolve, ResolverSpec};
use edgion_common::test_mode::is_integration_testing_mode;

use crate::observe::metrics::record_egress_blocked;
use crate::util::net::{classify_ip, IpClass};

/// Hard cap on the resolver's address memo.
///
/// A webhook client resolves exactly one hostname, so this is slack, not a budget.
/// It exists so that a future consumer resolving request-derived hostnames through
/// this resolver cannot grow the map without bound.
pub const EGRESS_CACHE_MAX_ENTRIES: usize = 16;

/// Gateway-local default for `EgressPolicy.block_private` when a resource does
/// not set `target.blockPrivate` explicitly. Installed once at startup by
/// `configure_egress_defaults`; defaults to `false` (permissive) until then.
static EGRESS_BLOCK_PRIVATE: AtomicBool = AtomicBool::new(false);

/// Gateway-local default TTL (in milliseconds) for `EgressResolver`'s address
/// memo. Installed once at startup by `configure_egress_defaults`; defaults to
/// 5000ms (matching `backends::discovery::dns_cache::DEFAULT_DNS_CACHE_TTL`)
/// until then.
static EGRESS_DNS_TTL_MS: AtomicU64 = AtomicU64::new(5_000);

/// Install the gateway-local egress defaults. Called once at startup from the
/// CLI config, before any `ManagedOutboundClient` is built.
pub fn configure_egress_defaults(block_private: bool, dns_cache_ttl: Duration) {
    EGRESS_BLOCK_PRIVATE.store(block_private, Ordering::Relaxed);
    EGRESS_DNS_TTL_MS.store(dns_cache_ttl.as_millis() as u64, Ordering::Relaxed);
    tracing::info!(
        block_private,
        dns_cache_ttl_ms = dns_cache_ttl.as_millis() as u64,
        "Outbound egress defaults configured"
    );
}

/// The gateway-local default for `blockPrivate` when a resource does not set it.
pub fn default_block_private() -> bool {
    EGRESS_BLOCK_PRIVATE.load(Ordering::Relaxed)
}

/// The gateway-local TTL for `EgressResolver`'s address memo.
pub fn egress_dns_cache_ttl() -> Duration {
    Duration::from_millis(EGRESS_DNS_TTL_MS.load(Ordering::Relaxed))
}

/// Egress policy for a single outbound client.
///
/// Plugin and callout consumers constructed with [`EgressResolver::new`] reject
/// loopback and link-local addresses (including the cloud-metadata address
/// `169.254.169.254`). `block_private` additionally rejects RFC1918 and
/// unique-local addresses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EgressPolicy {
    pub block_private: bool,
}

struct CacheEntry {
    /// Already-classified addresses. May be empty (a cached rejection).
    addrs: Vec<IpAddr>,
    expires_at: Instant,
}

struct Inner {
    spec: ResolverSpec,
    policy: EgressPolicy,
    ttl: Duration,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

/// A canonical DNS resolver that applies an egress policy to every resolved address.
/// It also implements reqwest's resolver trait for plugin outbound clients.
#[derive(Clone)]
pub struct EgressResolver {
    inner: Arc<Inner>,
}

impl EgressResolver {
    pub fn new(spec: ResolverSpec, policy: EgressPolicy, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                spec,
                policy,
                ttl,
                cache: RwLock::new(HashMap::new()),
            }),
        }
    }

    #[cfg(test)]
    fn filter_addrs(&self, addrs: Vec<IpAddr>) -> Vec<IpAddr> {
        filter_resolved_addrs(addrs, self.inner.policy, false)
    }

    #[cfg(test)]
    fn cached(&self, host: &str, now: Instant) -> Option<Vec<IpAddr>> {
        self.inner.cached(host, now)
    }

    #[cfg(test)]
    fn store(&self, host: &str, addrs: Vec<IpAddr>, now: Instant) {
        self.inner.store(host, addrs, now)
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.inner.cache.read().len()
    }

    /// Resolve and classify the exact socket addresses an outbound connector
    /// will dial. Non-reqwest transports use this method so DNS, cache, test
    /// escape-hatch, metrics, and IP policy stay canonical.
    pub async fn resolve_socket_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let now = Instant::now();
        let allowed = match self.inner.cached(host, now) {
            Some(hit) => hit,
            None => {
                let raw = dns_resolve(&self.inner.spec, host)
                    .await
                    .map_err(|e| format!("DNS resolution failed for outbound target: {e}"))?;
                let allowed = filter_resolved_addrs(raw, self.inner.policy, false);
                self.inner.store(host, allowed.clone(), Instant::now());
                allowed
            }
        };
        if allowed.is_empty() {
            return Err("egress policy blocked all resolved addresses".to_string());
        }
        Ok(allowed.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
    }
}

impl Inner {
    /// Read the memo. Takes the read lock and releases it before returning.
    fn cached(&self, host: &str, now: Instant) -> Option<Vec<IpAddr>> {
        let guard = self.cache.read();
        let entry = guard.get(host)?;
        (entry.expires_at > now).then(|| entry.addrs.clone())
    }

    /// Write the memo, dropping expired entries and enforcing the cap.
    fn store(&self, host: &str, addrs: Vec<IpAddr>, now: Instant) {
        let mut guard = self.cache.write();
        guard.retain(|_, v| v.expires_at > now);
        if guard.len() >= EGRESS_CACHE_MAX_ENTRIES && !guard.contains_key(host) {
            guard.clear();
        }
        guard.insert(
            host.to_string(),
            CacheEntry {
                addrs,
                expires_at: now + self.ttl,
            },
        );
    }
}

/// Drop every resolved address rejected by the canonical outbound egress
/// policy and record a metric per drop. Routed upstreams pass their captured
/// `allowLoopbackUpstream` value; plugin callouts pass `false`.
pub(crate) fn filter_resolved_addrs(addrs: Vec<IpAddr>, policy: EgressPolicy, allow_loopback: bool) -> Vec<IpAddr> {
    addrs
        .into_iter()
        .filter(|ip| {
            let class = classify_ip(*ip);
            let reason = match class {
                // Integration tests bind test_server to 127.0.0.1; the same escape
                // hatch that `validate_endpoint_backend_addr` honors must apply here,
                // or the guard kills our own test suite.
                IpClass::Loopback if allow_loopback || cfg!(test) || is_integration_testing_mode() => return true,
                IpClass::Loopback => "loopback",
                IpClass::LinkLocal => "link_local",
                // Unspecified (0.0.0.0 / ::) is blocked unconditionally, not gated on
                // `block_private`: on Linux a connect to 0.0.0.0 is routed to
                // 127.0.0.1, so it is a loopback-equivalent bypass, not a private range.
                IpClass::Unspecified => "unspecified",
                IpClass::Private | IpClass::UniqueLocal if policy.block_private => "private",
                IpClass::Private | IpClass::UniqueLocal | IpClass::Global => return true,
            };
            record_egress_blocked(reason);
            false
        })
        .collect()
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let inner = Arc::clone(&self.inner);
        let host = name.as_str().to_string();

        Box::pin(async move {
            // No lock guard may cross the await below: `cached` takes and releases
            // the read lock, `store` takes and releases the write lock, and neither
            // guard is alive across `dns_resolve(..).await`.
            let now = Instant::now();
            let allowed = match inner.cached(&host, now) {
                Some(hit) => hit,
                None => {
                    let raw = dns_resolve(&inner.spec, &host)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                    let allowed = filter_resolved_addrs(raw, inner.policy, false);
                    inner.store(&host, allowed.clone(), Instant::now());
                    allowed
                }
            };

            if allowed.is_empty() {
                return Err(format!("egress policy blocked all resolved addresses for host '{host}'").into());
            }

            // Port 0: reqwest substitutes the URL's port, or the scheme's default.
            let addrs: Addrs = Box::new(allowed.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// `configure_egress_defaults` updates both process-global accessors, and
    /// the accessors read back exactly what was stored. Resets to the
    /// compile-time defaults afterward so other tests in this binary observe a
    /// stable starting point.
    #[test]
    fn configure_egress_defaults_updates_accessors() {
        configure_egress_defaults(true, Duration::from_millis(30_000));
        assert!(default_block_private());
        assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(30_000));

        // reset to compile-time defaults for other tests
        configure_egress_defaults(false, Duration::from_millis(5_000));
        assert!(!default_block_private());
        assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(5_000));
    }

    fn resolver(block_private: bool) -> EgressResolver {
        EgressResolver::new(
            ResolverSpec::System,
            EgressPolicy { block_private },
            Duration::from_secs(5),
        )
    }

    /// Link-local (including the cloud-metadata address) is always dropped.
    #[test]
    fn filter_always_drops_link_local_and_metadata() {
        let r = resolver(false);
        let out = r.filter_addrs(vec![ip("169.254.169.254"), ip("93.184.216.34")]);
        assert_eq!(out, vec![ip("93.184.216.34")]);
    }

    /// Private ranges pass by default and are dropped when block_private is set.
    #[test]
    fn filter_honors_block_private() {
        let addrs = vec![ip("10.1.2.3"), ip("192.168.1.1"), ip("fd00::1"), ip("93.184.216.34")];

        let permissive = resolver(false).filter_addrs(addrs.clone());
        assert_eq!(permissive.len(), 4, "private must pass by default");

        let strict = resolver(true).filter_addrs(addrs);
        assert_eq!(strict, vec![ip("93.184.216.34")]);
    }

    /// Every address blocked yields an empty set; the caller turns that into an Err.
    #[test]
    fn filter_can_return_empty() {
        let r = resolver(true);
        let out = r.filter_addrs(vec![ip("10.0.0.1"), ip("169.254.169.254")]);
        assert!(out.is_empty());
    }

    /// 0.0.0.0 / :: are blocked regardless of `block_private`: on Linux a TCP connect
    /// to 0.0.0.0 is routed to 127.0.0.1, so this must not be gated on the private-range
    /// policy knob.
    #[test]
    fn filter_always_drops_unspecified_regardless_of_block_private() {
        let r = resolver(false);
        let out = r.filter_addrs(vec![ip("0.0.0.0"), ip("::"), ip("93.184.216.34")]);
        assert_eq!(out, vec![ip("93.184.216.34")]);
    }

    /// A cached entry is served without re-invoking the underlying resolver.
    #[test]
    fn cache_hit_avoids_reresolution() {
        let r = resolver(false);
        let now = Instant::now();
        r.store("api.example.com", vec![ip("93.184.216.34")], now);
        assert_eq!(r.cached("api.example.com", now), Some(vec![ip("93.184.216.34")]));
    }

    /// An empty (all-blocked) result is cached too, so a hostile target is not
    /// re-resolved on every request.
    #[test]
    fn negative_result_is_cached() {
        let r = resolver(true);
        let now = Instant::now();
        r.store("evil.example.com", vec![], now);
        assert_eq!(r.cached("evil.example.com", now), Some(vec![]));
    }

    /// An expired entry is a miss.
    #[test]
    fn expired_entry_is_a_miss() {
        let r = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_millis(1));
        let t0 = Instant::now();
        r.store("api.example.com", vec![ip("93.184.216.34")], t0);
        let later = t0 + Duration::from_millis(50);
        assert_eq!(r.cached("api.example.com", later), None);
    }

    /// The cache never exceeds its cap, even with many distinct names.
    #[test]
    fn cache_is_bounded() {
        let r = resolver(false);
        let now = Instant::now();
        for i in 0..(EGRESS_CACHE_MAX_ENTRIES * 3) {
            r.store(&format!("host-{i}.example.com"), vec![ip("93.184.216.34")], now);
        }
        assert!(
            r.cache_len() <= EGRESS_CACHE_MAX_ENTRIES,
            "cache grew to {} entries, cap is {EGRESS_CACHE_MAX_ENTRIES}",
            r.cache_len()
        );
    }

    /// A cached non-empty result flows through `resolve()` as port-0 SocketAddrs:
    /// reqwest substitutes the URL's own port when it sees port 0.
    #[tokio::test]
    async fn resolve_returns_cached_addrs_with_port_zero() {
        use std::str::FromStr;

        let r = resolver(false);
        let now = Instant::now();
        r.store("api.example.com", vec![ip("93.184.216.34")], now);

        let name = Name::from_str("api.example.com").unwrap();
        let addrs: Vec<SocketAddr> = Resolve::resolve(&r, name).await.unwrap().collect();

        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip(), ip("93.184.216.34"));
        assert_eq!(addrs[0].port(), 0);
    }

    /// A cached empty result (all addresses blocked by the egress policy) makes
    /// `resolve()` return `Err` — the security semantic of the whole feature.
    #[tokio::test]
    async fn resolve_errors_when_cached_result_is_empty() {
        use std::str::FromStr;

        let r = resolver(true);
        let now = Instant::now();
        r.store("evil.example.com", vec![], now);

        let name = Name::from_str("evil.example.com").unwrap();
        let result = Resolve::resolve(&r, name).await;
        let Err(err) = result else {
            panic!("all-blocked cache entry must surface as an error");
        };

        assert!(
            err.to_string().contains("evil.example.com"),
            "error message should name the blocked host, got: {err}"
        );
    }

    /// On a cache hit, `resolve()` never touches the network. The resolver spec
    /// points at a TEST-NET-3 address (RFC 5737, guaranteed unreachable), so if the
    /// memo were bypassed this would fail or hang instead of returning promptly.
    #[tokio::test]
    async fn resolve_does_not_touch_network_on_cache_hit() {
        use std::str::FromStr;

        let r = EgressResolver::new(
            ResolverSpec::StandardDns {
                servers: vec!["203.0.113.1:53".to_string()],
            },
            EgressPolicy::default(),
            Duration::from_secs(5),
        );
        let now = Instant::now();
        r.store("cached.example.com", vec![ip("93.184.216.34")], now);

        let name = Name::from_str("cached.example.com").unwrap();
        let addrs: Vec<SocketAddr> =
            tokio::time::timeout(std::time::Duration::from_secs(2), Resolve::resolve(&r, name))
                .await
                .expect("resolve must hit the cache, not the network")
                .unwrap()
                .collect();

        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip(), ip("93.184.216.34"));
    }
}
