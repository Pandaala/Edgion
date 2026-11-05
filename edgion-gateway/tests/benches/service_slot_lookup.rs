//! Compare lookup and compiled selection + metadata, not full request latency.
//! Run: cargo bench -p edgion-gateway --bench service_slot_lookup
use edgion_gateway::backends::discovery::{ResolvedServiceSection, ServiceDiscoveryCoordinator, ServiceSectionKey};
use edgion_gateway::lb::compiled_selector::PolicyKind;
use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use kube::api::ObjectMeta;
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Instant;

fn section(coordinator: &ServiceDiscoveryCoordinator) -> Arc<ResolvedServiceSection> {
    coordinator
        .load_generation()
        .available("default/backend")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(NonZeroU16::new(80).unwrap()))
        .unwrap()
}

fn measure(mut run: impl FnMut(usize)) -> f64 {
    const ITERATIONS: usize = 20_000;
    for i in 0..1_000 {
        run(i);
    }
    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for i in 0..ITERATIONS {
            run(black_box(i));
        }
        samples.push(start.elapsed().as_nanos() as f64 / ITERATIONS as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[2]
}

fn main() {
    println!("Median ns/op; compiled selection excludes capture, degrade and live health lookup.");
    println!("No new per-generation index or per-lookup allocations; slot storage is unchanged.");
    for size in [4, 32, 256, 2048, 4096] {
        let coordinator = ServiceDiscoveryCoordinator::new();
        coordinator.replace_services(&HashMap::from([(
            "default/backend".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".into()),
                    name: Some("backend".into()),
                    uid: Some("service".into()),
                    ..Default::default()
                },
                spec: Some(ServiceSpec {
                    ports: Some(vec![ServicePort {
                        name: Some("http".into()),
                        port: 80,
                        protocol: Some("TCP".into()),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )]));
        let mut source = EndpointSlice {
            metadata: ObjectMeta {
                namespace: Some("default".into()),
                name: Some("slice".into()),
                uid: Some("slice".into()),
                labels: Some(BTreeMap::from([(
                    "kubernetes.io/service-name".into(),
                    "backend".into(),
                )])),
                ..Default::default()
            },
            address_type: "IPv4".into(),
            endpoints: (0..size)
                .map(|i| Endpoint {
                    addresses: vec![Ipv4Addr::from(0x0a000001 + i as u32).to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(i % 4 != 0),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect(),
            ports: Some(vec![EndpointPort {
                name: Some("http".into()),
                port: Some(8080),
                protocol: Some("TCP".into()),
                ..Default::default()
            }]),
        };
        let started = Instant::now();
        coordinator.replace_endpoint_slices(&HashMap::from([("default/slice".into(), Arc::new(source.clone()))]));
        let published = section(&coordinator);
        println!("N={size} initial_publication_us={}", started.elapsed().as_micros());
        let late = *published.all_slots().last().unwrap().backend.addr.as_inet().unwrap();
        let missing: SocketAddr = "10.255.255.255:8080".parse().unwrap();
        let unready = *published.all_slots()[0].backend.addr.as_inet().unwrap();
        for (name, address) in [("first-unready", unready), ("late", late), ("miss", missing)] {
            let linear = measure(|_| {
                let target = black_box(address);
                black_box(
                    published
                        .all_slots()
                        .iter()
                        .find(|slot| slot.backend.addr.as_inet() == Some(&target)),
                );
            });
            let binary = measure(|_| {
                black_box(published.slot_for_addr(black_box(address)));
            });
            println!("N={size} lookup={name} linear_ns={linear:.1} binary_ns={binary:.1}");
        }
        for kind in [
            PolicyKind::RoundRobin,
            PolicyKind::ConsistentHash,
            PolicyKind::LeastConn,
            PolicyKind::Ewma,
        ] {
            let selector = published.selectors.get(kind);
            for binary in [false, true] {
                let ns = measure(|i| {
                    let key = i.to_be_bytes();
                    let backend = selector.select(Some(&key), |_| true, 0).unwrap();
                    let found = if binary {
                        published
                            .slot_for_addr(*backend.addr.as_inet().unwrap())
                            .filter(|slot| Arc::ptr_eq(&slot.backend, &backend))
                    } else {
                        published
                            .all_slots()
                            .iter()
                            .find(|slot| Arc::ptr_eq(&slot.backend, &backend))
                            .map(Arc::as_ref)
                    };
                    black_box(found.unwrap());
                });
                println!("N={size} selection={kind:?} binary={binary} ns={ns:.1}");
            }
        }
        source.endpoints[0].conditions.as_mut().unwrap().ready = Some(true);
        let started = Instant::now();
        coordinator.replace_endpoint_slices(&HashMap::from([("default/slice".into(), Arc::new(source))]));
        let replacement = section(&coordinator);
        println!(
            "N={size} reload_us={} retained_old_slots={} new_slots={}",
            started.elapsed().as_micros(),
            published.all_slots().len(),
            replacement.all_slots().len()
        );
    }
}
