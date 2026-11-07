use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
#[cfg(test)]
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::{
    get_service_discovery_coordinator, ResolvedServiceSection, ServiceDiscoveryCoordinator, ServiceSectionKey,
};
use crate::lb::LbBackend;

static ROUNDROBIN_STORE: LazyLock<EpSliceStore<RoundRobin>> =
    LazyLock::new(|| EpSliceStore::new(Arc::clone(get_service_discovery_coordinator())));

pub fn get_roundrobin_store() -> &'static EpSliceStore<RoundRobin> {
    &ROUNDROBIN_STORE
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
    pub fn ready_endpoints_for_port(&self, service_key: &str, service_port: Option<i32>) -> Vec<SocketAddr> {
        let Some(section) = self.ready_section_for_port(service_key, service_port) else {
            return Vec::new();
        };
        ready_section_sockets(&section)
    }

    /// Return the resolved section behind [`Self::ready_endpoints_for_port`].
    ///
    /// The `Arc` identity lets a caller cache data derived from the section and
    /// rebuild it only when a new section is published.
    pub fn ready_section_for_port(
        &self,
        service_key: &str,
        service_port: Option<i32>,
    ) -> Option<Arc<ResolvedServiceSection>> {
        let service_port = service_port
            .and_then(|port| u16::try_from(port).ok())
            .and_then(NonZeroU16::new)?;
        let generation = self.coordinator.load_generation();
        let service = generation.available(service_key)?;
        service.resolve_section(&ServiceSectionKey::Number(service_port)).ok()
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
        Self::new(Arc::clone(get_service_discovery_coordinator()))
    }
}

#[cfg(test)]
include!("../../../../tests/support/backends/discovery/endpoint_slice/ep_slice_store_test/186_function_item.rs");

/// Ready sockets of one resolved section. Fails closed (empty) if any slot is
/// not an Inet address, matching the historical facade.
pub fn ready_section_sockets(section: &ResolvedServiceSection) -> Vec<SocketAddr> {
    section
        .ready_slots
        .iter()
        .map(|slot| slot.backend.addr.as_inet().copied())
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/endpoint_slice/ep_slice_store_test/inventory_tests.rs"]
mod inventory_tests;
