use super::*;
use edgion_common::dns::endpoint_slices::{build_dns_endpoint_slices, DnsSliceOrigin};
use k8s_openapi::api::core::v1::ServicePort;

fn slices(addresses: &[&str]) -> Vec<EndpointSlice> {
    let ports = [ServicePort {
        port: 80,
        ..Default::default()
    }];
    let addresses: Vec<std::net::IpAddr> = addresses.iter().map(|addr| addr.parse().unwrap()).collect();
    build_dns_endpoint_slices("ns", "svc", &ports, &addresses, DnsSliceOrigin::Controller)
}

#[test]
fn replace_reports_only_changed_keys() {
    let store = DnsSyntheticSlices::new();
    assert_eq!(
        store.replace_service(0, "ns/svc", slices(&["10.0.0.1"])),
        vec!["ns/svc-edgion-ctl-dns".to_string()]
    );
    assert!(store.replace_service(0, "ns/svc", slices(&["10.0.0.1"])).is_empty());
    assert_eq!(
        store.replace_service(0, "ns/svc", slices(&["10.0.0.1", "2001:db8::1"])),
        vec!["ns/svc-edgion-ctl-dns-ipv6".to_string()]
    );
    assert_eq!(
        store.replace_service(0, "ns/svc", slices(&["2001:db8::1"])),
        vec!["ns/svc-edgion-ctl-dns".to_string()],
        "a vanished family is a removal"
    );
    assert!(store.get("ns/svc-edgion-ctl-dns").is_none());
    assert!(store.get("ns/svc-edgion-ctl-dns-ipv6").is_some());
}

#[test]
fn withdraw_and_clear_leave_nothing_behind() {
    let store = DnsSyntheticSlices::new();
    store.replace_service(0, "ns/svc", slices(&["10.0.0.1", "2001:db8::1"]));
    assert_eq!(store.len(), 2);
    assert_eq!(store.replace_service(0, "ns/svc", Vec::new()).len(), 2);
    assert_eq!(store.len(), 0);
    assert!(store.service_keys().is_empty());
    assert!(store.replace_service(0, "ns/svc", Vec::new()).is_empty());

    store.replace_service(0, "ns/svc", slices(&["10.0.0.1"]));
    store.clear();
    assert_eq!(store.len(), 0);
    assert!(store.service_keys().is_empty());
}

#[test]
fn writes_from_a_task_older_than_the_last_clear_are_rejected() {
    let store = DnsSyntheticSlices::new();
    let old_epoch = store.epoch();
    store.clear();
    assert!(store
        .replace_service(old_epoch, "ns/svc", slices(&["10.0.0.1"]))
        .is_empty());
    assert!(store.service_keys().is_empty(), "stale writer changed nothing");
    let current = store.epoch();
    assert_ne!(current, old_epoch);
    assert_eq!(store.replace_service(current, "ns/svc", slices(&["10.0.0.1"])).len(), 1);
}
