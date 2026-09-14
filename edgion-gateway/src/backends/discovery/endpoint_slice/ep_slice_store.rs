use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
#[cfg(test)]
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::num::NonZeroU16;
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::{get_service_discovery_coordinator, ServiceDiscoveryCoordinator, ServiceSectionKey};
use crate::lb::LbBackend;

static ROUNDROBIN_STORE: LazyLock<Arc<EpSliceStore<RoundRobin>>> =
    LazyLock::new(|| Arc::new(EpSliceStore::new(get_service_discovery_coordinator())));

pub fn get_roundrobin_store() -> Arc<EpSliceStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

/// Read-only compatibility facade over one joint Service selector generation.
pub struct EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    coordinator: Arc<ServiceDiscoveryCoordinator>,
    _marker: PhantomData<S>,
}

impl<S> EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    pub fn new(coordinator: Arc<ServiceDiscoveryCoordinator>) -> Self {
        Self {
            coordinator,
            _marker: PhantomData,
        }
    }

    pub fn get_slices_for_service(&self, service_key: &str) -> Option<Arc<[Arc<EndpointSlice>]>> {
        self.coordinator
            .load_generation()
            .available(service_key)
            .map(|service| Arc::clone(&service.raw.endpoint_slices))
            .filter(|slices| !slices.is_empty())
    }

    /// Return the ready sockets proven for one Kubernetes Service port.
    ///
    /// The requested port identifies `ServicePort.port`; each returned socket
    /// retains the serving port compiled from the matching EndpointSlice
    /// section. Invalid or unavailable sections fail closed.
    pub fn ready_endpoints_for_port(&self, service_key: &str, service_port: Option<i32>) -> Vec<String> {
        let Some(service_port) = service_port
            .and_then(|port| u16::try_from(port).ok())
            .and_then(NonZeroU16::new)
        else {
            return Vec::new();
        };
        let generation = self.coordinator.load_generation();
        let Some(service) = generation.available(service_key) else {
            return Vec::new();
        };
        let Ok(section) = service.resolve_section(&ServiceSectionKey::Number(service_port)) else {
            return Vec::new();
        };

        section
            .ready_slots
            .iter()
            .map(|slot| slot.backend.addr.as_inet().map(ToString::to_string))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
    }

    pub fn get_backends_for_service(&self, service_key: &str) -> Arc<[Arc<LbBackend>]> {
        self.coordinator
            .load_generation()
            .available(service_key)
            .map(|service| service.canonical_probe_backends())
            .unwrap_or_else(|| Arc::from(Vec::new()))
    }

    pub fn slice_count(&self) -> usize {
        self.coordinator.load_generation().stats.endpoint_slice_sources
    }

    pub fn service_index_count(&self) -> usize {
        self.coordinator.load_generation().stats.available_services
    }

    pub fn keys(&self) -> Vec<String> {
        let generation = self.coordinator.load_generation();
        let mut keys = Vec::new();
        for service in generation.services.values() {
            if let crate::backends::discovery::ServicePublishedState::Available(service) = service {
                keys.extend(service.raw.endpoint_slices.iter().filter_map(|slice| {
                    let namespace = slice.metadata.namespace.as_deref()?;
                    let name = slice.metadata.name.as_deref()?;
                    Some(format!("{namespace}/{name}"))
                }));
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    #[cfg(test)]
    pub fn update_data_only(
        &self,
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let affected: HashSet<String> = add.values().chain(update.values()).map(extract_service_key).collect();
        let mut synthetic_services = HashMap::new();
        for service_key in &affected {
            if self.coordinator.load_generation().service(service_key).is_some() {
                continue;
            }
            let (namespace, name) = service_key.split_once('/').unwrap_or(("default", service_key));
            let mut ports = Vec::new();
            let mut seen = HashSet::new();
            for slice in add
                .values()
                .chain(update.values())
                .filter(|slice| extract_service_key(slice) == *service_key)
            {
                for port in slice.ports.as_deref().unwrap_or(&[]) {
                    let Some(number) = port.port else {
                        continue;
                    };
                    if !seen.insert(number) {
                        continue;
                    }
                    ports.push(k8s_openapi::api::core::v1::ServicePort {
                        name: port.name.clone(),
                        port: number,
                        protocol: port.protocol.clone(),
                        ..Default::default()
                    });
                }
            }
            synthetic_services.insert(
                service_key.clone(),
                Arc::new(k8s_openapi::api::core::v1::Service {
                    metadata: kube::api::ObjectMeta {
                        namespace: Some(namespace.to_string()),
                        name: Some(name.to_string()),
                        uid: Some(format!("synthetic-{service_key}")),
                        ..Default::default()
                    },
                    spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                        ports: (!ports.is_empty()).then_some(ports),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            );
        }
        if !synthetic_services.is_empty() {
            self.coordinator
                .update_services(synthetic_services, HashMap::new(), HashSet::new());
        }
        self.coordinator.update_endpoint_slices(
            add.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            update.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            remove.clone(),
        );
        affected
    }
}

impl<S> Default for EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    fn default() -> Self {
        Self::new(get_service_discovery_coordinator())
    }
}

#[cfg(test)]
pub fn extract_service_key(slice: &EndpointSlice) -> String {
    super::super::coordinator::endpoint_slice_service_key(slice)
}

#[cfg(test)]
mod inventory_tests {
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
}
