//! Standard DNS backend using hickory-resolver against an explicit server list.

use std::net::{IpAddr, SocketAddr};

use super::{hickory, require_addresses, strip_brackets, system, DnsError, DnsLookupFamily, DnsQueryPolicy};

/// Parse a `ip` or `ip:port` server string into a SocketAddr (default port 53).
/// Accepts a bracketed IPv6 literal without a port (e.g. `"[::1]"`).
fn parse_server(s: &str) -> Option<SocketAddr> {
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    let ip: IpAddr = strip_brackets(s).parse().ok()?;
    Some(SocketAddr::new(ip, 53))
}

/// Parse `servers` into distinct `SocketAddr`s, dropping any entry that fails
/// to parse. Pure and side-effect-free so the per-server-port grouping in
/// [`resolve_standard`] stays unit-testable without spinning up a resolver.
fn parse_servers(servers: &[String]) -> Vec<SocketAddr> {
    servers.iter().filter_map(|s| parse_server(s)).collect()
}

/// Resolve `domain` against the given DNS servers. Empty list ⇒ system resolver.
pub async fn resolve_standard(
    servers: &[String],
    domain: &str,
    policy: DnsQueryPolicy,
) -> Result<Vec<IpAddr>, DnsError> {
    if servers.is_empty() {
        let addrs = system::resolve_system(domain, policy)
            .await
            .map_err(|e| DnsError::Io(e.to_string()))?;
        return if addrs.is_empty() {
            Err(DnsError::NoAddresses)
        } else {
            Ok(addrs)
        };
    }

    use hickory_resolver::net::runtime::TokioRuntimeProvider;
    use hickory_resolver::net::{DnsError as HickoryDnsError, NetError};
    use hickory_resolver::TokioResolver;

    let socket_addrs = parse_servers(servers);
    if socket_addrs.is_empty() {
        return Err(DnsError::Io(format!("no valid DNS servers in {servers:?}")));
    }

    let config = hickory::resolver_config(&socket_addrs);
    let resolver = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .build()
        .map_err(|e| DnsError::Io(e.to_string()))?;

    if let DnsQueryPolicy::Family(family) = policy {
        use hickory_resolver::proto::{
            op::ResponseCode,
            rr::{RData, RecordType},
        };
        let query = async |record_type| match resolver.lookup(domain, record_type).await {
            Ok(records) => Ok(records
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    hickory_resolver::proto::rr::RData::A(a) => Some(IpAddr::V4(a.0)),
                    hickory_resolver::proto::rr::RData::AAAA(a) => Some(IpAddr::V6(a.0)),
                    _ => None,
                })
                .collect::<Vec<_>>()),
            // Hickory 0.26.1 also synthesizes NoError on exhausted CNAME chains,
            // possibly with an SOA but without preserved authorities. Only wire
            // negative responses retain matching SOA authority evidence. Treat
            // ambiguous no-SOA empties conservatively; recheck on dependency upgrades.
            Err(NetError::Dns(HickoryDnsError::NoRecordsFound(records)))
                if records.response_code == ResponseCode::NoError
                    && records.soa.as_ref().is_some_and(|soa| {
                        records.authorities.as_ref().is_some_and(|authorities| {
                            authorities.iter().any(|record| {
                                record.name == soa.name
                                    && record.dns_class == soa.dns_class
                                    && matches!(&record.data, RData::SOA(data) if data == &soa.data)
                            })
                        })
                    }) =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(DnsError::Io(error.to_string())),
        };
        let addresses = match family {
            DnsLookupFamily::Ipv4Only => query(RecordType::A).await?,
            DnsLookupFamily::Ipv6Only => query(RecordType::AAAA).await?,
            DnsLookupFamily::All => {
                let (v4, v6) = tokio::join!(query(RecordType::A), query(RecordType::AAAA));
                let mut out = v4?;
                out.extend(v6?);
                out
            }
        };
        return require_addresses(policy.select(addresses));
    }
    let lookup = resolver.lookup_ip(domain).await.map_err(|e| {
        if matches!(e, NetError::Dns(HickoryDnsError::NoRecordsFound(_))) {
            DnsError::NoAddresses
        } else {
            DnsError::Io(e.to_string())
        }
    })?;
    let out: Vec<IpAddr> = lookup.iter().collect();
    if out.is_empty() {
        Err(DnsError::NoAddresses)
    } else {
        Ok(out)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/standard_test.rs"]
mod tests;
