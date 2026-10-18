use super::*;
use edgion_resources::resources::common::RefDenied;
use std::sync::{Arc, Barrier};

fn resolved_endpoint(address: &str, backend_name: &str) -> ResolvedEndpoint {
    ResolvedEndpoint {
        address: address.to_string(),
        backend_name: backend_name.to_string(),
        transport: Arc::new(ProbeTransport::new(
            crate::backends::policy::BackendTlsPolicyLookup::Absent,
            get_service_discovery_coordinator().load_generation(),
            get_global_backend_tls_policy_store().snapshot().generation(),
            None,
        )),
        allow_loopback: false,
    }
}

fn completed_endpoint(ordinal: usize, address: &str, status: u16, body_bytes: &[u8]) -> CompletedEndpointResult {
    CompletedEndpointResult {
        ordinal,
        response_body_bytes: body_bytes.len(),
        result: EndpointResult {
            address: address.to_string(),
            status: Some(status),
            latency_ms: 0,
            body: Some(String::from_utf8_lossy(body_bytes).to_string()),
            body_truncated: false,
            headers: None,
            error: None,
        },
    }
}

fn backend_ref(name: &str, namespace: Option<&str>, denied: bool) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        port: Some(8080),
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: denied.then(|| RefDenied {
            target_namespace: namespace.unwrap_or("route-ns").to_string(),
            target_name: name.to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }),
    }
}

fn backend_result(name: &str, endpoint_count: usize) -> BackendResult {
    BackendResult {
        name: name.to_string(),
        namespace: "route-ns".to_string(),
        port: 8080,
        endpoint_count,
        endpoints: Vec::new(),
    }
}

#[test]
fn encoded_rate_gate_handles_zero_boundary_and_remaining_delay() {
    let state = AtomicU64::new(0);

    assert_eq!(claim_execution_at(&state, 0, 1_000), Ok(()));
    assert_eq!(state.load(Ordering::Relaxed), 1);
    assert_eq!(claim_execution_at(&state, 0, 1_000), Err(1_000));
    assert_eq!(claim_execution_at(&state, 999, 1_000), Err(1));
    assert_eq!(claim_execution_at(&state, 1_000, 1_000), Ok(()));
}

#[test]
fn encoded_rate_gate_rejects_rollback_until_clock_catches_up() {
    let state = AtomicU64::new(0);

    assert_eq!(claim_execution_at(&state, 1_000, 100), Ok(()));
    assert_eq!(claim_execution_at(&state, 1_100, 100), Ok(()));
    assert_eq!(claim_execution_at(&state, 900, 100), Err(100));
    assert_eq!(claim_execution_at(&state, 1_199, 100), Err(1));
    assert_eq!(claim_execution_at(&state, 1_200, 100), Ok(()));
}

#[test]
fn encoded_rate_gate_has_one_concurrent_winner_at_epoch_zero() {
    const THREADS: usize = 16;
    let state = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::with_capacity(THREADS);

    for _ in 0..THREADS {
        let state = Arc::clone(&state);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            claim_execution_at(&state, 0, 1_000).is_ok()
        }));
    }

    let winners = handles
        .into_iter()
        .map(|handle| usize::from(handle.join().expect("rate-gate worker must not panic")))
        .sum::<usize>();
    assert_eq!(winners, 1);
}

#[test]
fn completion_record_cannot_replace_a_newer_claim() {
    let state = AtomicU64::new(0);
    assert_eq!(claim_execution_at(&state, 100, 100), Ok(()));
    assert_eq!(claim_execution_at(&state, 200, 100), Ok(()));

    record_execution_completed_at(&state, 150);
    assert_eq!(state.load(Ordering::Relaxed), encode_elapsed_ms(200));

    record_execution_completed_at(&state, 250);
    assert_eq!(state.load(Ordering::Relaxed), encode_elapsed_ms(250));
}

#[test]
fn elapsed_tick_encoding_saturates_without_reusing_zero() {
    assert_eq!(duration_millis(Duration::MAX), MAX_ELAPSED_MS);
    assert_eq!(encode_elapsed_ms(u64::MAX), u64::MAX);
    assert_eq!(encode_elapsed_ms(0), 1);

    let state = AtomicU64::new(0);
    assert_eq!(claim_execution_at(&state, MAX_ELAPSED_MS, 1_000), Ok(()));
    assert_eq!(state.load(Ordering::Relaxed), u64::MAX);
    assert_eq!(claim_execution_at(&state, u64::MAX, 1_000), Err(1_000));

    record_execution_completed_at(&state, u64::MAX);
    assert_eq!(state.load(Ordering::Relaxed), u64::MAX);
    assert_ne!(state.load(Ordering::Relaxed), 0);
}

