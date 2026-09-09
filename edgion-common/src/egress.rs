//! Backend-agnostic outbound egress safety.
//!
//! [`EgressResolver`] implements [`reqwest::dns::Resolve`] and classifies the
//! exact addresses an outbound connector is about to dial. Keeping this core in
//! `edgion-common` lets Controller and Gateway clients share one DNS-rebinding
//! and IP-classification policy without introducing a Controller -> Gateway
//! dependency.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::dns::DnsError;
use crate::dns::{resolve as dns_resolve, ResolverSpec};
use crate::test_mode::is_integration_testing_mode;

/// Hard cap on each resolver's address memo.
pub const EGRESS_CACHE_MAX_ENTRIES: usize = 16;

/// Coarse, policy-free IP segments used by outbound egress decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpClass {
    /// 127.0.0.0/8 or ::1.
    Loopback,
    /// 169.254.0.0/16 or fe80::/10.
    LinkLocal,
    /// RFC 1918 private IPv4 space.
    Private,
    /// RFC 4193 unique-local IPv6 space.
    UniqueLocal,
    /// 0.0.0.0 or ::.
    Unspecified,
    /// Any other address.
    Global,
}

/// Classify an IP address without making an allow/block decision.
///
/// IPv4-mapped IPv6 is normalized before IPv6 range checks so mapped loopback,
/// link-local, private, and unspecified addresses cannot bypass policy.
#[inline]
pub fn classify_ip(ip: IpAddr) -> IpClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                classify_v4(v4)
            } else if v6.is_loopback() {
                IpClass::Loopback
            } else if v6.is_unspecified() {
                IpClass::Unspecified
            } else {
                let first = v6.segments()[0];
                if (first & 0xfe00) == 0xfc00 {
                    IpClass::UniqueLocal
                } else if (first & 0xffc0) == 0xfe80 {
                    IpClass::LinkLocal
                } else {
                    IpClass::Global
                }
            }
        }
    }
}

#[inline]
fn classify_v4(ip: Ipv4Addr) -> IpClass {
    if ip.is_loopback() {
        IpClass::Loopback
    } else if ip.is_link_local() {
        IpClass::LinkLocal
    } else if ip.is_private() {
        IpClass::Private
    } else if ip.is_unspecified() {
        IpClass::Unspecified
    } else {
        IpClass::Global
    }
}

/// Low-cardinality reason emitted when an address is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressBlockReason {
    Loopback,
    LinkLocal,
    Unspecified,
    Private,
}

impl EgressBlockReason {
    /// Stable metric/log vocabulary used by the Gateway adapter.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::LinkLocal => "link_local",
            Self::Unspecified => "unspecified",
            Self::Private => "private",
        }
    }
}

/// Optional binary-specific observation hook. The common core itself emits no
/// metrics or logs.
pub type EgressBlockObserver = fn(EgressBlockReason);

/// Egress policy for one outbound client.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EgressPolicy {
    pub block_private: bool,
}

struct CacheEntry {
    addrs: Vec<IpAddr>,
    inserted_at: Instant,
}

impl CacheEntry {
    fn is_fresh(&self, now: Instant, ttl: Duration) -> bool {
        now.saturating_duration_since(self.inserted_at) < ttl
    }
}

struct Inner {
    spec: ResolverSpec,
    policy: EgressPolicy,
    ttl: Duration,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

/// DNS resolver that applies egress policy to every resolved address.
#[derive(Clone)]
pub struct EgressResolver {
    inner: Arc<Inner>,
}

enum ResolveFailure {
    Dns(DnsError),
    Blocked(String),
}

impl EgressResolver {
    /// Construct a resolver with production-safe loopback policy and no
    /// binary-specific observation hook.
    pub fn new(spec: ResolverSpec, policy: EgressPolicy, ttl: Duration) -> Self {
        Self::new_with_options(spec, policy, ttl, false, None)
    }

