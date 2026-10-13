use super::*;
use crate::backends::outlier_detection::{FailureClass, OutlierDetectionStore};
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
use k8s_openapi::api::core::v1::ServiceSpec;
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions};
use kube::api::ObjectMeta;

fn service(uid: Option<&str>, target_port: IntOrString) -> Arc<Service> {
    Arc::new(Service {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("backend".to_string()),
            uid: uid.map(str::to_string),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            ports: Some(vec![ServicePort {
                name: Some("http".to_string()),
                port: 80,
                protocol: Some("TCP".to_string()),
                target_port: Some(target_port),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn slice_ports(ports: &[(&str, i32)]) -> Vec<EndpointPort> {
    ports
        .iter()
        .map(|(name, port)| EndpointPort {
            name: if name.is_empty() {
                None
            } else {
                Some((*name).to_string())
            },
            port: Some(*port),
            protocol: Some("TCP".to_string()),
            ..Default::default()
        })
        .collect()
}

fn slice(name: &str, uid: Option<&str>, ready: bool) -> Arc<EndpointSlice> {
    Arc::new(EndpointSlice {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some(name.to_string()),
            uid: uid.map(str::to_string),
            labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), "backend".to_string())])),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        endpoints: vec![Endpoint {
            addresses: vec!["10.0.0.1".to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(ready),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ports: Some(slice_ports(&[("http", 8080)])),
    })
}

fn named_service(name: &str, uid: &str) -> Arc<Service> {
    let mut value = (*service(Some(uid), IntOrString::String("web".to_string()))).clone();
    value.metadata.name = Some(name.to_string());
    Arc::new(value)
}

fn owned_slice(name: &str, uid: &str, service_name: &str, address: &str) -> Arc<EndpointSlice> {
    let mut value = (*slice(name, Some(uid), true)).clone();
    value.metadata.labels = Some(BTreeMap::from([(SERVICE_NAME.to_string(), service_name.to_string())]));
    value.endpoints[0].addresses = vec![address.to_string()];
    Arc::new(value)
}

fn publish_base(coordinator: &ServiceDiscoveryCoordinator) {
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String("web".to_string())),
    )]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        slice("backend-a", Some("slice-a"), true),
    )]));
}

fn health_policy(generation: u64) -> Arc<HealthPolicyIdentity> {
    Arc::new(HealthPolicyIdentity { generation })
}

struct ActiveHealthFixture {
    coordinator: ServiceDiscoveryCoordinator,
    policy: Arc<HealthPolicyIdentity>,
    backend: Arc<LbBackend>,
}

impl ActiveHealthFixture {
    fn new() -> Self {
        let coordinator = ServiceDiscoveryCoordinator::new();
        let policy = health_policy(1);
        coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy)));
        publish_base(&coordinator);
        let backend = Arc::clone(
            &coordinator
                .load_generation()
                .available("default/backend")
                .unwrap()
                .canonical_probe_backends()[0],
        );
        Self {
            coordinator,
            policy,
            backend,
        }
    }

    fn complete(&self, success: bool, healthy: u32, unhealthy: u32) {
        assert!(self.coordinator.record_health_completion(
            &self.policy,
            self.backend.runtime_membership_identity().unwrap(),
            self.backend.runtime_identity().unwrap(),
            success,
            healthy,
            unhealthy
        ));
    }

    fn eligible(&self) -> bool {
        self.coordinator
            .runtime_cell("default/backend")
            .unwrap()
            .active
            .load()
            .entries[self.backend.runtime_identity().unwrap().as_ref()]
        .health_eligible()
    }
}

#[test]
fn fresh_canonical_endpoint_is_health_eligible() {
    let fixture = ActiveHealthFixture::new();
    assert!(fixture.eligible());
    fixture.complete(true, 2, 3);
    assert!(fixture.eligible());
}

#[test]
fn test_consecutive_failures_trigger_unhealthy() {
    let fixture = ActiveHealthFixture::new();
    for _ in 0..2 {
        fixture.complete(false, 2, 3);
        assert!(fixture.eligible());
    }
    fixture.complete(false, 2, 3);
    assert!(!fixture.eligible());
}

#[test]
fn test_consecutive_successes_trigger_healthy() {
    let fixture = ActiveHealthFixture::new();
    fixture.complete(false, 2, 1);
    assert!(!fixture.eligible());
    fixture.complete(true, 2, 1);
    assert!(!fixture.eligible());
    fixture.complete(true, 2, 1);
    assert!(fixture.eligible());
}

#[test]
fn test_success_resets_failure_counter() {
    let fixture = ActiveHealthFixture::new();
    fixture.complete(false, 2, 3);
    fixture.complete(false, 2, 3);
    fixture.complete(true, 2, 3);
    for _ in 0..2 {
        fixture.complete(false, 2, 3);
        assert!(fixture.eligible());
    }
    fixture.complete(false, 2, 3);
    assert!(!fixture.eligible());
}

#[test]
fn test_failure_resets_success_counter() {
    let fixture = ActiveHealthFixture::new();
    fixture.complete(false, 2, 1);
    fixture.complete(true, 2, 1);
    fixture.complete(false, 2, 1);
    fixture.complete(true, 2, 1);
    assert!(!fixture.eligible());
    fixture.complete(true, 2, 1);
    assert!(fixture.eligible());
}

#[test]
fn test_cross_service_backend_isolation() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let first_policy = health_policy(1);
    let second_policy = health_policy(2);
    coordinator.set_health_policy("default/first", Some(Arc::clone(&first_policy)));
    coordinator.set_health_policy("default/second", Some(Arc::clone(&second_policy)));
    coordinator.replace_services(&HashMap::from([
        ("default/first".to_string(), named_service("first", "first-uid")),
        ("default/second".to_string(), named_service("second", "second-uid")),
    ]));
    coordinator.replace_endpoint_slices(&HashMap::from([
        (
            "default/first-a".to_string(),
            owned_slice("first-a", "first-slice", "first", "10.0.0.1"),
        ),
        (
            "default/second-a".to_string(),
            owned_slice("second-a", "second-slice", "second", "10.0.0.1"),
        ),
    ]));
    let generation = coordinator.load_generation();
    let first = &generation
        .available("default/first")
        .unwrap()
        .canonical_probe_backends()[0];
    let second = &generation
        .available("default/second")
        .unwrap()
        .canonical_probe_backends()[0];
    assert_eq!(first.addr, second.addr);
    assert_ne!(first.runtime_identity(), second.runtime_identity());
    assert!(coordinator.record_health_completion(
        &first_policy,
        first.runtime_membership_identity().unwrap(),
        first.runtime_identity().unwrap(),
        false,
        1,
        1
    ));
    assert!(
        !coordinator.runtime_cell("default/first").unwrap().active.load().entries
            [first.runtime_identity().unwrap().as_ref()]
        .health_eligible()
    );
    assert!(coordinator
        .runtime_cell("default/second")
        .unwrap()
        .active
        .load()
        .entries[second.runtime_identity().unwrap().as_ref()]
    .health_eligible());
    assert!(coordinator.record_health_completion(
        &second_policy,
        second.runtime_membership_identity().unwrap(),
        second.runtime_identity().unwrap(),
        false,
        1,
        1
    ));
    assert!(!coordinator
        .runtime_cell("default/second")
        .unwrap()
        .active
        .load()
        .entries[second.runtime_identity().unwrap().as_ref()]
    .health_eligible());
}

