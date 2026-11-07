use std::collections::HashMap;

use super::*;
use edgion_resources::resources::gateway::GatewaySpec;
use edgion_resources::resources::proxy_protocol::ProxyProtocolPolicySourceKind;

fn gateway(name: &str, port: i32, state: ResolvedInboundProxyProtocol) -> Gateway {
    let mut gateway = Gateway::new(
        name,
        GatewaySpec {
            current_status: None,
            gateway_class_name: "edgion".to_string(),
            listeners: Some(vec![Listener {
                name: "tcp".to_string(),
                hostname: None,
                port,
                protocol: "TCP".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: state,
            resolved_attachment_proof: None,
        },
    );
    gateway.metadata.namespace = Some("default".to_string());
    gateway
}

fn ready(policy: CanonicalProxyProtocolTrustPolicy) -> ResolvedInboundProxyProtocol {
    ResolvedInboundProxyProtocol::Ready {
        listener_names: vec!["tcp".to_string()],
        policy,
        source: ProxyProtocolPolicySourceKind::Inline,
    }
}

/// Listener identity follows the local bind IP, not `spec.addresses`.
fn with_bind_ip(mut gateway: Gateway, ip: &str) -> Gateway {
    gateway
        .metadata
        .annotations
        .get_or_insert_with(Default::default)
        .insert(
            edgion_resources::constants::annotations::edgion::LOCAL_BIND_IP.to_string(),
            ip.to_string(),
        );
    gateway
}

#[test]
fn preflight_mapping_and_trusted_matcher_are_exact() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let required = gateway("required", 18080, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    let trusted = gateway(
        "trusted",
        18081,
        ready(CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: vec!["10.0.0.0/8".to_string()],
        }),
    );
    assert!(matches!(
        runtime.observe_snapshot([&required, &trusted]),
        SnapshotDisposition::Publish
    ));
    let prepared = runtime.preflight_and_freeze([&required, &trusted]).unwrap();

    let required_key = ListenerKey::new(resolve_listener_addr(&required, 18080), "TCP");
    assert!(matches!(
        prepared.get(&required_key),
        Some(PreparedProxyProtocolPolicy::Required)
    ));
    let trusted_key = ListenerKey::new(resolve_listener_addr(&trusted, 18081), "TCP");
    let Some(PreparedProxyProtocolPolicy::TrustedSources { trust, cidr_count }) = prepared.get(&trusted_key) else {
        panic!("trustedSources must prepare a matcher")
    };
    assert_eq!(*cidr_count, 1);
    assert!(trust.is_trusted(&"10.2.3.4:1".parse().unwrap()));
    assert!(!trust.is_trusted(&"192.0.2.1:1".parse().unwrap()));
}

#[test]
fn shared_listener_requires_identical_declarations() {
    let required = gateway("a", 18082, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    let disabled = gateway("b", 18082, ResolvedInboundProxyProtocol::Disabled);
    let error = aggregate_snapshot([&required, &disabled]).unwrap_err();
    assert_eq!(error.code, "SharedListenerPolicyConflict");
    assert_eq!(error.gateway_key.as_deref(), Some("default/b"));
    assert_eq!(error.conflicting_gateway_key.as_deref(), Some("default/a"));
    assert_eq!(error.listener_key.unwrap().protocol(), "TCP");
}

#[test]
fn initial_snapshot_does_not_trip_but_every_transition_is_monotonic() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let disabled = gateway("a", 18083, ResolvedInboundProxyProtocol::Disabled);
    assert!(matches!(
        runtime.observe_snapshot([&disabled]),
        SnapshotDisposition::Publish
    ));
    runtime.preflight_and_freeze([&disabled]).unwrap();
    assert!(matches!(
        runtime.observe_snapshot([&disabled]),
        SnapshotDisposition::Publish
    ));

    let required = gateway("a", 18083, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    assert!(matches!(
        runtime.observe_snapshot([&required]),
        SnapshotDisposition::RejectFatal
    ));
    assert!(runtime.is_fatal());
    assert!(matches!(
        runtime.observe_snapshot([&disabled]),
        SnapshotDisposition::RejectFatal
    ));
    assert!(runtime.is_fatal(), "reverting must never clear the fatal guard");
}

#[test]
fn redundant_same_policy_contributor_add_remove_and_reorder_do_not_trip() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let first = gateway("a", 18084, ResolvedInboundProxyProtocol::Disabled);
    let second = gateway("b", 18084, ResolvedInboundProxyProtocol::Disabled);
    runtime.observe_snapshot([&first]);
    runtime.preflight_and_freeze([&first]).unwrap();
    assert!(matches!(
        runtime.observe_snapshot([&first, &second]),
        SnapshotDisposition::Publish
    ));
    assert!(matches!(
        runtime.observe_snapshot([&second, &first]),
        SnapshotDisposition::Publish
    ));
    assert!(matches!(
        runtime.observe_snapshot([&first]),
        SnapshotDisposition::Publish
    ));
    assert!(!runtime.is_fatal());
}

#[test]
fn every_restart_scoped_policy_dimension_trips_the_guard() {
    let cases = [
        (
            gateway("a", 18100, ResolvedInboundProxyProtocol::Disabled),
            gateway("a", 18100, ready(CanonicalProxyProtocolTrustPolicy::Required {})),
        ),
        (
            gateway("a", 18101, ready(CanonicalProxyProtocolTrustPolicy::Required {})),
            gateway(
                "a",
                18101,
                ready(CanonicalProxyProtocolTrustPolicy::TrustedSources {
                    trusted_cidrs: vec!["10.0.0.0/8".to_string()],
                }),
            ),
        ),
        (
            gateway(
                "a",
                18102,
                ready(CanonicalProxyProtocolTrustPolicy::TrustedSources {
                    trusted_cidrs: vec!["10.0.0.0/8".to_string()],
                }),
            ),
            gateway(
                "a",
                18102,
                ready(CanonicalProxyProtocolTrustPolicy::TrustedSources {
                    trusted_cidrs: vec!["192.0.2.0/24".to_string()],
                }),
            ),
        ),
        (
            with_bind_ip(
                gateway("a", 18103, ready(CanonicalProxyProtocolTrustPolicy::Required {})),
                "127.0.0.1",
            ),
            with_bind_ip(
                gateway("a", 18103, ready(CanonicalProxyProtocolTrustPolicy::Required {})),
                "127.0.0.2",
            ),
        ),
    ];

    for (initial, changed) in cases {
        let runtime = ProxyProtocolPolicyRuntime::new();
        runtime.observe_snapshot([&initial]);
        runtime.preflight_and_freeze([&initial]).unwrap();
        assert!(matches!(
            runtime.observe_snapshot([&changed]),
            SnapshotDisposition::RejectFatal
        ));
    }

    let runtime = ProxyProtocolPolicyRuntime::new();
    let initial = gateway("remove", 18104, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    runtime.observe_snapshot([&initial]);
    runtime.preflight_and_freeze([&initial]).unwrap();
    assert!(matches!(
        runtime.observe_snapshot(std::iter::empty()),
        SnapshotDisposition::RejectFatal
    ));
}

#[test]
fn disabled_listener_add_remove_and_readd_do_not_trip() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let initial = gateway("remove", 18130, ResolvedInboundProxyProtocol::Disabled);
    runtime.observe_snapshot([&initial]);
    runtime.preflight_and_freeze([&initial]).unwrap();
    assert!(matches!(
        runtime.observe_snapshot(std::iter::empty()),
        SnapshotDisposition::Publish
    ));
    assert!(!runtime.is_fatal());
    assert!(matches!(
        runtime.observe_snapshot([&initial]),
        SnapshotDisposition::Publish
    ));
    let extra = gateway("extra", 18131, ResolvedInboundProxyProtocol::Disabled);
    assert!(matches!(
        runtime.observe_snapshot([&initial, &extra]),
        SnapshotDisposition::Publish
    ));
    assert!(!runtime.is_fatal());
}

#[test]
fn listener_identity_change_with_same_socket_declaration_does_not_trip() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let initial = gateway("a", 18105, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    runtime.observe_snapshot([&initial]);
    runtime.preflight_and_freeze([&initial]).unwrap();

    let mut changed = initial.clone();
    changed.spec.listeners.as_mut().unwrap()[0].name = "renamed".to_string();
    if let ResolvedInboundProxyProtocol::Ready { listener_names, .. } =
        &mut changed.spec.resolved_inbound_proxy_protocol
    {
        *listener_names = vec!["renamed".to_string()];
    }
    assert!(matches!(
        runtime.observe_snapshot([&changed]),
        SnapshotDisposition::Publish
    ));
    assert!(!runtime.is_fatal());
}

#[test]
fn impossible_ready_and_invalid_transport_policy_abort_startup() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let mut udp = gateway("udp", 18106, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    udp.spec.listeners.as_mut().unwrap()[0].protocol = "UDP".to_string();
    runtime.observe_snapshot([&udp]);
    assert!(runtime
        .preflight_and_freeze([&udp])
        .unwrap_err()
        .to_string()
        .contains("ImpossibleReadyState"));

    let invalid_sets = vec![
        vec![],
        vec!["0.0.0.0/0".to_string()],
        vec!["not-a-cidr".to_string()],
        vec!["x".repeat(65)],
        vec!["10.0.0.0/8".to_string(), "10.0.0.0/8".to_string()],
        vec!["192.0.2.0/24".to_string(), "10.0.0.0/8".to_string()],
        vec!["10.1.2.3/8".to_string()],
        vec!["192.0.2.1".to_string()],
    ];
    for (index, trusted_cidrs) in invalid_sets.into_iter().enumerate() {
        let runtime = ProxyProtocolPolicyRuntime::new();
        let invalid = gateway(
            &format!("invalid-{index}"),
            18107 + index as i32,
            ready(CanonicalProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs }),
        );
        assert!(matches!(
            runtime.observe_snapshot([&invalid]),
            SnapshotDisposition::Publish
        ));
        assert!(runtime
            .preflight_and_freeze([&invalid])
            .unwrap_err()
            .to_string()
            .contains("InvalidTransportPolicy"));
    }

    let runtime = ProxyProtocolPolicyRuntime::new();
    let over_limit = gateway(
        "over-limit",
        18120,
        ready(CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: (0..=256)
                .map(|index| format!("10.{}.{}.1/32", index / 256, index % 256))
                .collect(),
        }),
    );
    runtime.observe_snapshot([&over_limit]);
    assert!(runtime
        .preflight_and_freeze([&over_limit])
        .unwrap_err()
        .to_string()
        .contains("InvalidTransportPolicy"));
}

