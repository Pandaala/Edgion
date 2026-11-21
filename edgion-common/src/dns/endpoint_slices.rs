//! Synthetic EndpointSlices for resolved `ExternalName` hostnames.
//!
//! One builder shared by the Gateway endpoint table and the Controller
//! resolver, so both publish the same shape: one slice per address family,
//! every endpoint ready, ports mirrored from the Service.

use std::collections::BTreeMap;
use std::net::IpAddr;

use edgion_resources::constants::labels::dns::{ENDPOINT_ORIGIN, ORIGIN_CONTROLLER, ORIGIN_GATEWAY};
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use k8s_openapi::api::core::v1::ServicePort;
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

/// Which plane built a synthetic DNS slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DnsSliceOrigin {
    Gateway,
    Controller,
}

impl DnsSliceOrigin {
    /// Value of the `edgion.io/endpoint-origin` label.
    pub fn label_value(self) -> &'static str {
        match self {
            Self::Gateway => ORIGIN_GATEWAY,
            Self::Controller => ORIGIN_CONTROLLER,
        }
    }

    fn name_infix(self) -> &'static str {
        match self {
            Self::Gateway => "gw",
            Self::Controller => "ctl",
        }
    }

    /// The origin recorded on `slice`, if it is a synthetic DNS slice.
    pub fn of(slice: &EndpointSlice) -> Option<Self> {
        match slice.metadata.labels.as_ref()?.get(ENDPOINT_ORIGIN)?.as_str() {
            ORIGIN_GATEWAY => Some(Self::Gateway),
            ORIGIN_CONTROLLER => Some(Self::Controller),
            _ => None,
        }
    }
}

/// Canonical form of a resolved address set: IPv4-mapped IPv6 folded to IPv4,
/// link-local and unspecified addresses dropped, duplicates removed, IPv4
/// ascending then IPv6 ascending.
///
/// The fixed order is what makes "the answer did not change" a plain equality
/// check, independent of the resolver's (often rotating) answer order.
/// Link-local answers (169.254.0.0/16, fe80::/10, e.g. cloud metadata),
/// answers in the process-wide operator blocked CIDR list (installed only by
/// the Gateway) and unspecified ones (0.0.0.0, ::, which a connect turns into
/// the local host) are never valid backend endpoints: dropping them before
/// publication keeps load balancing and active health probes away from them.
/// Loopback is kept; each Gateway's `allowLoopbackUpstream` decides at
/// selection time.
pub fn normalize_addrs(addrs: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
    use crate::egress::{classify_ip, IpClass};
    let mut out: Vec<IpAddr> = addrs
        .into_iter()
        .map(|ip| ip.to_canonical())
        .filter(|ip| !ip.is_unspecified() && !matches!(classify_ip(*ip), IpClass::LinkLocal | IpClass::BlockedCidr))
        .collect();
    out.sort_unstable_by_key(|ip| (ip.is_ipv6(), *ip));
    out.dedup();
    out
}

/// Deterministic slice name for one Service, origin and address family.
pub fn dns_slice_name(service_name: &str, origin: DnsSliceOrigin, is_ipv6: bool) -> String {
    let infix = origin.name_infix();
    if is_ipv6 {
        format!("{service_name}-edgion-{infix}-dns-ipv6")
    } else {
        format!("{service_name}-edgion-{infix}-dns")
    }
}

/// Build the synthetic slices for one Service from a normalized address set.
///
/// Returns one slice per address family present. Returns nothing when there
/// are no addresses or the Service declares no ports (a port-less slice is
/// unroutable). Ports mirror `ServicePort.port`, so selection keyed on the
/// route's backend port matches, and carry names / protocols for named-port
/// resolution.
pub fn build_dns_endpoint_slices(
    namespace: &str,
    service_name: &str,
    service_ports: &[ServicePort],
    addrs: &[IpAddr],
    origin: DnsSliceOrigin,
) -> Vec<EndpointSlice> {
    let ports: Vec<EndpointPort> = service_ports
        .iter()
        .map(|port| EndpointPort {
            name: port.name.clone(),
            port: Some(port.port),
            protocol: port.protocol.clone(),
            app_protocol: port.app_protocol.clone(),
        })
        .collect();
    if ports.is_empty() || addrs.is_empty() {
        return Vec::new();
    }
    let (v6, v4): (Vec<IpAddr>, Vec<IpAddr>) = addrs.iter().copied().partition(IpAddr::is_ipv6);
    [(v4, false), (v6, true)]
        .into_iter()
        .filter(|(family, _)| !family.is_empty())
        .map(|(family, is_ipv6)| build_one(namespace, service_name, &ports, &family, origin, is_ipv6))
        .collect()
}

fn build_one(
    namespace: &str,
    service_name: &str,
    ports: &[EndpointPort],
    addrs: &[IpAddr],
    origin: DnsSliceOrigin,
    is_ipv6: bool,
) -> EndpointSlice {
    let labels = BTreeMap::from([
        (SERVICE_NAME.to_string(), service_name.to_string()),
        (ENDPOINT_ORIGIN.to_string(), origin.label_value().to_string()),
    ]);
    let endpoints = addrs
        .iter()
        .map(|ip| Endpoint {
            addresses: vec![ip.to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                serving: Some(true),
                terminating: Some(false),
            }),
            ..Default::default()
        })
        .collect();
    EndpointSlice {
        metadata: ObjectMeta {
            name: Some(dns_slice_name(service_name, origin, is_ipv6)),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: if is_ipv6 { "IPv6" } else { "IPv4" }.to_string(),
        endpoints,
        ports: Some(ports.to_vec()),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/endpoint_slices_test.rs"]
mod tests;