#[test]
fn degrade_threshold_resolver_runs_under_coordinator_lock() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    publish_base(&coordinator);
    let service = coordinator
        .load_generation()
        .available("default/backend")
        .cloned()
        .expect("base service must be available");
    let before = service.runtime_cell.active.load_full();

    let lock_probe = Arc::clone(&coordinator);
    coordinator.refresh_degrade_threshold_with("default/backend", move || {
        assert!(
            lock_probe.state.try_lock().is_none(),
            "degrade threshold must resolve after acquiring coordinator state"
        );
        73
    });

    let after = service.runtime_cell.active.load_full();
    assert_eq!(after.degrade_threshold, 73);
    assert_eq!(after.membership_identity, before.membership_identity);
    assert!(Arc::ptr_eq(&after.entries, &before.entries));
    assert_eq!(after.health_policy_present, before.health_policy_present);
    assert_eq!(after.outlier_policy_present, before.outlier_policy_present);
    assert!(before.degrade_selections.is_none());
    assert!(after.degrade_selections.is_none());
}

#[test]
fn direct_endpoint_mapping_is_section_scoped_and_includes_unready_slots() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut backend_service = (*service(Some("service-uid"), IntOrString::Int(8443))).clone();
    let ports = backend_service.spec.as_mut().unwrap().ports.as_mut().unwrap();
    ports[0].port = 443;
    ports.push(ServicePort {
        name: Some("admin".to_string()),
        port: 9443,
        target_port: Some(IntOrString::Int(9000)),
        ..Default::default()
    });
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        Arc::new(backend_service),
    )]));
    let mut endpoints = (*slice("backend-a", Some("slice-a"), false)).clone();
    endpoints.ports = Some(slice_ports(&[("http", 8443), ("admin", 9000)]));
    coordinator.replace_endpoint_slices(&HashMap::from([("default/backend-a".to_string(), Arc::new(endpoints))]));
    let section = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(443).unwrap()))
        .unwrap();
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(section.ready_slots.is_empty());
    assert_eq!(section.direct_endpoint_addr(ip, None), Some(SocketAddr::new(ip, 8443)));
    assert_eq!(
        section.direct_endpoint_addr(ip, Some(8443)),
        Some(SocketAddr::new(ip, 8443))
    );
    for port in [0, 443, 9000, 9443] {
        assert_eq!(section.direct_endpoint_addr(ip, Some(port)), None);
    }
    assert_eq!(section.direct_endpoint_addr("10.0.0.2".parse().unwrap(), None), None);
}

#[test]
fn direct_endpoint_index_preserves_per_ip_ports_and_captured_generations() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let old_generation = coordinator.load_generation();
    let key = ServiceSectionKey::Number(NonZeroU16::new(80).unwrap());
    let old_section = old_generation
        .available("default/backend")
        .unwrap()
        .resolve_section(&key)
        .unwrap();
    let mut replacement = (*slice("backend-a", Some("slice-a"), true)).clone();
    replacement.ports = Some(slice_ports(&[("http", 8181)]));
    let mut sibling = (*slice("backend-b", Some("slice-b"), true)).clone();
    sibling.endpoints[0].addresses = vec!["10.0.0.2".to_string()];
    coordinator.replace_endpoint_slices(&HashMap::from([
        ("default/backend-a".to_string(), Arc::new(replacement)),
        ("default/backend-b".to_string(), Arc::new(sibling)),
    ]));
    let new_section = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .resolve_section(&key)
        .unwrap();
    let first: IpAddr = "10.0.0.1".parse().unwrap();
    let second: IpAddr = "10.0.0.2".parse().unwrap();
    assert_eq!(
        old_section.direct_endpoint_addr(first, None),
        Some(SocketAddr::new(first, 8080))
    );
    assert_eq!(old_section.direct_endpoint_addr(second, None), None);
    assert_eq!(
        new_section.direct_endpoint_addr(first, None),
        Some(SocketAddr::new(first, 8181))
    );
    assert_eq!(new_section.direct_endpoint_addr(first, Some(8080)), None);
    assert_eq!(
        new_section.direct_endpoint_addr(second, None),
        Some(SocketAddr::new(second, 8080))
    );
    assert_eq!(new_section.direct_endpoint_addr(second, Some(8181)), None);
}

#[test]
fn direct_endpoint_ipv6_uses_numeric_identity_and_deduplicates_origins() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let mut endpoints = (*slice("backend-a", Some("slice-a"), true)).clone();
    endpoints.address_type = "IPv6".to_string();
    endpoints.endpoints[0].addresses = vec!["2001:db8:0:0:0:0:0:1".to_string(), "2001:db8::1".to_string()];
    coordinator.replace_endpoint_slices(&HashMap::from([("default/backend-a".to_string(), Arc::new(endpoints))]));
    let section = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap();
    let ip: IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(section.all_slots.len(), 1);
    assert_eq!(section.direct_endpoint_addr(ip, None), Some(SocketAddr::new(ip, 8080)));
    assert_eq!(
        section.direct_endpoint_addr(ip, Some(8080)),
        Some(SocketAddr::new(ip, 8080))
    );
}

#[test]
fn canonical_aliases_merge_equivalent_origins_and_missing_is_typed_invalid() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String("web".to_string())),
    )]));
    coordinator.replace_endpoint_slices(&HashMap::from([
        (
            "default/backend-a".to_string(),
            slice("backend-a", Some("slice-a"), false),
        ),
        (
            "default/backend-b".to_string(),
            slice("backend-b", Some("slice-b"), true),
        ),
    ]));
    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").expect("available service");
    let by_number = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .expect("number alias");
    let by_name = available
        .resolve_section(&ServiceSectionKey::Name(Arc::from("http")))
        .expect("name alias");
    assert!(Arc::ptr_eq(&by_number, &by_name));
    assert_eq!(by_number.all_slots.len(), 1);
    assert_eq!(by_number.ready_slots.len(), 1);
    assert_eq!(by_number.all_slots[0].origin_set.len(), 2);
    assert_eq!(generation.stats.valid_sections, 1);
    assert_eq!(generation.stats.canonical_slots, 1);
    assert!(matches!(
        available.resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(81).unwrap())),
        Err(ServiceSectionInvalid::MissingRequestedPort)
    ));
}