#[tokio::test]
async fn fatal_guard_runs_before_connection_accounting_and_stream_filter() {
    use crate::plugins::stream::StreamPluginConnectionFilter;
    use crate::runtime::server::downstream_connections::{ConnectionCounter, ListenerAdmissionFilter};
    use pingora_core::listeners::ConnectionAdmission;

    let runtime = Arc::new(ProxyProtocolPolicyRuntime::new());
    let disabled = gateway("a", 18085, ResolvedInboundProxyProtocol::Disabled);
    runtime.observe_snapshot([&disabled]);
    runtime.preflight_and_freeze([&disabled]).unwrap();
    let required = gateway("a", 18085, ready(CanonicalProxyProtocolTrustPolicy::Required {}));
    runtime.observe_snapshot([&required]);
    assert!(runtime.is_fatal());

    let global = Arc::new(ConnectionCounter::unlimited());
    let listener = Arc::new(ConnectionCounter::unlimited());
    let filter = ListenerAdmissionFilter::new(
        runtime,
        Arc::clone(&global),
        Arc::clone(&listener),
        StreamPluginConnectionFilter::new(ListenerKey::new("0.0.0.0:18085".parse().unwrap(), "TCP"), 18085),
    );
    // An accepting chain isolates the guard: a tripped policy must reject
    // without counting the socket or running the chain.
    let ran = std::sync::atomic::AtomicBool::new(false);
    let admission = filter
        .admit_with(|_| {
            ran.store(true, std::sync::atomic::Ordering::Relaxed);
            async { true }
        })
        .await;
    assert!(matches!(admission, ConnectionAdmission::Reject));
    assert!(!ran.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(global.active(), 0);
    assert_eq!(listener.active(), 0);
}

#[test]
fn full_relist_identity_is_order_independent() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let first = gateway("a", 18086, ResolvedInboundProxyProtocol::Disabled);
    let second = gateway("b", 18087, ResolvedInboundProxyProtocol::Disabled);
    runtime.observe_snapshot([&first, &second]);
    runtime.preflight_and_freeze([&first, &second]).unwrap();
    assert!(matches!(
        runtime.observe_snapshot([&second, &first]),
        SnapshotDisposition::Publish
    ));
}

#[test]
fn invalid_state_fails_startup_with_bounded_code() {
    let runtime = ProxyProtocolPolicyRuntime::new();
    let invalid = gateway(
        "bad",
        18088,
        ResolvedInboundProxyProtocol::Invalid {
            code: ProxyProtocolConfigErrorCode::InvalidPolicy,
            listener_names: vec!["tcp".to_string()],
            reference: None,
        },
    );
    runtime.observe_snapshot([&invalid]);
    let error = runtime.preflight_and_freeze([&invalid]).unwrap_err().to_string();
    assert!(error.contains("InvalidPolicy"));
    assert!(!error.contains("trustedCidrs"));
}

#[test]
fn prospective_snapshot_helper_has_stable_gateway_keys() {
    let gateway = gateway("stable", 18089, ResolvedInboundProxyProtocol::Disabled);
    let map = HashMap::from([(gateway.key_name(), Arc::new(gateway))]);
    let snapshot = aggregate_snapshot(map.values().map(Arc::as_ref)).unwrap();
    assert_eq!(snapshot.len(), 1);
}
