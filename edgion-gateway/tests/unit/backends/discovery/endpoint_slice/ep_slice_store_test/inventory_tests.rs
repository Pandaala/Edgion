use super::super::super::coordinator::ServiceUnavailableReason;
use super::*;
use crate::backends::discovery::{ServicePublishedState, ServiceSectionInvalid};
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;

fn service(ports: Vec<ServicePort>, uid: Option<&str>) -> Arc<Service> {
    Arc::new(Service {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("backend".to_string()),
            uid: uid.map(str::to_string),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            ports: Some(ports),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn service_port(name: &str, port: i32, target_port: i32) -> ServicePort {
    ServicePort {
        name: Some(name.to_string()),
        port,
        protocol: Some("TCP".to_string()),
        target_port: Some(IntOrString::Int(target_port)),
        ..Default::default()
    }
}

fn endpoint_slice(
    name: &str,
    address_type: &str,
    addresses: &[(&str, bool)],
    ports: &[(&str, i32)],
) -> Arc<EndpointSlice> {
    Arc::new(EndpointSlice {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some(name.to_string()),
            uid: Some(format!("{name}-uid")),
            labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), "backend".to_string())])),
            ..Default::default()
        },
        address_type: address_type.to_string(),
        endpoints: addresses
            .iter()
            .map(|(address, ready)| Endpoint {
                addresses: vec![(*address).to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(*ready),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect(),
        ports: Some(
            ports
                .iter()
                .map(|(name, port)| EndpointPort {
                    name: Some((*name).to_string()),
                    port: Some(*port),
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                })
                .collect(),
        ),
    })
}

fn publish(coordinator: &ServiceDiscoveryCoordinator, ports: Vec<ServicePort>, slices: Vec<Arc<EndpointSlice>>) {
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(ports, Some("backend-uid")),
    )]));
    coordinator.replace_endpoint_slices(
        &slices
            .into_iter()
            .enumerate()
            .map(|(index, slice)| (format!("default/backend-{index}"), slice))
            .collect(),
    );
}

#[test]
fn service_port_selects_its_canonical_serving_port_not_a_sibling_number() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    publish(
        &coordinator,
        vec![service_port("https", 443, 8443), service_port("admin", 9443, 443)],
        vec![endpoint_slice(
            "backend-a",
            "IPv4",
            &[("10.0.0.1", true)],
            &[("admin", 443), ("https", 8443)],
        )],
    );
    let store = EpSliceStore::<RoundRobin>::new(coordinator);

    let endpoints = store.ready_endpoints_for_port("default/backend", Some(443));
    assert_eq!(endpoints, ["10.0.0.1:8443"]);
    assert!(!endpoints.iter().any(|endpoint| endpoint.ends_with(":443")));
}

#[test]
fn service_port_lookup_filters_readiness_formats_ips_and_refreshes_generation() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    publish(
        &coordinator,
        vec![service_port("https", 443, 8443)],
        vec![endpoint_slice(
            "backend-v4",
            "IPv4",
            &[("10.0.0.1", true), ("10.0.0.2", false)],
            &[("https", 8443)],
        )],
    );
    let store = EpSliceStore::<RoundRobin>::new(Arc::clone(&coordinator));
    assert_eq!(
        store.ready_endpoints_for_port("default/backend", Some(443)),
        ["10.0.0.1:8443"]
    );

    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-v6".to_string(),
        endpoint_slice(
            "backend-v6",
            "IPv6",
            &[("2001:db8::1", true), ("2001:db8::2", false)],
            &[("https", 9443)],
        ),
    )]));

    assert_eq!(
        store.ready_endpoints_for_port("default/backend", Some(443)),
        ["[2001:db8::1]:9443"]
    );

    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-unready".to_string(),
        endpoint_slice("backend-unready", "IPv4", &[("10.0.0.3", false)], &[("https", 8443)]),
    )]));
    assert!(store.ready_endpoints_for_port("default/backend", Some(443)).is_empty());
}

#[test]
fn invalid_missing_ambiguous_and_unavailable_sections_fail_closed() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    publish(
        &coordinator,
        vec![service_port("https", 443, 8443), service_port("duplicate", 443, 9443)],
        vec![endpoint_slice(
            "backend-a",
            "IPv4",
            &[("10.0.0.1", true)],
            &[("https", 8443), ("duplicate", 9443)],
        )],
    );
    let store = EpSliceStore::<RoundRobin>::new(Arc::clone(&coordinator));
    let section_key = ServiceSectionKey::Number(NonZeroU16::new(443).unwrap());
    assert!(matches!(
        coordinator
            .load_generation()
            .available("default/backend")
            .unwrap()
            .resolve_section(&section_key),
        Err(ServiceSectionInvalid::AmbiguousServicePort)
    ));
    for port in [None, Some(0), Some(-1), Some(65_536), Some(443)] {
        assert!(store.ready_endpoints_for_port("default/backend", port).is_empty());
    }
    assert!(store.ready_endpoints_for_port("default/missing", Some(443)).is_empty());

    let exhausted = Arc::new(ServiceDiscoveryCoordinator::new());
    exhausted.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        endpoint_slice("backend-a", "IPv4", &[("10.0.0.1", true)], &[("https", 8443)]),
    )]));
    exhausted.set_next_epoch_for_test(u64::MAX);
    exhausted.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(vec![service_port("https", 443, 8443)], None),
    )]));
    assert!(matches!(
        exhausted.load_generation().service("default/backend"),
        Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch
        ))
    ));
    let exhausted_store = EpSliceStore::<RoundRobin>::new(exhausted);
    assert!(exhausted_store
        .ready_endpoints_for_port("default/backend", Some(443))
        .is_empty());
}

#[test]
fn unavailable_slices_count_in_inventory_without_exposing_raw_or_backends() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    coordinator.set_next_epoch_for_test(u64::MAX);
    let make_slice = |name: &str| {
        Arc::new(EndpointSlice {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some(name.to_string()),
                labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), "backend".to_string())])),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: Vec::new(),
            ports: None,
        })
    };
    coordinator.replace_endpoint_slices(&HashMap::from([
        ("default/a".to_string(), make_slice("a")),
        ("default/b".to_string(), make_slice("b")),
    ]));
    let store = EpSliceStore::<RoundRobin>::new(coordinator);
    assert_eq!(store.slice_count(), 2);
    assert_eq!(store.service_index_count(), 0);
    assert!(store.get_slices_for_service("default/backend").is_none());
    assert!(store.get_backends_for_service("default/backend").is_empty());
    assert!(store.keys().is_empty());
}