#[test]
fn published_app_protocol_comes_from_service_port() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut annotated = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    annotated.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some("kubernetes.io/h2c".to_string());
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(annotated))]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        slice("backend-a", Some("slice-a"), true),
    )]));
    let section = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap();
    assert_eq!(section.identity.app_protocol.as_deref(), Some("kubernetes.io/h2c"));
}

#[test]
fn named_target_port_distinct_from_service_port_name_resolves() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String("http".to_string())),
    )]));
    let mut renamed = (*service(Some("service-uid"), IntOrString::String("http".to_string()))).clone();
    renamed.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].name = Some("http-web".to_string());
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(renamed))]));
    let mut named_slice = (*slice("backend-a", Some("slice-a"), true)).clone();
    named_slice.ports = Some(slice_ports(&[("http-web", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        Arc::new(named_slice),
    )]));

    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").expect("available service");
    let section = available
        .resolve_section(&ServiceSectionKey::Name(Arc::from("http-web")))
        .expect("section matches Service port name, not targetPort name");
    assert_eq!(section.all_slots.len(), 1);
    assert_eq!(section.all_slots[0].origin_set[0].serving_port.get(), 8080);
}

#[test]
fn unnamed_single_service_port_matches_empty_or_unique_endpoint_port() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut unnamed = (*service(Some("service-uid"), IntOrString::Int(8080))).clone();
    unnamed.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].name = None;
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(unnamed))]));
    let mut unnamed_slice = (*slice("backend-a", Some("slice-a"), true)).clone();
    unnamed_slice.ports = Some(slice_ports(&[("", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        Arc::new(unnamed_slice),
    )]));
    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").expect("available service");
    available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .expect("unnamed Service port matches unnamed EndpointPort");

    let mut unique_named = (*slice("backend-a", Some("slice-a"), true)).clone();
    unique_named.ports = Some(slice_ports(&[("container-http", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        Arc::new(unique_named),
    )]));
    let unique_generation = coordinator.load_generation();
    unique_generation
        .available("default/backend")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .expect("unnamed single-port Service accepts the unique EndpointPort");
}

#[test]
fn sibling_slice_without_named_port_does_not_invalidate_section() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String("web".to_string())),
    )]));
    let named_slice = slice("backend-a", Some("slice-a"), true);
    let mut unnamed_sibling = (*slice("backend-b", Some("slice-b"), true)).clone();
    unnamed_sibling.ports = Some(slice_ports(&[("", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([
        ("default/backend-a".to_string(), named_slice),
        ("default/backend-b".to_string(), Arc::new(unnamed_sibling)),
    ]));

    let generation = coordinator.load_generation();
    let section = generation
        .available("default/backend")
        .expect("available service")
        .resolve_section(&ServiceSectionKey::Name(Arc::from("http")))
        .expect("named slice still resolves when a sibling omits the Service port name");
    assert_eq!(section.all_slots.len(), 1);
    assert_eq!(section.all_slots[0].origin_set[0].serving_port.get(), 8080);
}

#[test]
fn external_name_port_edit_requires_repaired_slice_name_and_protocol() {
    for field in ["name", "protocol"] {
        let coordinator = ServiceDiscoveryCoordinator::new();
        let mut source = (*service(Some("service-uid"), IntOrString::Int(8080))).clone();
        let spec = source.spec.as_mut().unwrap();
        spec.type_ = Some("ExternalName".into());
        spec.external_name = Some("backend.example.com".into());
        let port = &mut spec.ports.as_mut().unwrap()[0];
        if field == "name" {
            port.name = Some("renamed".into());
        } else {
            port.protocol = Some("UDP".into());
        }
        let desired = EndpointPort {
            name: port.name.clone(),
            port: Some(port.port),
            protocol: port.protocol.clone(),
            app_protocol: port.app_protocol.clone(),
        };
        coordinator.replace_services(&HashMap::from([("default/backend".into(), Arc::new(source))]));
        let mut published = (*slice("backend-edgion-dns", Some("slice-uid"), true)).clone();
        published.ports.as_mut().unwrap()[0].port = Some(80);
        coordinator.replace_endpoint_slices(&HashMap::from([(
            "default/backend-edgion-dns".into(),
            Arc::new(published.clone()),
        )]));
        let key = ServiceSectionKey::Number(NonZeroU16::new(80).unwrap());
        let stale = coordinator
            .load_generation()
            .available("default/backend")
            .unwrap()
            .resolve_section(&key)
            .unwrap();
        assert!(stale.all_slots.is_empty(), "stale {field}");
        published.ports = Some(vec![desired]);
        coordinator.replace_endpoint_slices(&HashMap::from([(
            "default/backend-edgion-dns".into(),
            Arc::new(published),
        )]));
        let repaired = coordinator
            .load_generation()
            .available("default/backend")
            .unwrap()
            .resolve_section(&key)
            .unwrap();
        assert_eq!(repaired.ready_slots.len(), 1, "repaired {field}");
        assert_eq!(repaired.all_slots[0].origin_set[0].serving_port.get(), 80);
    }
}

#[test]
fn same_ip_distinct_slice_ports_are_separate_slots() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut unnamed = (*service(Some("service-uid"), IntOrString::Int(8080))).clone();
    unnamed.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].name = None;
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(unnamed))]));
    let mut first = (*slice("backend-a", Some("slice-a"), true)).clone();
    first.ports = Some(slice_ports(&[("grpc", 30021)]));
    let mut second = (*slice("backend-b", Some("slice-b"), true)).clone();
    second.ports = Some(slice_ports(&[("grpc", 30022)]));
    coordinator.replace_endpoint_slices(&HashMap::from([
        ("default/backend-a".to_string(), Arc::new(first)),
        ("default/backend-b".to_string(), Arc::new(second)),
    ]));

    let section = coordinator
        .load_generation()
        .available("default/backend")
        .expect("available service")
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .expect("same IP with distinct serving ports is a hostNetwork-style topology, not a conflict");
    let ports: Vec<u16> = section
        .all_slots
        .iter()
        .map(|slot| slot.origin_set[0].serving_port.get())
        .collect();
    assert_eq!(ports, vec![30021, 30022]);
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    assert_eq!(section.direct_endpoint_addr(ip, None), None);
    for port in [30021, 30022] {
        assert_eq!(
            section.direct_endpoint_addr(ip, Some(port)),
            Some(SocketAddr::new(ip, port))
        );
    }
    assert_eq!(section.direct_endpoint_addr(ip, Some(8080)), None);
}

