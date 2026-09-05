use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
#[cfg(test)]
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::{get_service_discovery_coordinator, ServiceDiscoveryCoordinator};
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

    pub fn ready_endpoints_for_port(&self, service_key: &str, port: Option<i32>) -> Vec<String> {
        let Some(slices) = self.get_slices_for_service(service_key) else {
            return Vec::new();
        };
        let mut endpoints = Vec::new();
        for slice in slices.iter() {
            let Some(serving_port) = slice
                .ports
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .find(|candidate| port.is_none() || candidate.port == port)
                .and_then(|candidate| candidate.port)
                .and_then(|value| u16::try_from(value).ok())
                .filter(|value| *value != 0)
            else {
                continue;
            };
            for endpoint in &slice.endpoints {
                if !endpoint
                    .conditions
                    .as_ref()
                    .and_then(|conditions| conditions.ready)
                    .unwrap_or(false)
                {
                    continue;
                }
                for address in &endpoint.addresses {
                    if let Ok(ip) = address.parse::<std::net::IpAddr>() {
                        endpoints.push(std::net::SocketAddr::new(ip, serving_port).to_string());
                    }
                }
            }
        }
        endpoints
    }

    pub fn get_backends_for_service(&self, service_key: &str) -> Arc<Vec<Arc<LbBackend>>> {
        self.coordinator
            .load_generation()
            .available(service_key)
            .map(|service| service.canonical_probe_backends())
            .unwrap_or_else(|| Arc::new(Vec::new()))
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

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

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
