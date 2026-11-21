//! Backend-agnostic outbound egress safety.
//!
//! [`EgressResolver`] implements [`reqwest::dns::Resolve`] and classifies the
//! exact addresses an outbound connector is about to dial. Keeping this core in
//! `edgion-common` lets Controller and Gateway clients share one DNS-rebinding
//! and IP-classification policy without introducing a Controller -> Gateway
//! dependency.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::dns::{egress_answer_cache, DnsAnswerCache, DnsCachePolicy, DnsError, DnsQueryPolicy, ResolverSpec};
use crate::test_mode::is_integration_testing_mode;

/// Coarse IP segments used by outbound egress decisions.
///
/// Every class is a fixed address segment except [`IpClass::BlockedCidr`],
/// which reflects the process-wide operator list installed with
/// [`install_blocked_cidrs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpClass {
    /// 127.0.0.0/8 or ::1.
    Loopback,
    /// 169.254.0.0/16 or fe80::/10.
    LinkLocal,
    /// Matched the operator-configured blocked CIDR list. Never produced for
    /// loopback or link-local addresses, which keep their built-in classes.
    BlockedCidr,
    /// RFC 1918 private IPv4 or RFC 6598 shared address space.
    Private,
    /// RFC 4193 unique-local IPv6 space.
    UniqueLocal,
    /// 0.0.0.0 or ::.
    Unspecified,
    /// An invalid unicast destination or an invalid NAT64/6to4 embedded address.
    NonDialable,
    /// Any other address.
    Global,
}

impl IpClass {
    /// Rejections shared even by consumers that allow local and private targets.
    pub fn always_blocked_reason(self) -> Option<EgressBlockReason> {
        match self {
            Self::BlockedCidr => Some(EgressBlockReason::BlockedCidr),
            Self::NonDialable => Some(EgressBlockReason::NonDialable),
            Self::Loopback | Self::LinkLocal | Self::Private | Self::UniqueLocal | Self::Unspecified | Self::Global => {
                None
            }
        }
    }
}

/// Process-wide operator-configured blocked CIDRs. Installed at most once, at
/// Gateway startup before any outbound dial; the Controller never installs it.
static BLOCKED_CIDRS: OnceLock<IpRadixMatcher> = OnceLock::new();

/// Group label shared by every operator-configured blocked CIDR.
const BLOCKED_CIDR_GROUP: &str = "blocked-cidr";

/// Compile an operator-configured blocked CIDR list.
///
/// Every entry must use `address/prefix` notation. Returns `Ok(None)` for an
/// empty list. An invalid entry is an error so the owning binary fails startup
/// instead of silently running with a weaker list.
pub fn compile_blocked_cidrs<S: AsRef<str>>(cidrs: &[S]) -> Result<Option<IpRadixMatcher>, String> {
    if cidrs.is_empty() {
        return Ok(None);
    }
    let mut builder = IpRadixMatcher::builder();
    for cidr in cidrs {
        let cidr = cidr.as_ref();
        builder
            .insert(cidr, BLOCKED_CIDR_GROUP)
            .map_err(|error| format!("invalid blocked CIDR {cidr:?}: {error}"))?;
    }
    builder
        .build()
        .map(Some)
        .map_err(|error| format!("invalid blocked CIDR list: {error}"))
}

/// Install the process-wide blocked CIDR matcher consulted by [`classify_ip`].
///
/// Must run once, before any outbound client is built or any backend is
/// dialed. A second installation is rejected.
pub fn install_blocked_cidrs(matcher: IpRadixMatcher) -> Result<(), String> {
    BLOCKED_CIDRS
        .set(matcher)
        .map_err(|_| "egress blocked CIDR list is already installed".to_string())
}

/// Whether startup installed a non-empty operator list.
pub fn has_blocked_cidrs() -> bool {
    BLOCKED_CIDRS.get().is_some()
}

/// Classify an IP address without making an allow/block decision.
///
/// IPv4-mapped IPv6 is normalized before IPv6 range checks so mapped loopback,
/// link-local, private, and unspecified addresses cannot bypass policy. The
/// process-wide blocked CIDR list, when installed, is applied as described in
/// [`classify_ip_with`].
#[inline]
pub fn classify_ip(ip: IpAddr) -> IpClass {
    classify_ip_with(ip, BLOCKED_CIDRS.get())
}