#[test]
fn annotation_only_update_does_not_publish_or_rotate_identity() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let first = coordinator.load_generation();
    let first_backend = Arc::clone(&first.available("default/backend").unwrap().canonical_probe_backends()[0]);
    let first_identity = Arc::clone(first_backend.runtime_identity().unwrap());

    let mut annotated = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    annotated.metadata.annotations = Some(BTreeMap::from([(
        "kubectl.kubernetes.io/last-applied-configuration".to_string(),
        "{\"kind\":\"Service\"}".to_string(),
    )]));
    annotated.metadata.labels = Some(BTreeMap::from([("env".to_string(), "prod".to_string())]));
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([("default/backend".to_string(), Arc::new(annotated))]),
        HashSet::new(),
    );
    let second = coordinator.load_generation();
    let second_backend = Arc::clone(&second.available("default/backend").unwrap().canonical_probe_backends()[0]);
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first_backend, &second_backend));
    assert!(Arc::ptr_eq(&first_identity, second_backend.runtime_identity().unwrap()));
}

#[test]
fn dns_resolver_annotation_change_republishes_raw_service() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut first = (*service(Some("service-uid"), IntOrString::Int(8080))).clone();
    first.spec.as_mut().unwrap().type_ = Some("ExternalName".to_string());
    first.spec.as_mut().unwrap().external_name = Some("flapbackend.test".to_string());
    first.metadata.annotations = Some(BTreeMap::from([(
        DNS_RESOLVER_REF.to_string(),
        "edgion-default/dns-flap-dead".to_string(),
    )]));
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(first))]));
    let first_generation = coordinator.load_generation();
    let first_service = Arc::clone(&first_generation.available("default/backend").unwrap().raw.service);

    let mut updated = (*first_service).clone();
    updated.metadata.annotations = Some(BTreeMap::from([(
        DNS_RESOLVER_REF.to_string(),
        "edgion-default/dns-flap-local".to_string(),
    )]));
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([("default/backend".to_string(), Arc::new(updated))]),
        HashSet::new(),
    );
    let second_generation = coordinator.load_generation();
    let second_service = Arc::clone(&second_generation.available("default/backend").unwrap().raw.service);
    assert!(!Arc::ptr_eq(&first_generation, &second_generation));
    assert_eq!(
        second_service
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(DNS_RESOLVER_REF))
            .map(String::as_str),
        Some("edgion-default/dns-flap-local")
    );
}

#[test]
fn malformed_service_port_publishes_typed_invalid_for_each_addressable_alias() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String(String::new())),
    )]));
    let generation = coordinator.load_generation();
    let available = generation
        .available("default/backend")
        .expect("source remains decodable");
    assert!(matches!(
        available.resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap())),
        Err(ServiceSectionInvalid::InvalidServicePort)
    ));
    assert!(matches!(
        available.resolve_section(&ServiceSectionKey::Name(Arc::from("http"))),
        Err(ServiceSectionInvalid::InvalidServicePort)
    ));
}

#[test]
fn canonical_sections_keep_same_socket_runtime_state_isolated() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut service = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    service
        .spec
        .as_mut()
        .unwrap()
        .ports
        .as_mut()
        .unwrap()
        .push(ServicePort {
            name: Some("metrics".to_string()),
            port: 81,
            protocol: Some("TCP".to_string()),
            target_port: Some(IntOrString::String("web".to_string())),
            ..Default::default()
        });
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(service))]));
    let mut dual = (*slice("backend-a", Some("slice-a"), true)).clone();
    dual.ports = Some(slice_ports(&[("http", 8080), ("metrics", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([("default/backend-a".to_string(), Arc::new(dual))]));

    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").unwrap();
    let first = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap();
    let second = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(81).unwrap()))
        .unwrap();
    let first_backend = &first.ready_slots[0].backend;
    let second_backend = &second.ready_slots[0].backend;
    assert_eq!(first_backend.addr, second_backend.addr);
    assert!(!Arc::ptr_eq(first_backend, second_backend));
    let first_identity = first_backend.runtime_identity().expect("section slot is identified");
    let second_identity = second_backend.runtime_identity().expect("section slot is identified");
    assert_ne!(first_identity.as_ref(), second_identity.as_ref());
    let active = available.runtime_cell.active.load_full();
    assert!(active.entries.contains_key(first_identity.as_ref()));
    assert!(active.entries.contains_key(second_identity.as_ref()));
    assert_eq!(active.entries.len(), 2);
    assert_eq!(available.canonical_probe_backends().len(), 2);
    let http_alias = available
        .resolve_section(&ServiceSectionKey::Name(Arc::from("http")))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &http_alias));
    assert_eq!(
        available.canonical_probe_backends().len(),
        2,
        "number/name aliases of one section are probed once; two sections stay two identities"
    );
    for selectors in [&first.selectors, &second.selectors] {
        assert_eq!(selectors.round_robin.backend_count(), 1);
        assert_eq!(selectors.consistent_hash.backend_count(), 1);
        assert_eq!(selectors.least_conn.backend_count(), 1);
        assert_eq!(selectors.ewma.backend_count(), 1);
    }
}

#[test]
fn source_identity_survives_same_live_update_but_not_recreate() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("uid-a"), IntOrString::String("web".to_string())),
    )]));
    let first = coordinator
        .state
        .lock()
        .services
        .get("default/backend")
        .unwrap()
        .identity
        .clone();
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([(
            "default/backend".to_string(),
            service(Some("uid-a"), IntOrString::Int(8080)),
        )]),
        HashSet::new(),
    );
    let updated = coordinator
        .state
        .lock()
        .services
        .get("default/backend")
        .unwrap()
        .identity
        .clone();
    assert_eq!(first, updated);
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([(
            "default/backend".to_string(),
            service(Some("uid-b"), IntOrString::Int(8080)),
        )]),
        HashSet::new(),
    );
    let recreated = coordinator
        .state
        .lock()
        .services
        .get("default/backend")
        .unwrap()
        .identity
        .clone();
    assert_ne!(updated, recreated);
}

#[test]
fn semantic_noop_does_not_publish_and_resource_version_is_ignored() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let before = coordinator.load_generation();
    let mut same = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    same.metadata.resource_version = Some("99".to_string());
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([("default/backend".to_string(), Arc::new(same))]),
        HashSet::new(),
    );
    assert!(Arc::ptr_eq(&before, &coordinator.load_generation()));
}

#[test]
fn partial_update_normalizes_overlapping_add_update_remove_with_update_winning() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let key = "default/backend".to_string();
    coordinator.update_services(
        HashMap::from([(
            key.clone(),
            service(Some("uid-a"), IntOrString::String("ignored".to_string())),
        )]),
        HashMap::from([(
            key.clone(),
            service(Some("uid-b"), IntOrString::String("web".to_string())),
        )]),
        HashSet::from([key.clone()]),
    );
    let state = coordinator.state.lock();
    let stored = &state.services[&key];
    assert_eq!(stored.resource.metadata.uid.as_deref(), Some("uid-b"));
    assert!(stored.identity.is_some());
    drop(state);
    assert_eq!(coordinator.load_generation().stats.service_sources, 1);
}

