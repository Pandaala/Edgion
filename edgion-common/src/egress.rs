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
        // A zero-TTL entry can never be fresh; avoid memo work entirely.
        let cache_enabled = !self.inner.ttl.is_zero();
        let cached = if cache_enabled {
            self.inner.cached(host, Instant::now())
        } else {
            None
        };
        let allowed = match cached {
            Some(hit) => hit,
            None => {
                let raw = dns_resolve(&self.inner.spec, host).await.map_err(ResolveFailure::Dns)?;
                let allowed = filter_resolved_addrs_with_observer(
                    raw,
                    self.inner.policy,
                    self.inner.allow_loopback,
                    self.inner.observer,
                );
                if cache_enabled {
                    self.inner.store(host, allowed.clone(), Instant::now());
                }
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
        .filter(|ip| is_ip_allowed_with_observer(*ip, policy, allow_loopback, observer))
        .collect()
}

/// Check one address with the same policy and rejection observation as batch filtering.
pub fn is_ip_allowed_with_observer(
    ip: IpAddr,
    policy: EgressPolicy,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
) -> bool {
    let reason = match classify_ip(ip) {
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
#[path = "../tests/unit/egress_test.rs"]
mod tests;