/// Classify an IP address against an explicit blocked CIDR matcher.
///
/// Loopback and link-local keep their built-in classes, so the list can only
/// add blocks and never changes their reasons or exceptions. Any other address
/// covered by `blocked` (including either representation of a well-known
/// NAT64 or 6to4 address, and IPv4-mapped forms) is
/// [`IpClass::BlockedCidr`].
#[inline]
pub fn classify_ip_with(ip: IpAddr, blocked: Option<&IpRadixMatcher>) -> IpClass {
    let class = match ip {
        IpAddr::V6(v6) if let Some(v4) = transition_ipv4(v6) => {
            if blocked.is_some_and(|matcher| matcher.contains(&IpAddr::V4(v4))) {
                return IpClass::BlockedCidr;
            }
            if is_global_transition_endpoint(v4) {
                IpClass::Global
            } else {
                IpClass::NonDialable
            }
        }
        _ => classify_builtin(ip),
    };
    match class {
        IpClass::Loopback | IpClass::LinkLocal => class,
        _ if blocked.is_some_and(|matcher| matcher.contains(&ip)) => IpClass::BlockedCidr,
        _ => class,
    }
}

/// RFC 6052 well-known /96 and RFC 3056 6to4 /16 use different IPv4 offsets.
fn transition_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    match ip.segments() {
        [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] | [0x2002, hi, lo, ..] => {
            Some(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)))
        }
        _ => None,
    }
}

/// Transition endpoints require global unicast. These additional IANA
/// special-purpose blocks remain available as native IPv4 for local uses.
fn is_global_transition_endpoint(ip: Ipv4Addr) -> bool {
    if classify_v4(ip) != IpClass::Global || ip.is_documentation() {
        return false;
    }
    match ip.octets() {
        [192, 0, 0, host] => matches!(host, 9 | 10), // Globally reachable protocol anycasts
        [192, 88, 99, _] | [198, 18..=19, _, _] => false, // Deprecated relay and benchmarking
        _ => true,
    }
}

#[inline]
fn classify_builtin(ip: IpAddr) -> IpClass {
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
    } else if ip.is_private() || matches!(ip.octets(), [100, 64..=127, _, _]) {
        IpClass::Private
    } else if ip.is_unspecified() {
        IpClass::Unspecified
    } else if matches!(ip.octets()[0], 0 | 224..=255) {
        IpClass::NonDialable
    } else {
        IpClass::Global
    }
}

/// Low-cardinality reason emitted when an address is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressBlockReason {
    Loopback,
    LinkLocal,
    BlockedCidr,
    Unspecified,
    NonDialable,
    Private,
}

impl EgressBlockReason {
    /// Stable metric/log vocabulary used by the Gateway adapter.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::LinkLocal => "link_local",
            Self::BlockedCidr => "blocked_cidr",
            Self::Unspecified => "unspecified",
            Self::NonDialable => "non_dialable",
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

/// Which address restrictions apply to the exact DNS answers used for a dial.
#[derive(Clone, Copy)]
pub enum ResolverAddressPolicy {
    Standard(EgressPolicy),
    /// Keep local and private addresses, subject to configured CIDRs and
    /// the unconditional non-dialable class.
    AllowPrivateAndLocal,
}

impl From<EgressPolicy> for ResolverAddressPolicy {
    fn from(policy: EgressPolicy) -> Self {
        Self::Standard(policy)
    }
}

struct Inner {
    spec: ResolverSpec,
    policy: ResolverAddressPolicy,
    cache_policy: DnsCachePolicy,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
    cache: &'static DnsAnswerCache,
}

/// DNS resolver that applies egress policy to every resolved address.
#[derive(Clone)]
pub struct EgressResolver {
    inner: Arc<Inner>,
}

/// OS resolver for clients that permit scoped local sockets and require an
/// uncached answer on every dial. The socket and its interface ID stay together.
#[derive(Clone, Copy)]
pub struct UncachedLocalSystemResolver {
    observer: Option<EgressBlockObserver>,
}

impl UncachedLocalSystemResolver {
    pub fn new(observer: Option<EgressBlockObserver>) -> Self {
        Self { observer }
    }

