//! Shared Hickory resolver construction for explicit DNS servers.

use std::net::SocketAddr;

use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig, ResolverOpts};

/// Resolver options for the per-lookup resolvers built from an explicit server
/// list. Hickory's own response cache is disabled: each resolver serves a
/// single lookup, and answers are cached by `DnsAnswerCache`.
pub(super) fn resolver_options() -> ResolverOpts {
    let mut options = ResolverOpts::default();
    options.cache_size = 0;
    options
}

/// Build a resolver configuration that queries every server over UDP and TCP
/// while preserving each server's configured port.
pub(super) fn resolver_config(socket_addrs: &[SocketAddr]) -> ResolverConfig {
    let name_servers = socket_addrs
        .iter()
        .map(|addr| {
            let mut udp = ConnectionConfig::udp();
            udp.port = addr.port();
            let mut tcp = ConnectionConfig::tcp();
            tcp.port = addr.port();
            NameServerConfig::new(addr.ip(), true, vec![udp, tcp])
        })
        .collect();
    ResolverConfig::from_parts(None, vec![], name_servers)
}

#[cfg(test)]
#[path = "../../tests/unit/dns/hickory_test.rs"]
mod tests;