#[test]
fn denied_service_ref_is_filtered_before_resolution() {
    let refs = vec![
        backend_ref("denied", Some("other-ns"), true),
        backend_ref("allowed", None, false),
    ];
    let mut calls = Vec::new();

    let collection = collect_route_endpoints_with(
        &refs,
        "route-ns",
        10,
        false,
        |name, namespace, _port, _allow_loopback| {
            calls.push((name.to_string(), namespace.to_string()));
            Ok((vec![resolved_endpoint("10.0.0.1:8080", name)], 0))
        },
    );

    assert_eq!(calls, [("allowed".to_string(), "route-ns".to_string())]);
    assert_eq!(collection.denied_ref_count, 1);
    assert_eq!(collection.backends.len(), 1);
    assert_eq!(collection.backends[0].name, "allowed");
    assert_eq!(collection.discovered_endpoint_count, 1);
    assert_eq!(collection.selected_endpoints.len(), 1);
}

#[test]
fn endpoint_collection_distinguishes_no_discovery_from_cap_zero() {
    let refs = vec![backend_ref("service", None, false)];
    let no_discovery = collect_route_endpoints_with(
        &refs,
        "route-ns",
        10,
        false,
        |_name, _namespace, _port, _allow_loopback| Ok((Vec::new(), 0)),
    );

    assert_eq!(no_discovery.backends[0].endpoint_count, 0);
    assert_eq!(no_discovery.discovered_endpoint_count, 0);
    assert!(no_discovery.selected_endpoints.is_empty());
    assert!(!no_discovery.truncated());

    let cap_zero = collect_route_endpoints_with(
        &refs,
        "route-ns",
        0,
        false,
        |name, _namespace, _port, _allow_loopback| {
            Ok((
                vec![
                    resolved_endpoint("10.0.0.1:8080", name),
                    resolved_endpoint("10.0.0.2:8080", name),
                ],
                0,
            ))
        },
    );

    assert_eq!(cap_zero.backends[0].endpoint_count, 2);
    assert_eq!(cap_zero.discovered_endpoint_count, 2);
    assert!(cap_zero.selected_endpoints.is_empty());
    assert!(cap_zero.truncated());
}

#[test]
fn endpoint_collection_keeps_global_prefix_and_full_backend_counts() {
    let refs = vec![backend_ref("first", None, false), backend_ref("second", None, false)];
    let collection = collect_route_endpoints_with(
        &refs,
        "route-ns",
        3,
        false,
        |name, _namespace, _port, _allow_loopback| {
            Ok((
                vec![
                    resolved_endpoint(&format!("{name}-1"), name),
                    resolved_endpoint(&format!("{name}-2"), name),
                ],
                0,
            ))
        },
    );

    assert_eq!(
        collection
            .backends
            .iter()
            .map(|backend| backend.endpoint_count)
            .collect::<Vec<_>>(),
        [2, 2]
    );
    assert_eq!(collection.discovered_endpoint_count, 4);
    assert_eq!(
        collection
            .selected_endpoints
            .iter()
            .map(|endpoint| endpoint.address.as_str())
            .collect::<Vec<_>>(),
        ["first-1", "first-2", "second-1"]
    );
    assert!(collection.truncated());
}

#[test]
fn fan_out_permits_are_bounded_by_selected_endpoints() {
    assert_eq!(fan_out_permit_count(usize::MAX, 50), Some(50));
    assert_eq!(fan_out_permit_count(10, 0), None);
    let _semaphore = Semaphore::new(fan_out_permit_count(usize::MAX, 50).unwrap());
}

#[test]
fn invalid_service_ports_never_reach_endpoint_resolution() {
    for port in [None, Some(0), Some(-1), Some(65_536), Some(i32::MAX)] {
        let mut backend = backend_ref("service", None, false);
        backend.port = port;
        let collection = collect_route_endpoints_with(&[backend], "default", 10, false, |_, _, _, _| {
            panic!("invalid port must fail before resolution")
        });
        assert_eq!(collection.invalid_ref_count, 1);
        assert!(collection.selected_endpoints.is_empty());
    }
}