#[test]
fn endpoint_slice_owner_update_rebuilds_both_services_and_retires_old_runtime() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let old_key = "default/old".to_string();
    let new_key = "default/new".to_string();
    let slice_key = "default/shared".to_string();
    let policy = health_policy(1);
    coordinator.set_health_policy(&old_key, Some(Arc::clone(&policy)));
    coordinator.replace_services(&HashMap::from([
        (old_key.clone(), named_service("old", "old-service-uid")),
        (new_key.clone(), named_service("new", "new-service-uid")),
    ]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        slice_key.clone(),
        owned_slice("shared", "slice-uid", "old", "10.0.0.1"),
    )]));

    let before = coordinator.load_generation();
    let old_projection = before.available(&old_key).unwrap();
    let old_backend = Arc::clone(&old_projection.canonical_probe_backends()[0]);
    let old_identity = Arc::clone(old_backend.runtime_identity().unwrap());
    let old_membership = old_backend.runtime_membership_identity().unwrap();
    let old_cell = Arc::clone(&old_projection.runtime_cell);
    let old_active = old_cell.active.load_full();
    assert!(coordinator.record_health_completion(&policy, old_membership, &old_identity, false, 1, 1));

    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(slice_key, owned_slice("shared", "slice-uid", "new", "10.0.0.2"))]),
        HashSet::new(),
    );

    let after = coordinator.load_generation();
    let old_projection = after.available(&old_key).unwrap();
    assert!(old_projection.raw.endpoint_slices.is_empty());
    assert!(old_projection.canonical_probe_backends().is_empty());
    assert!(old_cell.active.load().entries.is_empty());
    assert!(!old_active.entries[old_identity.as_ref()]
        .attached
        .load(Ordering::Acquire));
    assert!(!coordinator.record_health_completion(&policy, old_membership, &old_identity, true, 1, 1));

    let new_projection = after.available(&new_key).unwrap();
    assert_eq!(new_projection.raw.endpoint_slices.len(), 1);
    assert_eq!(new_projection.canonical_probe_backends().len(), 1);
    assert_eq!(
        new_projection.canonical_probe_backends()[0]
            .inner()
            .addr
            .as_inet()
            .map(ToString::to_string),
        Some("10.0.0.2:8080".to_string())
    );
    assert_eq!(after.stats.endpoint_slice_sources, 1);
}

#[test]
fn endpoint_slice_compressed_recreate_removes_missing_old_owner_projection() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let old_key = "default/old".to_string();
    let new_key = "default/new".to_string();
    let slice_key = "default/shared".to_string();
    coordinator.replace_services(&HashMap::from([
        (old_key.clone(), named_service("old", "old-service-uid")),
        (new_key.clone(), named_service("new", "new-service-uid")),
    ]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        slice_key.clone(),
        owned_slice("shared", "old-slice-uid", "old", "10.0.0.1"),
    )]));
    assert!(coordinator.runtime_cell(&old_key).is_some());

    coordinator.update_services(HashMap::new(), HashMap::new(), HashSet::from([old_key.clone()]));
    assert!(matches!(
        coordinator.load_generation().service(&old_key),
        Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::MissingService
        ))
    ));
    assert!(coordinator.runtime_cell(&old_key).is_none());

    coordinator.update_endpoint_slices(
        HashMap::from([(
            slice_key.clone(),
            owned_slice("shared", "new-slice-uid", "new", "10.0.0.2"),
        )]),
        HashMap::new(),
        HashSet::from([slice_key]),
    );

    let generation = coordinator.load_generation();
    assert!(generation.service(&old_key).is_none());
    assert!(coordinator.runtime_cell(&old_key).is_none());
    let new_projection = generation.available(&new_key).unwrap();
    assert_eq!(new_projection.raw.endpoint_slices.len(), 1);
    assert_eq!(new_projection.canonical_probe_backends().len(), 1);
    assert_eq!(generation.stats.endpoint_slice_sources, 1);
}

#[test]
fn late_health_completion_cannot_mutate_replacement_identity() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let policy = health_policy(1);
    coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy)));
    publish_base(&coordinator);
    let old_generation = coordinator.load_generation();
    let old_backend = Arc::clone(
        &old_generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let old_identity = Arc::clone(old_backend.runtime_identity().unwrap());
    let old_membership = old_backend.runtime_membership_identity().unwrap();
    assert!(coordinator.record_health_completion(&policy, old_membership, &old_identity, false, 1, 1));
    let old_cell = coordinator.runtime_cell("default/backend").unwrap();
    let old_active = old_cell.active.load_full();
    assert!(!old_active.entries.get(old_identity.as_ref()).unwrap().health_eligible());

    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(
            "default/backend-a".to_string(),
            slice("backend-a", Some("slice-b"), true),
        )]),
        HashSet::new(),
    );
    let new_generation = coordinator.load_generation();
    let new_backend = Arc::clone(
        &new_generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let new_identity = Arc::clone(new_backend.runtime_identity().unwrap());
    assert_ne!(old_identity, new_identity);
    assert!(Arc::ptr_eq(
        &old_backend.runtime_counters(),
        &new_backend.runtime_counters()
    ));
    assert!(!coordinator.record_health_completion(&policy, old_membership, &old_identity, true, 1, 1));
    assert!(!old_active.entries.get(old_identity.as_ref()).unwrap().health_eligible());
    let new_active = old_cell.active.load_full();
    assert!(new_active.entries.get(new_identity.as_ref()).unwrap().health_eligible());
}

#[test]
fn epoch_exhaustion_is_fail_closed_without_hiding_raw_unidentified_input() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/backend-a".to_string(),
        slice("backend-a", Some("slice-a"), true),
    )]));
    coordinator.set_next_epoch_for_test(u64::MAX);
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(None, IntOrString::String("web".to_string())),
    )]));
    let generation = coordinator.load_generation();
    assert!(matches!(
        generation.service("default/backend"),
        Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch
        ))
    ));
    assert!(generation.available("default/backend").is_none());
    assert!(coordinator.state.lock().services.contains_key("default/backend"));
    assert!(coordinator.runtime_cell("default/backend").is_none());
}

