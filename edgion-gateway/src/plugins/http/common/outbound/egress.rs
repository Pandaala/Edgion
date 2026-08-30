//! Gateway adapter for the backend-agnostic egress safety core.
//!
//! DNS resolution, caching, and IP classification live in
//! [`edgion_common::egress`]. This module retains Gateway-local defaults,
//! startup logging, and metric observation while preserving the existing
//! Gateway constructor API.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use reqwest::dns::{Name, Resolve, Resolving};

use edgion_common::dns::ResolverSpec;
use edgion_common::egress::{
    filter_resolved_addrs_with_observer, EgressBlockReason, EgressResolver as CoreEgressResolver,
};

use crate::observe::metrics::record_egress_blocked;

pub use edgion_common::egress::{EgressPolicy, EGRESS_CACHE_MAX_ENTRIES};

static EGRESS_BLOCK_PRIVATE: AtomicBool = AtomicBool::new(false);
static EGRESS_DNS_TTL_MS: AtomicU64 = AtomicU64::new(5_000);

/// Install Gateway-local egress defaults before outbound clients are built.
pub fn configure_egress_defaults(block_private: bool, dns_cache_ttl: Duration) {
    EGRESS_BLOCK_PRIVATE.store(block_private, Ordering::Relaxed);
    EGRESS_DNS_TTL_MS.store(dns_cache_ttl.as_millis() as u64, Ordering::Relaxed);
    tracing::info!(
        block_private,
        dns_cache_ttl_ms = dns_cache_ttl.as_millis() as u64,
        "Outbound egress defaults configured"
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_egress_defaults_updates_accessors() {
        configure_egress_defaults(true, Duration::from_millis(30_000));
        assert!(default_block_private());
        assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(30_000));

        configure_egress_defaults(false, Duration::from_millis(5_000));
        assert!(!default_block_private());
        assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(5_000));
    }
}