#[test]
fn probes_use_the_requested_service_section_and_each_slices_serving_port() {
    use crate::backends::discovery::ServiceDiscoveryCoordinator;
    use crate::backends::policy::BackendTLSPolicyStore;
    use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use std::collections::BTreeMap;

    let coordinator = ServiceDiscoveryCoordinator::new();
    let service = Service {
        metadata: ObjectMeta {
            namespace: Some("default".to_owned()),
            name: Some("backend".to_owned()),
            uid: Some("probe-service".to_owned()),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            ports: Some(vec![ServicePort {
                name: Some("https".to_owned()),
                port: 443,
                target_port: Some(IntOrString::String("web".to_owned())),
                protocol: Some("TCP".to_owned()),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    coordinator.replace_services(&HashMap::from([("default/backend".to_owned(), Arc::new(service))]));
    let slices = [("first", "10.0.0.1", 8443), ("second", "10.0.0.2", 9443)].map(|(name, ip, port)| {
        (
            format!("default/{name}"),
            Arc::new(EndpointSlice {
                metadata: ObjectMeta {
                    namespace: Some("default".to_owned()),
                    name: Some(name.to_owned()),
                    uid: Some(name.to_owned()),
                    labels: Some(BTreeMap::from([(
                        "kubernetes.io/service-name".to_owned(),
                        "backend".to_owned(),
                    )])),
                    ..Default::default()
                },
                address_type: "IPv4".to_owned(),
                endpoints: vec![Endpoint {
                    addresses: vec![ip.to_owned()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ports: Some(vec![EndpointPort {
                    name: Some("https".to_owned()),
                    port: Some(port),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                }]),
            }),
        )
    });
    coordinator.replace_endpoint_slices(&HashMap::from(slices));
    let discovery = coordinator.load_generation();
    let policies = BackendTLSPolicyStore::new().snapshot();
    let (endpoints, unsafe_count) =
        resolve_endpoints("backend", "default", 443, false, &discovery, &policies, None).unwrap();
    let mut addresses: Vec<_> = endpoints.iter().map(|endpoint| endpoint.address.as_str()).collect();
    addresses.sort_unstable();
    assert_eq!(addresses, ["10.0.0.1:8443", "10.0.0.2:9443"]);
    assert_eq!(unsafe_count, 0);
    assert!(Arc::ptr_eq(&endpoints[0].transport, &endpoints[1].transport));
    assert!(resolve_endpoints("backend", "default", 8443, false, &discovery, &policies, None).is_err());
    assert!(resolve_endpoints("missing", "default", 443, false, &discovery, &policies, None).is_err());
}

#[tokio::test(start_paused = true)]
async fn fan_out_returns_when_all_requests_complete() {
    let futures = (0..2)
        .map(|ordinal| async move { completed_endpoint(ordinal, "duplicate:8080", 200, b"OK") })
        .collect();

    let (results, wall_timeout_hit) = fan_out_with_wall_timeout(futures, Duration::from_secs(1)).await;

    assert_eq!(results.len(), 2);
    assert!(!wall_timeout_hit);
}

#[tokio::test(start_paused = true)]
async fn fan_out_preserves_results_completed_before_wall_timeout() {
    let futures = [1_u64, 500]
        .into_iter()
        .enumerate()
        .map(|(ordinal, delay_ms)| async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            completed_endpoint(ordinal, "duplicate:8080", 200, b"OK")
        })
        .collect();

    let (results, wall_timeout_hit) = fan_out_with_wall_timeout(futures, Duration::from_millis(50)).await;

    assert!(wall_timeout_hit);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].ordinal, 0);
}

#[tokio::test(start_paused = true)]
async fn fan_out_prefers_a_result_ready_at_the_wall_deadline() {
    let futures = [50_u64, 500]
        .into_iter()
        .enumerate()
        .map(|(ordinal, delay_ms)| async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            completed_endpoint(ordinal, "duplicate:8080", 200, b"OK")
        })
        .collect();

    let (results, wall_timeout_hit) = fan_out_with_wall_timeout(futures, Duration::from_millis(50)).await;

    assert!(wall_timeout_hit);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].ordinal, 0);
}

#[test]
fn ordinal_assembly_preserves_duplicates_statuses_and_raw_body_bytes() {
    let selected = vec![
        resolved_endpoint("10.0.0.1:8080", "first"),
        resolved_endpoint("10.0.0.1:8080", "first"),
        resolved_endpoint("10.0.0.1:8080", "second"),
        resolved_endpoint("10.0.0.1:8080", "second"),
    ];
    let mut backends = vec![backend_result("first", 2), backend_result("second", 3)];
    let completed = vec![
        completed_endpoint(3, "10.0.0.1:8080", 503, &[0xff, 0xfe]),
        completed_endpoint(2, "10.0.0.1:8080", 399, b"redirect"),
        completed_endpoint(0, "10.0.0.1:8080", 204, b"ok"),
    ];

    let aggregate = assemble_endpoint_results(&selected, completed, &mut backends);

    assert_eq!(
        aggregate,
        EndpointAggregate {
            success_count: 2,
            failure_count: 2,
            total_response_bytes: 12,
        }
    );
    assert_eq!(backends[0].endpoints.len(), 2);
    assert_eq!(backends[0].endpoints[0].status, Some(204));
    assert_eq!(backends[0].endpoints[1].error.as_deref(), Some("wall timeout exceeded"));
    assert_eq!(backends[1].endpoints.len(), 2);
    assert_eq!(backends[1].endpoints[0].status, Some(399));
    assert_eq!(backends[1].endpoints[1].status, Some(503));
    assert_ne!(backends[1].endpoints[1].body.as_deref().unwrap().len(), 2);
}

#[test]
fn inbound_body_headers_are_content_length_and_expect() {
    assert!(is_inbound_body_header("Content-Length"));
    assert!(is_inbound_body_header("expect"));
    assert!(
        !is_inbound_body_header("Transfer-Encoding"),
        "Transfer-Encoding is hop-by-hop, not an inbound-body header"
    );
    assert!(!is_inbound_body_header("authorization"));
    assert!(!is_inbound_body_header("host"));
    assert!(is_hop_by_hop_or_listed("Transfer-Encoding", &[]));
}

#[test]
fn method_preparation_rejects_invalid_overrides_and_preserves_tokens() {
    for value in ["", "BAD METHOD", " GET", "GET ", "G\tET", "GET\r\n", "G/ET", "méthod"] {
        let config = AllEndpointStatusConfig {
            method_override: Some(value.into()),
            ..Default::default()
        };
        assert!(AllEndpointStatus::new(&config).is_err(), "{value:?}");
    }
    for value in ["GET", "POST", "post", "X-Custom", "!#$%&'*+-.^_`|~"] {
        let config = AllEndpointStatusConfig {
            method_override: Some(value.into()),
            ..Default::default()
        };
        let plugin = AllEndpointStatus::new(&config).unwrap();
        assert_eq!(plugin.selected_method("HEAD").unwrap().as_str(), value);
        assert_eq!(plugin.selected_method("BAD METHOD").unwrap().as_str(), value);
    }
}

#[test]
fn missing_method_override_inherits_once_without_get_recovery() {
    let plugin = AllEndpointStatus::new(&AllEndpointStatusConfig::default()).unwrap();
    for value in ["HEAD", "DELETE", "post", "X-Custom"] {
        assert_eq!(plugin.selected_method(value).unwrap().as_str(), value);
    }
    for value in ["", "BAD METHOD", "GET\r\n", "G/ET"] {
        assert!(plugin.selected_method(value).is_err());
    }
}

#[tokio::test]
async fn json_response_transfers_owned_and_static_bytes_in_write_order() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;

    let plugin = AllEndpointStatus::new(&AllEndpointStatusConfig::default()).unwrap();
    for fail_writes in [false, true] {
        let owned = String::from(r#"{"value":"é"}"#);
        let owned_ptr = owned.as_ptr() as usize;
        let static_body = b"{}";
        let cases = [
            (Bytes::from(owned), owned_ptr, r#"{"value":"é"}"#.as_bytes()),
            (
                Bytes::from_static(static_body),
                static_body.as_ptr() as usize,
                static_body.as_slice(),
            ),
            (Bytes::new(), 0, b"".as_slice()),
        ];
        for (body, original_ptr, expected) in cases {
            let mut session = MockPluginSession::new();
            let mut sequence = Sequence::new();
            session
                .expect_write_response_header()
                .withf(move |header, eos| {
                    let mut wire = Vec::new();
                    header.header_to_h1_wire(&mut wire);
                    let retry_after = b"Retry-After: 2\r\n";
                    !*eos
                        && wire.windows(retry_after.len()).any(|part| part == retry_after)
                        && header.status.as_u16() == 429
                        && header.headers["content-length"] == expected.len().to_string()
                        && header.headers["content-type"] == "application/json"
                        && header.headers["connection"] == "close"
                        && header.headers["cache-control"] == "no-store"
                        && header.headers["retry-after"] == "2"
                })
                .times(1)
                .in_sequence(&mut sequence)
                .returning(move |_, _| {
                    if fail_writes {
                        Err(std::io::Error::other("header write failed").into())
                    } else {
                        Ok(())
                    }
                });
            session
                .expect_write_response_body()
                .withf(move |body, eos| {
                    *eos && body.as_ref().is_some_and(|body| {
                        body.as_ref() == expected && (expected.is_empty() || body.as_ptr() as usize == original_ptr)
                    })
                })
                .times(1)
                .in_sequence(&mut sequence)
                .returning(move |_, _| {
                    if fail_writes {
                        Err(std::io::Error::other("body write failed").into())
                    } else {
                        Ok(())
                    }
                });
            session
                .expect_shutdown()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| {});
            assert_eq!(
                plugin
                    .send_json_response_with_headers(&mut session, 429, body, &[("Retry-After", "2")])
                    .await,
                PluginRunningResult::ErrTerminateRequest
            );
        }
    }
}
