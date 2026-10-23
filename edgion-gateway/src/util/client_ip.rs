//! Canonical extraction of the immutable direct peer and effective transport client.

use pingora_core::protocols::l4::socket::SocketAddr;
use std::net::SocketAddr as StdSocketAddr;

use crate::ctx::canonicalize_ip;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportClientAddrs {
    pub direct_peer_ip: String,
    pub direct_peer_port: u16,
    pub remote_ip: String,
    pub remote_port: u16,
}

/// Resolve both transport identities from Pingora's socket addresses.
///
/// `effective` is the ordinary peer or the source supplied by an accepted
/// PROXY header. `raw` is the physical previous hop and falls back to the
/// effective address only when the transport cannot expose it.
pub fn resolve_transport_client_addrs(
    effective: Option<&SocketAddr>,
    raw: Option<&SocketAddr>,
    missing_ip: &str,
) -> TransportClientAddrs {
    let effective = effective.and_then(SocketAddr::as_inet);
    let remote_ip = effective
        .map(|addr| canonicalize_ip(addr.ip()))
        .unwrap_or_else(|| missing_ip.to_string());
    let remote_port = effective.map_or(0, StdSocketAddr::port);

    let raw = raw.and_then(SocketAddr::as_inet);
    TransportClientAddrs {
        direct_peer_ip: match raw {
            Some(addr) if effective.map(StdSocketAddr::ip) != Some(addr.ip()) => canonicalize_ip(addr.ip()),
            // Equal IPs share formatting work, but retain independent strings and ports.
            _ => remote_ip.clone(),
        },
        direct_peer_port: raw.map_or(remote_port, StdSocketAddr::port),
        remote_ip,
        remote_port,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/util/client_ip_test.rs"]
mod tests;