    /// Construct a resolver with explicit test/transport loopback handling and
    /// an optional binary-owned observation hook.
    pub fn new_with_options(
        spec: ResolverSpec,
        policy: EgressPolicy,
        ttl: Duration,
        allow_loopback: bool,
        observer: Option<EgressBlockObserver>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                spec,
                policy,
                ttl,
                allow_loopback,
                observer,
                cache: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Resolve and classify the exact socket addresses a non-reqwest transport
    /// will dial.
    pub async fn resolve_socket_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let allowed = self.resolve_ips(host).await.map_err(|failure| match failure {
            ResolveFailure::Dns(error) => format!("DNS resolution failed for outbound target: {error}"),
            ResolveFailure::Blocked(_) => "egress policy blocked all resolved addresses".to_string(),
        })?;
        Ok(allowed.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
    }

    async fn resolve_ips(&self, host: &str) -> Result<Vec<IpAddr>, ResolveFailure> {
        let now = Instant::now();
        let allowed = match self.inner.cached(host, now) {
            Some(hit) => hit,
            None => {
                let raw = dns_resolve(&self.inner.spec, host).await.map_err(ResolveFailure::Dns)?;
                let allowed = filter_resolved_addrs_with_observer(
                    raw,
                    self.inner.policy,
                    self.inner.allow_loopback,
                    self.inner.observer,
                );
                self.inner.store(host, allowed.clone(), Instant::now());
                allowed
            }
        };
        if allowed.is_empty() {
            return Err(ResolveFailure::Blocked(format!(
                "egress policy blocked all resolved addresses for host '{host}'"
            )));
        }
        Ok(allowed)
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
}

impl Inner {
    fn cached(&self, host: &str, now: Instant) -> Option<Vec<IpAddr>> {
        let guard = self.cache.read();
        let entry = guard.get(host)?;
        entry.is_fresh(now, self.ttl).then(|| entry.addrs.clone())
    }

    fn store(&self, host: &str, addrs: Vec<IpAddr>, now: Instant) {
        let mut guard = self.cache.write();
        guard.retain(|_, entry| entry.is_fresh(now, self.ttl));
        if guard.len() >= EGRESS_CACHE_MAX_ENTRIES && !guard.contains_key(host) {
            guard.clear();
        }
        guard.insert(
            host.to_string(),
            CacheEntry {
                addrs,
                inserted_at: now,
            },
        );
    }
}

/// Filter resolved addresses without binary-specific observations.
pub fn filter_resolved_addrs(addrs: Vec<IpAddr>, policy: EgressPolicy, allow_loopback: bool) -> Vec<IpAddr> {
    filter_resolved_addrs_with_observer(addrs, policy, allow_loopback, None)
}

/// Filter resolved addresses and notify the optional observer for each drop.
pub fn filter_resolved_addrs_with_observer(
    addrs: Vec<IpAddr>,
    policy: EgressPolicy,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
) -> Vec<IpAddr> {
    addrs
        .into_iter()
        .filter(|ip| {
            let reason = match classify_ip(*ip) {
                IpClass::Loopback if allow_loopback || cfg!(test) || is_integration_testing_mode() => return true,
                IpClass::Loopback => EgressBlockReason::Loopback,
                IpClass::LinkLocal => EgressBlockReason::LinkLocal,
                IpClass::Unspecified => EgressBlockReason::Unspecified,
                IpClass::Private | IpClass::UniqueLocal if policy.block_private => EgressBlockReason::Private,
                IpClass::Private | IpClass::UniqueLocal | IpClass::Global => return true,
            };
            if let Some(observe) = observer {
                observe(reason);
            }
            false
        })
        .collect()
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let allowed =
                resolver
                    .resolve_ips(&host)
                    .await
                    .map_err(|failure| -> Box<dyn std::error::Error + Send + Sync> {
                        match failure {
                            ResolveFailure::Dns(error) => Box::new(error),
                            ResolveFailure::Blocked(message) => message.into(),
                        }
                    })?;
            let addrs: Addrs = Box::new(allowed.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn resolver(block_private: bool) -> EgressResolver {
        EgressResolver::new(
            ResolverSpec::System,
            EgressPolicy { block_private },
            Duration::from_secs(5),
        )
    }

    #[test]
    fn classify_all_segments_and_ipv4_mapped_ipv6() {
        let cases = [
            ("127.0.0.1", IpClass::Loopback),
            ("::1", IpClass::Loopback),
            ("169.254.169.254", IpClass::LinkLocal),
            ("febf::1", IpClass::LinkLocal),
            ("10.0.0.1", IpClass::Private),
            ("172.31.255.255", IpClass::Private),
            ("192.168.1.1", IpClass::Private),
            ("fd00::1", IpClass::UniqueLocal),
            ("0.0.0.0", IpClass::Unspecified),
            ("::", IpClass::Unspecified),
            ("8.8.8.8", IpClass::Global),
            ("2001:db8::1", IpClass::Global),
            ("::ffff:127.0.0.1", IpClass::Loopback),
            ("::ffff:169.254.169.254", IpClass::LinkLocal),
            ("::ffff:192.168.1.1", IpClass::Private),
            ("::ffff:0.0.0.0", IpClass::Unspecified),
            ("::ffff:8.8.8.8", IpClass::Global),
        ];
        for (value, expected) in cases {
            assert_eq!(classify_ip(ip(value)), expected, "classify_ip({value})");
        }
    }

    #[test]
    fn filter_enforces_link_local_unspecified_and_private_policy() {
        let input = vec![
            ip("169.254.169.254"),
            ip("0.0.0.0"),
            ip("10.1.2.3"),
            ip("fd00::1"),
            ip("93.184.216.34"),
        ];
        assert_eq!(
            filter_resolved_addrs(input.clone(), EgressPolicy::default(), false),
            vec![ip("10.1.2.3"), ip("fd00::1"), ip("93.184.216.34")]
        );
        assert_eq!(
            filter_resolved_addrs(input, EgressPolicy { block_private: true }, false),
            vec![ip("93.184.216.34")]
        );
    }

    #[test]
    fn cache_hit_negative_expiry_and_bound() {
        let resolver = resolver(true);
        let now = Instant::now();
        resolver.store("api.example.com", vec![ip("93.184.216.34")], now);
        resolver.store("evil.example.com", vec![], now);
        assert_eq!(resolver.cached("api.example.com", now), Some(vec![ip("93.184.216.34")]));
        assert_eq!(resolver.cached("evil.example.com", now), Some(vec![]));

        let short = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_millis(1));
        short.store("expired.example.com", vec![ip("93.184.216.34")], now);
        assert_eq!(
            short.cached("expired.example.com", now + Duration::from_millis(50)),
            None
        );

        for index in 0..(EGRESS_CACHE_MAX_ENTRIES * 3) {
            resolver.store(&format!("host-{index}.example.com"), vec![ip("93.184.216.34")], now);
        }
        assert!(resolver.cache_len() <= EGRESS_CACHE_MAX_ENTRIES);
    }

    #[test]
    fn cache_freshness_is_strict_at_ttl_boundary_and_pruning_matches() {
        let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_millis(10));
        let inserted_at = Instant::now();
        let address = ip("93.184.216.34");
        resolver.store("old.example.com", vec![address], inserted_at);

        assert_eq!(
            resolver.cached("old.example.com", inserted_at + Duration::from_millis(9)),
            Some(vec![address])
        );
        let boundary = inserted_at + Duration::from_millis(10);
        assert_eq!(resolver.cached("old.example.com", boundary), None);

        resolver.store("new.example.com", vec![address], boundary);
        assert_eq!(resolver.cache_len(), 1);
        assert_eq!(resolver.cached("old.example.com", boundary), None);
        assert_eq!(resolver.cached("new.example.com", boundary), Some(vec![address]));
    }

    #[test]
    fn zero_ttl_expires_immediately_in_lookup_and_pruning() {
        let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::ZERO);
        let now = Instant::now();
        let address = ip("93.184.216.34");
        resolver.store("old.example.com", vec![address], now);
        assert_eq!(resolver.cached("old.example.com", now), None);

        resolver.store("new.example.com", vec![address], now);
        assert_eq!(resolver.cache_len(), 1);
        assert_eq!(resolver.cached("old.example.com", now), None);
        assert_eq!(resolver.cached("new.example.com", now), None);
    }