#[test]
fn delete_consumes_no_epoch_and_authoritative_recreate_gets_new_identity() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(None, IntOrString::String("web".to_string())),
    )]));
    let first = coordinator
        .state
        .lock()
        .services
        .get("default/backend")
        .unwrap()
        .identity
        .clone();
    let before_delete = coordinator.state.lock().next_epoch;
    coordinator.update_services(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["default/backend".to_string()]),
    );
    assert_eq!(coordinator.state.lock().next_epoch, before_delete);
    coordinator.update_services(
        HashMap::from([(
            "default/backend".to_string(),
            service(None, IntOrString::String("web".to_string())),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let recreated = coordinator
        .state
        .lock()
        .services
        .get("default/backend")
        .unwrap()
        .identity
        .clone();
    assert_ne!(first, recreated);
}

#[test]
fn exhaustion_invalidates_every_allocation_needing_source_without_partial_commit() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.set_next_epoch_for_test(u64::MAX);
    coordinator.replace_services(&HashMap::from([
        (
            "default/a".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("a".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
        (
            "default/b".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("b".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
    ]));
    let state = coordinator.state.lock();
    assert!(state.services.values().all(|source| source.identity.is_none()));
    drop(state);
    let generation = coordinator.load_generation();
    assert!(generation.available("default/a").is_none());
    assert!(generation.available("default/b").is_none());
    assert_eq!(generation.stats.identified_service_sources, 0);
    assert_eq!(generation.stats.unavailable_services, 2);
}

#[test]
fn late_outlier_completion_is_identity_guarded_and_detached_state_stays_frozen() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let generation = coordinator.load_generation();
    let old_backend = Arc::clone(
        &generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let old_identity = Arc::clone(old_backend.runtime_identity().unwrap());
    let cell = coordinator.runtime_cell("default/backend").unwrap();
    let old_active = cell.active.load_full();
    let store = OutlierDetectionStore::new();
    store.set_service_config_in(
        &coordinator,
        "default/backend",
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 30,
            max_ejection_seconds: 30,
            max_ejection_percent: 0,
        }),
    );
    let old_policy = store.capture_completion_token(&old_backend).unwrap();
    store.record_failure_for_backend_in(&coordinator, &old_backend, &old_policy, FailureClass::Gateway);
    assert!(old_active.entries.get(old_identity.as_ref()).unwrap().outlier_ejected());

    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(
            "default/backend-a".to_string(),
            slice("backend-a", Some("slice-replacement"), true),
        )]),
        HashSet::new(),
    );
    // This test uses an isolated outlier store, so mirror the production
    // activation hook that revokes the retired identity from the global
    // store before exercising the late completion.
    store.remove_runtime_identity(&old_identity);
    store.record_failure_for_backend_in(&coordinator, &old_backend, &old_policy, FailureClass::Gateway);
    store.record_success_for_backend_in(&coordinator, &old_backend, &old_policy);
    assert!(!store.has_runtime_identity(&old_identity));
    assert!(old_active.entries.get(old_identity.as_ref()).unwrap().outlier_ejected());
    let current = cell.active.load_full();
    assert!(current.entries.values().all(|state| !state.outlier_ejected()));
}

#[test]
fn membership_change_always_replaces_wrapper_for_uid_recreate_and_non_first_port_change() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let first = coordinator.load_generation();
    let first_backend = Arc::clone(&first.available("default/backend").unwrap().canonical_probe_backends()[0]);
    let first_identity = Arc::clone(first_backend.runtime_identity().unwrap());
    let first_counters = first_backend.runtime_counters();

    let mut changed = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    changed
        .spec
        .as_mut()
        .unwrap()
        .ports
        .as_mut()
        .unwrap()
        .push(ServicePort {
            name: Some("metrics".to_string()),
            port: 81,
            protocol: Some("TCP".to_string()),
            target_port: Some(IntOrString::String("web".to_string())),
            ..Default::default()
        });
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([("default/backend".to_string(), Arc::new(changed.clone()))]),
        HashSet::new(),
    );
    let second = coordinator.load_generation();
    let second_backend = Arc::clone(&second.available("default/backend").unwrap().canonical_probe_backends()[0]);
    assert!(!Arc::ptr_eq(&first_backend, &second_backend));
    assert_ne!(
        first_identity.as_ref(),
        second_backend.runtime_identity().unwrap().as_ref()
    );
    assert!(Arc::ptr_eq(&first_counters, &second_backend.runtime_counters()));

    changed.metadata.uid = Some("service-recreated".to_string());
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([("default/backend".to_string(), Arc::new(changed))]),
        HashSet::new(),
    );
    let third = coordinator.load_generation();
    let third_backend = Arc::clone(&third.available("default/backend").unwrap().canonical_probe_backends()[0]);
    assert!(!Arc::ptr_eq(&second_backend, &third_backend));
    assert_ne!(
        second_backend.runtime_identity().unwrap().as_ref(),
        third_backend.runtime_identity().unwrap().as_ref()
    );
    assert!(Arc::ptr_eq(
        &second_backend.runtime_counters(),
        &third_backend.runtime_counters()
    ));
}

#[test]
fn transaction_epoch_failure_commits_delete_but_no_source_identity_or_membership_partially() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    coordinator.update_services(
        HashMap::from([(
            "default/removed".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("removed".to_string()),
                    uid: Some("removed-uid".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    coordinator.set_next_epoch_for_test(u64::MAX);
    coordinator.update_services(
        HashMap::new(),
        HashMap::from([(
            "default/backend".to_string(),
            service(Some("replacement-uid"), IntOrString::String("web".to_string())),
        )]),
        HashSet::from(["default/removed".to_string()]),
    );

    let state = coordinator.state.lock();
    assert!(!state.services.contains_key("default/removed"));
    assert!(state.services["default/backend"].identity.is_none());
    assert!(state.exhausted);
    drop(state);
    let generation = coordinator.load_generation();
    assert!(generation.service("default/removed").is_none());
    assert!(matches!(
        generation.service("default/backend"),
        Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch
        ))
    ));
    assert_eq!(generation.stats.unidentified_service_sources, 1);
}

#[test]
fn completion_linearizes_before_retirement_and_replacement_freezes_the_result() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    let policy = health_policy(7);
    coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy)));
    publish_base(&coordinator);
    let generation = coordinator.load_generation();
    let backend = Arc::clone(
        &generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let identity = Arc::clone(backend.runtime_identity().unwrap());
    let membership = backend.runtime_membership_identity().unwrap();
    let cell = coordinator.runtime_cell("default/backend").unwrap();
    let old_active = cell.active.load_full();
    let reached = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    coordinator.set_completion_barrier_for_test(Some((Arc::clone(&reached), Arc::clone(&resume))));

    let completion_coordinator = Arc::clone(&coordinator);
    let completion_identity = Arc::clone(&identity);
    let completion_policy = Arc::clone(&policy);
    let completion = std::thread::spawn(move || {
        completion_coordinator.record_health_completion(
            &completion_policy,
            membership,
            &completion_identity,
            false,
            1,
            1,
        )
    });
    reached.wait();
    let replacement_coordinator = Arc::clone(&coordinator);
    let replacement = std::thread::spawn(move || {
        replacement_coordinator.update_endpoint_slices(
            HashMap::new(),
            HashMap::from([(
                "default/backend-a".to_string(),
                slice("backend-a", Some("barrier-replacement"), true),
            )]),
            HashSet::new(),
        );
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while cell.active.load().membership_identity.is_some() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(cell.active.load().membership_identity.is_none());
    resume.wait();
    assert!(completion.join().unwrap());
    replacement.join().unwrap();
    coordinator.set_completion_barrier_for_test(None);

    assert!(!old_active.entries[identity.as_ref()].health_eligible());
    assert!(cell.active.load().entries.values().all(|state| state.health_eligible()));
}

#[test]
fn health_policy_remove_and_change_reset_current_and_drop_old_probe() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let policy_one = health_policy(1);
    coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy_one)));
    publish_base(&coordinator);
    let generation = coordinator.load_generation();
    let backend = &generation
        .available("default/backend")
        .unwrap()
        .canonical_probe_backends()[0];
    let identity = backend.runtime_identity().unwrap();
    let membership = backend.runtime_membership_identity().unwrap();
    assert!(coordinator.record_health_completion(&policy_one, membership, identity, false, 1, 1));
    let state = coordinator.runtime_cell("default/backend").unwrap().active.load_full();
    assert!(!state.entries[identity.as_ref()].health_eligible());

    coordinator.set_health_policy("default/backend", None);
    assert!(state.entries[identity.as_ref()].health_eligible());
    assert!(!coordinator.record_health_completion(&policy_one, membership, identity, false, 1, 1));
    let policy_two = health_policy(2);
    coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy_two)));
    assert!(!coordinator.record_health_completion(&policy_one, membership, identity, false, 1, 1));
    assert!(coordinator.record_health_completion(&policy_two, membership, identity, false, 1, 1));
    assert!(!state.entries[identity.as_ref()].health_eligible());
}

