use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
}

fn port(name: Option<&str>, number: i32) -> ServicePort {
    ServicePort {
        name: name.map(str::to_string),
        port: number,
        protocol: Some("TCP".to_string()),
        ..Default::default()
    }
}

#[test]
fn normalize_sorts_dedups_and_folds_mapped_addresses() {
    let mapped = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 1).to_ipv6_mapped());
    let normalized = normalize_addrs([
        ip("2001:db8::2"),
        ip("10.0.0.2"),
        mapped,
        ip("10.0.0.1"),
        ip("2001:db8::1"),
        ip("10.0.0.2"),
    ]);
    assert_eq!(
        normalized,
        vec![ip("10.0.0.1"), ip("10.0.0.2"), ip("2001:db8::1"), ip("2001:db8::2")]
    );
}

#[test]
fn normalize_drops_link_local_but_keeps_loopback() {
    let mapped_metadata = IpAddr::V6(Ipv4Addr::new(169, 254, 169, 254).to_ipv6_mapped());
    let normalized = normalize_addrs([
        ip("169.254.169.254"),
        mapped_metadata,
        ip("fe80::1"),
        ip("127.0.0.1"),
        ip("10.0.0.1"),
    ]);
    assert_eq!(normalized, vec![ip("10.0.0.1"), ip("127.0.0.1")]);
}

#[test]
fn normalize_drops_unspecified_addresses() {
    let normalized = normalize_addrs([ip("0.0.0.0"), ip("::"), ip("10.0.0.1")]);
    assert_eq!(normalized, vec![ip("10.0.0.1")]);
}

#[test]
fn normalize_is_independent_of_answer_order() {
    let a = normalize_addrs([ip("1.1.1.3"), ip("1.1.1.1"), ip("1.1.1.2")]);
    let b = normalize_addrs([ip("1.1.1.2"), ip("1.1.1.3"), ip("1.1.1.1")]);
    assert_eq!(a, b);
}

#[test]
fn builds_one_slice_per_family_with_origin_label() {
    let slices = build_dns_endpoint_slices(
        "ns",
        "svc",
        &[port(Some("http"), 80)],
        &[ip("10.0.0.1"), ip("10.0.0.2"), IpAddr::V6(Ipv6Addr::LOCALHOST)],
        DnsSliceOrigin::Gateway,
    );
    assert_eq!(slices.len(), 2);
    let v4 = &slices[0];
    assert_eq!(v4.metadata.name.as_deref(), Some("svc-edgion-gw-dns"));
    assert_eq!(v4.metadata.namespace.as_deref(), Some("ns"));
    assert_eq!(v4.address_type, "IPv4");
    assert_eq!(v4.endpoints.len(), 2);
    assert_eq!(DnsSliceOrigin::of(v4), Some(DnsSliceOrigin::Gateway));
    let labels = v4.metadata.labels.as_ref().unwrap();
    assert_eq!(labels.get(SERVICE_NAME).map(String::as_str), Some("svc"));
    assert!(v4.metadata.annotations.is_none(), "no per-refresh timestamp");
    let ports = v4.ports.as_ref().unwrap();
    assert_eq!(ports[0].port, Some(80));
    assert_eq!(ports[0].name.as_deref(), Some("http"));
    for endpoint in &v4.endpoints {
        assert_eq!(endpoint.conditions.as_ref().unwrap().ready, Some(true));
    }
    let v6 = &slices[1];
    assert_eq!(v6.metadata.name.as_deref(), Some("svc-edgion-gw-dns-ipv6"));
    assert_eq!(v6.address_type, "IPv6");
}

#[test]
fn controller_origin_uses_its_own_names() {
    let slices = build_dns_endpoint_slices(
        "ns",
        "svc",
        &[port(None, 443)],
        &[ip("10.0.0.1")],
        DnsSliceOrigin::Controller,
    );
    assert_eq!(slices[0].metadata.name.as_deref(), Some("svc-edgion-ctl-dns"));
    assert_eq!(DnsSliceOrigin::of(&slices[0]), Some(DnsSliceOrigin::Controller));
}

#[test]
fn no_addresses_or_no_ports_builds_nothing() {
    assert!(build_dns_endpoint_slices("ns", "svc", &[port(None, 80)], &[], DnsSliceOrigin::Gateway).is_empty());
    assert!(build_dns_endpoint_slices("ns", "svc", &[], &[ip("10.0.0.1")], DnsSliceOrigin::Gateway).is_empty());
}

#[test]
fn identical_input_builds_identical_slices() {
    let build = || {
        build_dns_endpoint_slices(
            "ns",
            "svc",
            &[port(None, 80)],
            &[ip("10.0.0.1"), ip("10.0.0.2")],
            DnsSliceOrigin::Gateway,
        )
    };
    assert_eq!(build(), build());
}

#[test]
fn ordinary_slices_have_no_origin() {
    assert_eq!(DnsSliceOrigin::of(&EndpointSlice::default()), None);
}