    #[test]
    fn cache_freshness_handles_reversed_samples_and_huge_ttl() {
        let address = ip("93.184.216.34");
        let earlier = Instant::now();
        let later = earlier + Duration::from_millis(10);
        let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_secs(1));
        resolver.store("future.example.com", vec![address], later);
        assert_eq!(resolver.cached("future.example.com", earlier), Some(vec![address]));
        resolver.store("current.example.com", vec![address], earlier);
        assert_eq!(resolver.cache_len(), 2);

        let huge = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::MAX);
        huge.store("huge.example.com", vec![address], earlier);
        assert_eq!(
            huge.cached("huge.example.com", earlier + Duration::from_secs(1)),
            Some(vec![address])
        );
    }

    #[tokio::test]
    async fn reqwest_resolve_uses_cached_port_zero_and_rejects_empty() {
        let resolver = resolver(true);
        let now = Instant::now();
        resolver.store("api.example.com", vec![ip("93.184.216.34")], now);
        let name = Name::from_str("api.example.com").unwrap();
        let addrs: Vec<_> = Resolve::resolve(&resolver, name).await.unwrap().collect();
        assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);

        resolver.store("evil.example.com", vec![], now);
        let name = Name::from_str("evil.example.com").unwrap();
        let result = Resolve::resolve(&resolver, name).await;
        let Err(error) = result else {
            panic!("all-blocked cache entry must surface as an error");
        };
        assert!(error.to_string().contains("evil.example.com"));
    }

    #[tokio::test]
    async fn cache_hit_does_not_touch_network() {
        let resolver = EgressResolver::new(
            ResolverSpec::StandardDns {
                servers: vec!["203.0.113.1:53".to_string()],
            },
            EgressPolicy::default(),
            Duration::from_secs(5),
        );
        resolver.store("cached.example.com", vec![ip("93.184.216.34")], Instant::now());
        let name = Name::from_str("cached.example.com").unwrap();
        let addrs: Vec<_> = tokio::time::timeout(Duration::from_secs(2), Resolve::resolve(&resolver, name))
            .await
            .expect("cache hit must not touch DNS")
            .unwrap()
            .collect();
        assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);
    }
}