#[test]
fn outlier_policy_clear_drops_old_attempt_and_detached_cooldown_is_frozen() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let generation = coordinator.load_generation();
    let backend = Arc::clone(
        &generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let identity = Arc::clone(backend.runtime_identity().unwrap());
    let cell = coordinator.runtime_cell("default/backend").unwrap();
    let old_active = cell.active.load_full();
    let store = OutlierDetectionStore::new();
    store.set_service_config_in(
        &coordinator,
        "default/backend",
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: 1,
            max_ejection_percent: 0,
        }),
    );
    let old_policy = store.capture_completion_token(&backend).unwrap();
    store.record_failure_for_backend_in(&coordinator, &backend, &old_policy, FailureClass::Gateway);
    assert!(old_active.entries[identity.as_ref()].outlier_ejected());

    store.set_service_config_in(&coordinator, "default/backend", None);
    assert!(!old_active.entries[identity.as_ref()].outlier_ejected());
    store.record_failure_for_backend_in(&coordinator, &backend, &old_policy, FailureClass::Gateway);
    assert!(!old_active.entries[identity.as_ref()].outlier_ejected());

    store.set_service_config_in(
        &coordinator,
        "default/backend",
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: 1,
            max_ejection_percent: 0,
        }),
    );
    let current_policy = store.capture_completion_token(&backend).unwrap();
    store.record_failure_for_backend_in(&coordinator, &backend, &current_policy, FailureClass::Gateway);
    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(
            "default/backend-a".to_string(),
            slice("backend-a", Some("cooldown-replacement"), true),
        )]),
        HashSet::new(),
    );
    std::thread::sleep(Duration::from_millis(5));
    assert!(old_active.entries[identity.as_ref()].outlier_ejected());
}

#[test]
fn outlier_policy_clear_waits_for_inflight_completion_then_resets_eligibility() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    publish_base(&coordinator);
    let generation = coordinator.load_generation();
    let backend = Arc::clone(
        &generation
            .available("default/backend")
            .unwrap()
            .canonical_probe_backends()[0],
    );
    let identity = Arc::clone(backend.runtime_identity().unwrap());
    let state = coordinator.runtime_cell("default/backend").unwrap().active.load_full();
    let store = Arc::new(OutlierDetectionStore::new());
    store.set_service_config_in(
        &coordinator,
        "default/backend",
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 30,
            max_ejection_seconds: 30,
            max_ejection_percent: 0,
        }),
    );
    let token = store.capture_completion_token(&backend).unwrap();
    let reached = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    coordinator.set_completion_barrier_for_test(Some((Arc::clone(&reached), Arc::clone(&resume))));

    let completion_store = Arc::clone(&store);
    let completion_coordinator = Arc::clone(&coordinator);
    let completion_backend = Arc::clone(&backend);
    let completion = std::thread::spawn(move || {
        completion_store.record_failure_for_backend_in(
            &completion_coordinator,
            &completion_backend,
            &token,
            FailureClass::Gateway,
        );
    });
    reached.wait();
    let clear_store = Arc::clone(&store);
    let clear_coordinator = Arc::clone(&coordinator);
    let clear = std::thread::spawn(move || {
        clear_store.set_service_config_in(&clear_coordinator, "default/backend", None);
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while coordinator.outlier_policies.contains_key("default/backend") && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(!coordinator.outlier_policies.contains_key("default/backend"));
    resume.wait();
    completion.join().unwrap();
    clear.join().unwrap();
    coordinator.set_completion_barrier_for_test(None);
    assert!(!state.entries[identity.as_ref()].outlier_ejected());
    assert!(!store.has_runtime_identity(&identity));
}

#[test]
fn dns_candidate_is_cold_and_published_with_its_actual_service_generation() {
    use edgion_common::dns::resolver_select::{LinkSysKey, ResolverCandidate};
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut source = (*service(Some("uid-a"), IntOrString::Int(80))).clone();
    source.metadata.annotations = Some(BTreeMap::from([(DNS_RESOLVER_REF.into(), " dns / first ".into())]));
    let source = Arc::new(source);
    coordinator.replace_services(&HashMap::from([("default/backend".into(), source.clone())]));
    let old = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .clone();
    assert_eq!(
        old.dns_resolver_candidate,
        Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".into(),
            name: "first".into()
        }))
    );
    coordinator.replace_services(&HashMap::from([("default/backend".into(), source.clone())]));
    assert!(Arc::ptr_eq(
        &old,
        coordinator.load_generation().available("default/backend").unwrap()
    ));
    let mut updated = (*source).clone();
    updated
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(DNS_RESOLVER_REF.into(), "dns/second".into());
    coordinator.replace_services(&HashMap::from([("default/backend".into(), Arc::new(updated.clone()))]));
    let new = coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .clone();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(
        new.dns_resolver_candidate,
        Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".into(),
            name: "second".into()
        }))
    );
    assert_eq!(
        old.dns_resolver_candidate,
        Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".into(),
            name: "first".into()
        }))
    );
    coordinator.replace_services(&HashMap::new());
    assert!(coordinator.load_generation().available("default/backend").is_none());
    updated.metadata.uid = Some("uid-b".into());
    coordinator.replace_services(&HashMap::from([("default/backend".into(), Arc::new(updated))]));
    assert_eq!(
        coordinator
            .load_generation()
            .available("default/backend")
            .unwrap()
            .dns_resolver_candidate,
        new.dns_resolver_candidate
    );
}

