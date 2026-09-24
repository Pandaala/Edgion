//! A `reqwest::dns::Resolve` that pins every lookup to an anchor Service's
//! endpoints.
//!
//! The composed upstream authority is never resolved in DNS: whatever name reqwest
//! asks for, this returns the anchor Service's ready endpoint addresses. That is
//! what keeps a request-influenced authority from becoming a request-influenced
//! destination. Spec §5.3 step 3.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::backends::discovery::endpoint_slice::get_roundrobin_store;

/// Pins a single-endpoint exchange without resolving the TLS hostname in DNS.
/// Classification runs again when the HTTP connector asks for a dial address.
#[derive(Debug)]
pub(crate) struct EndpointPinnedResolver {
    address: SocketAddr,
    allow_loopback: bool,
}

impl EndpointPinnedResolver {
    pub(crate) fn new(address: SocketAddr, allow_loopback: bool) -> Self {
        Self {
            address,
            allow_loopback,
        }
    }
}

impl Resolve for EndpointPinnedResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let address = self.address;
        let allowed = crate::backends::validate_endpoint_backend_addr(
            &pingora_core::protocols::l4::socket::SocketAddr::Inet(address),
            self.allow_loopback,
        )
        .is_ok();
        Box::pin(async move {
            if !allowed {
                return Err("unsafe upstream".into());
            }
            let addrs: Addrs = Box::new(std::iter::once(address));
            Ok(addrs)
        })
    }
}

/// Resolves ANY name to the anchor Service's ready endpoints.
#[derive(Debug)]
pub struct AnchorPinnedResolver {
    service_key: String,
    port: Option<i32>,
    cursor: AtomicUsize,
}

impl AnchorPinnedResolver {
    pub fn new(service_key: String, port: Option<i32>) -> Self {
        Self {
            service_key,
            port,
            cursor: AtomicUsize::new(0),
        }
    }

    /// Current ready endpoints, rotated so successive calls spread NEW
    /// connection establishments across them. reqwest pools keep-alive
    /// connections per host and only re-resolves when it needs to open a new
    /// one, so this rotation spreads new-connection placement the way
    /// `ResolvedTarget::pick_url` does for IP-based URLs — it does not, by
    /// itself, spread steady-state request load across already-open
    /// connections.
    fn addrs(&self) -> Vec<SocketAddr> {
        let eps = get_roundrobin_store().ready_endpoints_for_port(&self.service_key, self.port);
        if eps.is_empty() {
            return Vec::new();
        }
        let mut parsed: Vec<SocketAddr> = eps
            .iter()
            .filter_map(|endpoint| endpoint.parse().ok())
            .filter(|address| {
                crate::backends::validate_endpoint_backend_addr(
                    &pingora_core::protocols::l4::socket::SocketAddr::Inet(*address),
                    false,
                )
                .is_ok()
            })
            .collect();
        if parsed.is_empty() {
            return Vec::new();
        }
        let shift = self.cursor.fetch_add(1, Ordering::Relaxed) % parsed.len();
        parsed.rotate_left(shift);
        parsed
    }

    #[cfg(test)]
    pub fn addrs_for_test(&self) -> Vec<SocketAddr> {
        self.addrs()
    }
}

impl Resolve for AnchorPinnedResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        // The queried name is deliberately ignored — that is the pin.
        let addrs = self.addrs();
        Box::pin(async move {
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/common/outbound/pinned_resolver_test.rs"]
mod tests;
