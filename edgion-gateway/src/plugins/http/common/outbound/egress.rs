//! Gateway adapter for the backend-agnostic egress safety core.
//!
//! DNS resolution, caching, and IP classification live in
//! [`edgion_common::egress`]. This module retains Gateway-local defaults,
//! startup logging, and metric observation while preserving the existing
//! Gateway constructor API.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use reqwest::dns::{Name, Resolve, Resolving};

use edgion_common::dns::{DnsAnswerCache, ResolverSpec};
use edgion_common::egress::{
    filter_resolved_addrs_with_observer, is_ip_allowed_with_observer, EgressBlockReason,
    EgressResolver as CoreEgressResolver,
};

use crate::observe::metrics::record_egress_blocked;

pub use edgion_common::egress::{EgressPolicy, ResolverAddressPolicy};

const REQUEST_EGRESS_CACHE_CAPACITY: usize = 1024;
const REQUEST_EGRESS_CACHE_SHARDS: usize = 8;

/// Answer cache for egress destinations a request or an upstream can name
/// (DSL `http.call` URLs and the redirects they follow). Kept apart from the
/// shared egress cache so such names cannot evict the answers of
/// operator-configured consumers (OIDC, LDAP, webhooks).
pub fn request_egress_answer_cache() -> &'static DnsAnswerCache {
    static CACHE: LazyLock<DnsAnswerCache> =
        LazyLock::new(|| DnsAnswerCache::new(REQUEST_EGRESS_CACHE_CAPACITY, REQUEST_EGRESS_CACHE_SHARDS));
    &CACHE
}

static EGRESS_BLOCK_PRIVATE: AtomicBool = AtomicBool::new(false);
static EGRESS_DNS_TTL_MS: AtomicU64 = AtomicU64::new(5_000);

fn duration_millis_saturating(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Install Gateway-local egress defaults before outbound clients are built.
pub fn configure_egress_defaults(block_private: bool, dns_cache_ttl: Duration) {
    let dns_cache_ttl_ms = duration_millis_saturating(dns_cache_ttl);
    EGRESS_BLOCK_PRIVATE.store(block_private, Ordering::Relaxed);
    EGRESS_DNS_TTL_MS.store(dns_cache_ttl_ms, Ordering::Relaxed);
    tracing::info!(block_private, dns_cache_ttl_ms, "Outbound egress defaults configured");
}

/// Compile and install the operator `outbound_egress_blocked_cidrs` list into
/// the shared address classifier. Runs once at startup, before any outbound
/// client or backend dial; an empty list installs nothing.
pub fn configure_egress_blocked_cidrs(cidrs: &[String]) -> Result<(), String> {
    let Some(matcher) = edgion_common::egress::compile_blocked_cidrs(cidrs)? else {
        return Ok(());
    };
    edgion_common::egress::install_blocked_cidrs(matcher)?;
    tracing::info!(count = cidrs.len(), "Outbound egress blocked CIDRs configured");
    Ok(())
}

pub fn default_block_private() -> bool {
    EGRESS_BLOCK_PRIVATE.load(Ordering::Relaxed)
}

pub fn egress_dns_cache_ttl() -> Duration {
    Duration::from_millis(EGRESS_DNS_TTL_MS.load(Ordering::Relaxed))
}

fn observe_block(reason: EgressBlockReason) {
    record_egress_blocked(reason.as_str());
}

/// Gateway resolver wrapper that attaches the canonical Gateway metric hook.
#[derive(Clone)]
pub struct EgressResolver {
    inner: CoreEgressResolver,
}

impl EgressResolver {
    pub fn new(spec: ResolverSpec, policy: EgressPolicy, ttl: Duration) -> Self {
        Self {
            inner: CoreEgressResolver::new_with_options(spec, policy, ttl, cfg!(test), Some(observe_block)),
        }
    }

    /// Construct a resolver for request- or upstream-influenced destination
    /// names, backed by [`request_egress_answer_cache`].
    pub fn new_request_influenced(spec: ResolverSpec, policy: ResolverAddressPolicy, ttl: Duration) -> Self {
        Self {
            inner: CoreEgressResolver::with_cache(
                spec,
                policy,
                ttl,
                cfg!(test),
                Some(observe_block),
                request_egress_answer_cache(),
            ),
        }
    }

    /// Construct the canonical resolver with the integration fixture's
    /// loopback exception. This is deliberately unavailable to production
    /// configuration and runtime call sites.
    #[cfg(feature = "test-util")]
    pub(crate) fn new_for_test(spec: ResolverSpec, policy: EgressPolicy, ttl: Duration) -> Self {
        Self {
            inner: CoreEgressResolver::new_with_options(spec, policy, ttl, true, Some(observe_block)),
        }
    }

    pub async fn resolve_socket_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        self.inner.resolve_socket_addrs(host, port).await
    }
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        self.inner.resolve(name)
    }
}

/// Gateway-observed form of the shared address filter.
pub(crate) fn filter_resolved_addrs(addrs: Vec<IpAddr>, policy: EgressPolicy, allow_loopback: bool) -> Vec<IpAddr> {
    filter_resolved_addrs_with_observer(addrs, policy, allow_loopback || cfg!(test), Some(observe_block))
}

/// Gateway-observed admission for a single address without a temporary address vector.
pub(crate) fn is_ip_allowed(ip: IpAddr, policy: EgressPolicy, allow_loopback: bool) -> bool {
    is_ip_allowed_with_observer(ip, policy, allow_loopback || cfg!(test), Some(observe_block))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/common/outbound/egress_test.rs"]
mod tests;