#[test]
fn selector_sets_are_built_only_for_unique_final_resolved_sections() {
    use crate::lb::compiled_selector::selector_set_build_count_for_test as builds;

    let coordinator = ServiceDiscoveryCoordinator::new();
    let sources = HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::Int(8080)),
    )]);
    let before = builds();
    coordinator.replace_services(&sources);
    assert_eq!(builds() - before, 1, "empty final section still compiles eagerly");
    let slices = HashMap::from([(
        "default/backend-a".to_string(),
        slice("backend-a", Some("slice-a"), true),
    )]);
    let before = builds();
    coordinator.replace_endpoint_slices(&slices);
    assert_eq!(builds() - before, 1, "number/name aliases share one selector set");
    let old = coordinator.load_generation();
    let before = builds();
    let mut changed_state = coordinator.state.lock().clone();
    changed_state
        .endpoint_slices
        .get_mut("default/backend-a")
        .unwrap()
        .resource = slice("backend-a", Some("slice-a"), false);
    let demand = projection_epoch_requirement("default/backend", &changed_state, old.service("default/backend"));
    assert_eq!(demand, 1);
    assert_eq!(builds(), before, "preflight never constructs selectors");
    coordinator.replace_endpoint_slices(&slices);
    assert_eq!(builds(), before, "semantic no-op never constructs selectors");
    assert!(Arc::ptr_eq(&old, &coordinator.load_generation()));

    let mut dual = (*sources["default/backend"]).clone();
    dual.spec.as_mut().unwrap().ports.as_mut().unwrap().push(ServicePort {
        name: Some("metrics".to_string()),
        port: 81,
        ..Default::default()
    });
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(dual))]));
    assert_eq!(
        builds() - before,
        2,
        "every resolved section of a changed Service materializes"
    );

    let before = builds();
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("service-uid"), IntOrString::String(String::new())),
    )]));
    assert_eq!(builds(), before, "invalid sections have no selectors");
    assert_eq!(coordinator.load_generation().stats.invalid_sections, 1);
    let invalid = coordinator.load_generation();
    let invalid = invalid.available("default/backend").unwrap();
    assert!(Arc::ptr_eq(
        invalid
            .sections
            .get(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
            .unwrap(),
        invalid
            .sections
            .get(&ServiceSectionKey::Name(Arc::from("http")))
            .unwrap(),
    ));
    coordinator.set_next_epoch_for_test(u64::MAX);
    coordinator.replace_services(&HashMap::from([(
        "default/backend".to_string(),
        service(Some("replacement-uid"), IntOrString::Int(8080)),
    )]));
    assert_eq!(
        builds(),
        before,
        "failed whole-transaction reservation constructs no selectors"
    );
    assert!(matches!(
        coordinator.load_generation().service("default/backend"),
        Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch
        ))
    ));
}

#[test]
fn terminal_epoch_exact_fit_counts_aliases_once_and_invalid_sections_zero() {
    use crate::lb::compiled_selector::selector_set_build_count_for_test as builds;

    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_base(&coordinator);
    let mut updated = (*service(Some("service-uid"), IntOrString::Int(8080))).clone();
    updated
        .spec
        .as_mut()
        .unwrap()
        .ports
        .as_mut()
        .unwrap()
        .push(ServicePort {
            name: Some("invalid".to_string()),
            port: 81,
            target_port: Some(IntOrString::String(String::new())),
            ..Default::default()
        });
    coordinator.set_next_epoch_for_test(u64::MAX);
    let before = builds();
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(updated))]));
    assert_eq!(builds() - before, 1);
    let generation = coordinator.load_generation();
    let available = generation
        .available("default/backend")
        .expect("one slot exactly fits terminal epoch");
    let numbered = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap();
    let named = available
        .resolve_section(&ServiceSectionKey::Name(Arc::from("http")))
        .unwrap();
    assert!(Arc::ptr_eq(&numbered, &named));
    assert_eq!(
        numbered.all_slots[0].backend.runtime_identity().unwrap().incarnation,
        u64::MAX
    );
    for key in [
        ServiceSectionKey::Number(NonZeroU16::new(81).unwrap()),
        ServiceSectionKey::Name(Arc::from("invalid")),
    ] {
        assert!(matches!(
            available.resolve_section(&key),
            Err(ServiceSectionInvalid::InvalidServicePort)
        ));
    }
    assert!(coordinator.state.lock().exhausted);
}

#[test]
fn active_health_isolated_across_same_socket_sections() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let policy = health_policy(1);
    coordinator.set_health_policy("default/backend", Some(Arc::clone(&policy)));
    let mut service = (*service(Some("service-uid"), IntOrString::String("web".to_string()))).clone();
    service
        .spec
        .as_mut()
        .unwrap()
        .ports
        .as_mut()
        .unwrap()
        .push(ServicePort {
            name: Some("metrics".to_string()),
            port: 81,
            protocol: Some("TCP".to_string()),
            target_port: Some(IntOrString::String("web".to_string())),
            ..Default::default()
        });
    coordinator.replace_services(&HashMap::from([("default/backend".to_string(), Arc::new(service))]));
    let mut dual = (*slice("backend-a", Some("slice-a"), true)).clone();
    dual.ports = Some(slice_ports(&[("http", 8080), ("metrics", 8080)]));
    coordinator.replace_endpoint_slices(&HashMap::from([("default/backend-a".to_string(), Arc::new(dual))]));

    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").unwrap();
    let first = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap();
    let second = available
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(81).unwrap()))
        .unwrap();
    let first_backend = &first.ready_slots[0].backend;
    let second_backend = &second.ready_slots[0].backend;
    assert_eq!(first_backend.addr, second_backend.addr);
    assert!(!Arc::ptr_eq(first_backend, second_backend));
    let first_identity = first_backend.runtime_identity().expect("section slot is identified");
    let second_identity = second_backend.runtime_identity().expect("section slot is identified");
    assert_ne!(first_identity.as_ref(), second_identity.as_ref());
    let active = available.runtime_cell.active.load_full();
    assert!(coordinator.record_health_completion(
        &policy,
        first_backend.runtime_membership_identity().unwrap(),
        first_identity,
        false,
        1,
        1
    ));
    assert!(!active.entries[first_identity.as_ref()].health_eligible());
    assert!(active.entries[second_identity.as_ref()].health_eligible());
    assert!(coordinator.record_health_completion(
        &policy,
        second_backend.runtime_membership_identity().unwrap(),
        second_identity,
        false,
        1,
        1
    ));
    assert!(!active.entries[second_identity.as_ref()].health_eligible());
    assert!(coordinator.record_health_completion(
        &policy,
        first_backend.runtime_membership_identity().unwrap(),
        first_identity,
        true,
        1,
        1
    ));
    assert!(active.entries[first_identity.as_ref()].health_eligible());
    assert!(!active.entries[second_identity.as_ref()].health_eligible());
}