    fn allowed_sockets(&self, sockets: impl Iterator<Item = SocketAddr>) -> Vec<SocketAddr> {
        let mut seen = std::collections::HashSet::new();
        sockets
            .filter(|addr| seen.insert(*addr))
            .take(crate::dns::MAX_ANSWER_ADDRS)
            .filter(|addr| {
                is_ip_allowed_with_observer(
                    addr.ip(),
                    ResolverAddressPolicy::AllowPrivateAndLocal,
                    false,
                    self.observer,
                )
            })
            .collect()
    }
}

impl Resolve for UncachedLocalSystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = *self;
        Box::pin(async move {
            let sockets = crate::dns::system::lookup_system_sockets((name.as_str().to_owned(), 0)).await?;
            let allowed = resolver.allowed_sockets(sockets);
            if allowed.is_empty() {
                return Err("egress policy blocked all resolved addresses".into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

enum ResolveFailure {
    Dns(DnsError),
    Blocked,
}

impl EgressResolver {
    /// Construct a resolver with production-safe loopback policy and no
    /// binary-specific observation hook.
    ///
    /// `ttl` is the freshness of an answer whose resolver reports no record TTL;
    /// reported TTLs are honored within the shared answer cache's bounds. A zero
    /// `ttl` disables caching for this resolver.
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
        Self::with_cache(spec, policy, ttl, allow_loopback, observer, egress_answer_cache())
    }

    /// Construct a resolver backed by an explicit answer cache instance. A
    /// consumer whose destination names are request- or upstream-influenced
    /// uses its own instance so those names cannot evict the answers that
    /// operator-configured consumers share.
    pub fn with_cache(
        spec: ResolverSpec,
        policy: impl Into<ResolverAddressPolicy>,
        ttl: Duration,
        allow_loopback: bool,
        observer: Option<EgressBlockObserver>,
        cache: &'static DnsAnswerCache,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                spec,
                policy: policy.into(),
                cache_policy: DnsCachePolicy::new(ttl),
                allow_loopback,
                observer,
                cache,
            }),
        }
    }

    /// Resolve and classify the exact socket addresses a non-reqwest transport
    /// will dial.
    pub async fn resolve_socket_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let allowed = self.resolve_ips(host).await.map_err(|failure| match failure {
            ResolveFailure::Dns(error) => format!("DNS resolution failed for outbound target: {error}"),
            ResolveFailure::Blocked => "egress policy blocked all resolved addresses".to_string(),
        })?;
        Ok(allowed.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
    }

    /// Resolve through the shared answer cache, then classify the exact
    /// addresses about to be dialed. The cache holds raw answers, so the
    /// policy and observer of this resolver run on every use.
    async fn resolve_ips(&self, host: &str) -> Result<Vec<IpAddr>, ResolveFailure> {
        let raw = self
            .inner
            .cache
            .lookup(&self.inner.spec, host, DnsQueryPolicy::Native, &self.inner.cache_policy)
            .await
            .map_err(ResolveFailure::Dns)?;
        let allowed =
            filter_resolved_addrs_with_observer(raw, self.inner.policy, self.inner.allow_loopback, self.inner.observer);
        if allowed.is_empty() {
            return Err(ResolveFailure::Blocked);
        }
        Ok(allowed)
    }
}

/// Filter resolved addresses without binary-specific observations.
pub fn filter_resolved_addrs(addrs: Vec<IpAddr>, policy: EgressPolicy, allow_loopback: bool) -> Vec<IpAddr> {
    filter_resolved_addrs_with_observer(addrs, policy, allow_loopback, None)
}

/// Filter resolved addresses and notify the optional observer for each drop.
pub fn filter_resolved_addrs_with_observer(
    addrs: Vec<IpAddr>,
    policy: impl Into<ResolverAddressPolicy>,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
) -> Vec<IpAddr> {
    let policy = policy.into();
    addrs
        .into_iter()
        .filter(|ip| is_ip_allowed_with_observer(*ip, policy, allow_loopback, observer))
        .collect()
}

/// Check one address with the same policy and rejection observation as batch filtering.
pub fn is_ip_allowed_with_observer(
    ip: IpAddr,
    policy: impl Into<ResolverAddressPolicy>,
    allow_loopback: bool,
    observer: Option<EgressBlockObserver>,
) -> bool {
    let class = classify_ip(ip);
    let reason = match policy.into() {
        ResolverAddressPolicy::Standard(policy) => egress_block_reason(class, policy, allow_loopback),
        ResolverAddressPolicy::AllowPrivateAndLocal => class.always_blocked_reason(),
    };
    let Some(reason) = reason else {
        return true;
    };
    if let Some(observe) = observer {
        observe(reason);
    }
    false
}

/// Map one address class to its egress rejection reason, or `None` when allowed.
fn egress_block_reason(class: IpClass, policy: EgressPolicy, allow_loopback: bool) -> Option<EgressBlockReason> {
    match class {
        IpClass::Loopback if allow_loopback || cfg!(test) || is_integration_testing_mode() => None,
        IpClass::Loopback => Some(EgressBlockReason::Loopback),
        IpClass::LinkLocal => Some(EgressBlockReason::LinkLocal),
        IpClass::BlockedCidr => Some(EgressBlockReason::BlockedCidr),
        IpClass::Unspecified => Some(EgressBlockReason::Unspecified),
        IpClass::NonDialable => Some(EgressBlockReason::NonDialable),
        IpClass::Private | IpClass::UniqueLocal if policy.block_private => Some(EgressBlockReason::Private),
        IpClass::Private | IpClass::UniqueLocal | IpClass::Global => None,
    }
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.clone();
        Box::pin(async move {
            let allowed = resolver.resolve_ips(name.as_str()).await.map_err(
                |failure| -> Box<dyn std::error::Error + Send + Sync> {
                    match failure {
                        ResolveFailure::Dns(error) => Box::new(error),
                        ResolveFailure::Blocked => format!(
                            "egress policy blocked all resolved addresses for host '{}'",
                            name.as_str()
                        )
                        .into(),
                    }
                },
            )?;
            let addrs: Addrs = Box::new(allowed.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/egress_test.rs"]
mod tests;
