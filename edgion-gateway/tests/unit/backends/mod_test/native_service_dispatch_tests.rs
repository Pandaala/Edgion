use super::*;
use crate::backends::discovery::ServiceDiscoveryCoordinator;
use crate::routes::grpc::{GrpcBackendHandle, GrpcRouteRuleUnit, SelectedGrpcBackend};
use crate::routes::http::match_unit::HttpRouteRuleUnit;
use edgion_resources::{GRPCRoute, HTTPRoute};
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use tokio::io::AsyncWriteExt;

fn logical(namespace: Option<&str>, backend: Value) -> Option<LogicalBackendHandle> {
    let route: HTTPRoute = serde_json::from_value(json!({
        "metadata":{"name":"owner","namespace":namespace},
        "spec":{"rules":[{"backendRefs":[backend]}]}
    }))
    .unwrap();
    let runtime = HttpRouteRuleUnit::compile_http_rule(&route, 0).ok()?;
    LogicalBackendHandle::new(runtime.backends, 0)
}

fn selected(namespace: Option<&str>, port: u16, tls: BackendTlsPolicyLookup) -> SelectedServiceBackend {
    SelectedServiceBackend::new(logical(namespace, json!({"name":"backend","port":port})).unwrap(), tls).unwrap()
}

fn grpc_selected(namespace: Option<&str>, port: u16, tls: BackendTlsPolicyLookup) -> SelectedGrpcBackend {
    let route: GRPCRoute = serde_json::from_value(json!({
        "metadata":{"name":"owner","namespace":namespace},
        "spec":{"rules":[{"backendRefs":[{"name":"backend","port":port}]}]}
    }))
    .unwrap();
    let runtime = GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap();
    SelectedGrpcBackend::new(GrpcBackendHandle::new(runtime.backends, 0).unwrap(), tls).unwrap()
}

fn context(key: &str) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(crate::BackendContext {
        source: None,
        rsc: None,
        service_key: Some(key.to_owned()),
        service_kind: Some(ServiceBackendKind::Service),
        upstreams: vec![],
        current_upstream_id: None,
        backend_ref_idx: Some(0),
        effective_policy_captured: false,
        effective_policy: None,
    });
    ctx.push_upstream(None, None);
    ctx
}

async fn session() -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    session
}

fn service(kind: Option<&str>, hostname: Option<&str>, mode: Option<&str>, port: u16) -> Arc<Service> {
    let annotations = mode.map(|mode| BTreeMap::from([(edgion_annotations::DNS_RESOLUTION_MODE, mode)]));
    Arc::new(serde_json::from_value(json!({
        "metadata":{"name":"backend","namespace":"default","uid":"source-a","resourceVersion":"1","annotations":annotations},
        "spec":{"type":kind,"externalName":hostname,"ports":[{"name":"http","port":port,"targetPort":8080,"protocol":"TCP"}]}
    })).unwrap())
}

fn publish(coordinator: &ServiceDiscoveryCoordinator, service: Arc<Service>, ready: Option<bool>) {
    coordinator.replace_services(&HashMap::from([("default/backend".to_owned(), service)]));
    let slices = ready.map(|ready| {
        let slice: EndpointSlice = serde_json::from_value(json!({
            "metadata":{"name":"backend-slice","namespace":"default","uid":"slice-a","labels":{"kubernetes.io/service-name":"backend"}},
            "addressType":"IPv4","ports":[{"name":"http","port":8080,"protocol":"TCP"}],
            "endpoints":[{"addresses":["192.0.2.8"],"conditions":{"ready":ready}}]
        })).unwrap();
        HashMap::from([("default/backend-slice".to_owned(), Arc::new(slice))])
    }).unwrap_or_default();
    coordinator.replace_endpoint_slices(&slices);
}

fn capture(coordinator: &ServiceDiscoveryCoordinator, selected: &SelectedServiceBackend) -> CapturedServiceSelection {
    CapturedServiceSelection::capture(coordinator, selected.logical().service_target().unwrap()).unwrap()
}

fn input(selected: &SelectedServiceBackend) -> ServicePeerInput<'_> {
    ServicePeerInput {
        service: selected.logical().service_target().unwrap(),
        backend_tls_policy: selected.backend_tls_policy(),
    }
}

#[tokio::test]
async fn actual_http_and_grpc_default_owner_reach_standard_external_name_peer() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let session = session().await;
    for kind in [None, Some("ClusterIP"), Some("ExternalName")] {
        publish(
            &coordinator,
            service(kind, Some("192.0.2.44"), Some("gateway"), 443),
            Some(true),
        );
        for grpc in [false, true] {
            let selected = SelectedLogicalBackend::Service(selected(None, 443, BackendTlsPolicyLookup::Absent));
            let mut ctx = context("default/backend");
            let policy = super::selected_service_input_tests::named_captured_policy();
            let backend = ctx.backend_context.as_mut().unwrap();
            backend.effective_policy = Some(Arc::clone(&policy));
            backend.effective_policy_captured = true;
            if grpc {
                ctx.selected_grpc_backend = Some(grpc_selected(None, 443, BackendTlsPolicyLookup::Absent));
            }
            let reads = Cell::new(0);
            let peer = try_get_peer_with_capture(&mut ctx, &session, grpc, Some(&selected), |target| {
                reads.set(reads.get() + 1);
                assert_eq!(target.key(), "default/backend");
                CapturedServiceSelection::capture(&coordinator, target)
            })
            .await
            .unwrap();
            let expected = if kind == Some("ExternalName") {
                "192.0.2.44:443"
            } else {
                "192.0.2.8:8080"
            };
            assert_eq!(peer._address.as_inet().unwrap().to_string(), expected);
            assert!(!peer.is_tls(), "absence must stay plaintext even at port 443");
            assert_eq!(reads.get(), 1);
            assert!(Arc::ptr_eq(
                ctx.backend_context.as_ref().unwrap().effective_policy.as_ref().unwrap(),
                &policy
            ));
            assert_eq!(
                ctx.get_current_upstream().unwrap().lb_policy,
                if kind == Some("ExternalName") {
                    None
                } else {
                    policy.load_balancer.clone()
                }
            );
        }
    }
}

#[tokio::test]
async fn actual_native_input_rejection_precedes_projection_and_dns_work() {
    let session = session().await;
    for backend in [
        json!({"group":"core","name":"backend","port":80}),
        json!({"kind":"ServiceExternalName","name":"backend","port":80}),
        json!({"kind":"ServiceImport","name":"backend","port":80}),
        json!({"name":"backend"}),
        json!({"name":"backend","port":0}),
        json!({"name":"backend","port":65536}),
        json!({"name":"","port":80}),
        json!({"namespace":"","name":"backend","port":80}),
        json!({"name":"backend","port":80,"refDenied":{"targetNamespace":"default","targetName":"backend"}}),
    ] {
        let mut ctx = context("default/backend");
        let logical = logical(None, backend).unwrap();
        assert!(SelectedServiceBackend::new(logical.clone(), BackendTlsPolicyLookup::Absent).is_none());
        ctx.selected_backend = Some(SelectedHttpBackend::Invalid(logical));
        let reads = Cell::new(0);
        let result = try_get_peer_with_capture(&mut ctx, &session, false, None, |_| {
            reads.set(reads.get() + 1);
            Err(EdgionErrorCode::BackendServiceNotFound)
        })
        .await;
        assert_eq!(result.unwrap_err(), EdgionErrorCode::BackendUnsupportedKind);
        assert_eq!(reads.get(), 0);
    }
    for backend in [
        json!({"name":"backend"}),
        json!({"group":"core","name":"backend","port":80}),
        json!({"name":"backend","port":80,"refDenied":{"targetNamespace":"default","targetName":"backend"}}),
    ] {
        let route: GRPCRoute =
            serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[backend]}]}}))
                .unwrap();
        let runtime = GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap();
        assert!(SelectedGrpcBackend::new(
            GrpcBackendHandle::new(runtime.backends, 0).unwrap(),
            BackendTlsPolicyLookup::Absent
        )
        .is_none());
        let mut ctx = context("default/backend");
        let reads = Cell::new(0);
        assert_eq!(
            try_get_peer_with_capture(&mut ctx, &session, true, None, |_| {
                reads.set(reads.get() + 1);
                Err(EdgionErrorCode::BackendServiceNotFound)
            })
            .await
            .unwrap_err(),
            EdgionErrorCode::GrpcUpstreamNotBackendRefs
        );
        assert_eq!(reads.get(), 0);
    }
    for namespace in ["", "Bad Namespace"] {
        let route: GRPCRoute=serde_json::from_value(json!({"metadata":{"name":"owner","namespace":namespace},"spec":{"rules":[{"backendRefs":[{"name":"backend","port":80}]}]}})).unwrap();
        assert!(GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).is_none());
    }
    assert!(logical(Some(""), json!({"name":"backend","port":80})).is_none());
    assert!(logical(Some("Bad Namespace"), json!({"name":"backend","port":80})).is_none());
    for (grpc, tls_invalid, mismatch) in [
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        let tls = if tls_invalid {
            BackendTlsPolicyLookup::Invalid
        } else {
            BackendTlsPolicyLookup::Absent
        };
        let selected = SelectedLogicalBackend::Service(selected(None, 80, tls.clone()));
        let mut ctx = context(if mismatch { "default/other" } else { "default/backend" });
        if grpc {
            ctx.selected_grpc_backend = Some(grpc_selected(None, 80, tls));
        }
        let calls = Cell::new(0);
        let result = try_get_peer_with_capture(&mut ctx, &session, grpc, Some(&selected), |_| {
            calls.set(calls.get() + 1);
            Err(EdgionErrorCode::BackendServiceNotFound)
        })
        .await;
        assert_eq!(
            result.unwrap_err(),
            if mismatch {
                EdgionErrorCode::UpstreamNotBackendRefs
            } else {
                EdgionErrorCode::BackendTlsPolicyInvalid
            }
        );
        assert_eq!(calls.get(), 0);
    }
}

#[tokio::test]
async fn missing_service_section_and_hostname_have_fixed_failures_without_resolution() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let selected = SelectedLogicalBackend::Service(selected(None, 80, BackendTlsPolicyLookup::Absent));
    let session = session().await;
    for (source, expected) in [
        (None, EdgionErrorCode::BackendServiceNotFound),
        (
            Some(service(
                Some("ExternalName"),
                Some("unused.test"),
                Some("per-request"),
                81,
            )),
            EdgionErrorCode::BackendPortResolutionFailed,
        ),
        (
            Some(service(Some("ExternalName"), None, Some("per-request"), 80)),
            EdgionErrorCode::BackendExternalNameNotFound,
        ),
        (
            Some(service(Some("ExternalName"), Some(""), Some("per-request"), 80)),
            EdgionErrorCode::BackendExternalNameNotFound,
        ),
    ] {
        if let Some(source) = source {
            publish(&coordinator, source, None);
        }
        let mut ctx = context("default/backend");
        assert_eq!(
            try_get_peer_with_capture(&mut ctx, &session, false, Some(&selected), |target| {
                CapturedServiceSelection::capture(&coordinator, target)
            })
            .await
            .unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn controller_readiness_does_not_bypass_runtime_health_and_other_modes_ignore_slices() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let selected = selected(None, 80, BackendTlsPolicyLookup::Absent);
    for (mode, ready, expected) in [
        (None, true, ExternalNamePath::EndpointSlice),
        (Some("controller"), false, ExternalNamePath::SelfResolve { cache: true }),
        (Some("gateway"), true, ExternalNamePath::SelfResolve { cache: true }),
        (
            Some("per-request"),
            true,
            ExternalNamePath::SelfResolve { cache: false },
        ),
    ] {
        publish(
            &coordinator,
            service(Some("ExternalName"), Some("192.0.2.40"), mode, 80),
            Some(ready),
        );
        let captured = capture(&coordinator, &selected);
        assert_eq!(native_service_path(&captured).unwrap(), expected);
        let mut ctx = context("default/backend");
        let choice = SelectedLogicalBackend::Service(selected.clone());
        let peer = try_get_peer_with_capture(&mut ctx, &session().await, false, Some(&choice), |target| {
            CapturedServiceSelection::capture(&coordinator, target)
        })
        .await
        .unwrap();
        let expected_address = if expected == ExternalNamePath::EndpointSlice {
            "192.0.2.8:8080"
        } else {
            "192.0.2.40:80"
        };
        assert_eq!(peer._address.as_inet().unwrap().to_string(), expected_address);
        if ready {
            let generation = coordinator.load_generation();
            let active = generation
                .available("default/backend")
                .unwrap()
                .runtime_cell
                .active
                .load_full();
            for state in active.entries.values() {
                state.set_health_eligible(false);
            }
            assert_eq!(native_service_path(&captured).unwrap(), expected);
            assert!(captured
                .select(&None, ServiceDegradePolicy::CapturedOverride(Some(0)), None, "", None)
                .is_err());
        }
    }
}

#[tokio::test]
async fn actual_dns_stage_keeps_old_service_geometry_across_await_and_next_capture_recovers() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut old = (*service(Some("ExternalName"), Some("old.example.test"), Some("per-request"), 80)).clone();
    old.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some("kubernetes.io/h2c".into());
    old.metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(edgion_annotations::DNS_RESOLVER.into(), "192.0.2.53".into());
    publish(&coordinator, Arc::new(old), None);
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let spec = compute_external_name_resolver_with(
        captured.dns_resolver_candidate(),
        &context("default/backend"),
        None,
        |_| None,
    );
    let cache = DnsCache::with_capacity(8, 1);
    let mut ctx = context("default/backend");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let dns = ExternalNameResolution {
        spec: &spec,
        dns_cache: &cache,
        cache: true,
        ttl: Duration::from_secs(30),
    };
    let peer_input = input(&chosen);
    let spec_ref = &spec;
    let attempt = build_external_name_peer_with_resolver(
        &mut ctx,
        &peer_input,
        &captured,
        None,
        dns,
        |seen_spec, hostname| async move {
            assert!(std::ptr::eq(seen_spec, spec_ref));
            assert!(matches!(seen_spec,ResolverSpec::StandardDns{servers} if servers==&["192.0.2.53"]));
            assert_eq!(hostname, "old.example.test");
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            assert!(matches!(seen_spec,ResolverSpec::StandardDns{servers} if servers==&["192.0.2.53"]));
            assert_eq!(hostname, "old.example.test");
            Ok(vec!["192.0.2.10".parse().unwrap()])
        },
    );
    let update = async {
        entered_rx.await.unwrap();
        let mut changed = (*service(Some("ExternalName"), Some("new.example.test"), Some("gateway"), 81)).clone();
        changed
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(edgion_annotations::DNS_RESOLVER.into(), "192.0.2.54".into());
        changed.metadata.uid = Some("source-b".into());
        changed.metadata.resource_version = Some("2".into());
        changed.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].protocol = Some("UDP".into());
        publish(&coordinator, Arc::new(changed), None);
        release_tx.send(()).unwrap();
    };
    let (peer, ()) = tokio::join!(attempt, update);
    let peer = peer.unwrap();
    assert_eq!(peer._address.as_inet().unwrap().to_string(), "192.0.2.10:80");
    assert_eq!(peer.options.alpn, pingora_core::protocols::tls::ALPN::H2);
    assert_eq!(captured.source().metadata.uid.as_deref(), Some("source-a"));
    assert_eq!(captured.source().metadata.resource_version.as_deref(), Some("1"));
    assert_eq!(captured.section().identity.protocol.as_ref(), "TCP");
    assert!(matches!(
        CapturedServiceSelection::capture(&coordinator, chosen.logical().service_target().unwrap()),
        Err(EdgionErrorCode::BackendPortResolutionFailed)
    ));
    let current = selected(None, 81, BackendTlsPolicyLookup::Absent);
    let new = capture(&coordinator, &current);
    assert_eq!(external_name(&new).unwrap(), "new.example.test");
    assert_eq!(new.source().metadata.uid.as_deref(), Some("source-b"));
    let current_spec =
        compute_external_name_resolver_with(new.dns_resolver_candidate(), &context("default/backend"), None, |_| {
            None
        });
    assert_ne!(current_spec.fingerprint(), spec.fingerprint());
    assert_eq!(new.section().identity.protocol.as_ref(), "UDP");
    assert_eq!(
        native_service_path(&new).unwrap(),
        ExternalNamePath::SelfResolve { cache: true }
    );
    assert!(cache
        .get(
            "default/backend",
            "new.example.test",
            &spec.fingerprint(),
            Instant::now()
        )
        .is_none());
    coordinator.replace_services(&HashMap::new());
    assert!(matches!(
        CapturedServiceSelection::capture(&coordinator, current.logical().service_target().unwrap()),
        Err(EdgionErrorCode::BackendServiceNotFound)
    ));
    assert_eq!(external_name(&captured).unwrap(), "old.example.test");
}

#[tokio::test]
async fn actual_dns_cache_query_boundary_preserves_identity_modes_ttl_and_no_negative_entries() {
    let cache = DnsCache::with_capacity(16, 2);
    let spec = ResolverSpec::System;
    let queries = Cell::new(0);
    for (name, cached, ttl, expected) in [
        ("a.test", true, 30, 1),
        ("a.test", true, 30, 1),
        ("b.test", true, 30, 2),
        ("a.test", false, 30, 3),
        ("a.test", false, 30, 4),
        ("zero.test", true, 0, 5),
        ("zero.test", true, 0, 6),
    ] {
        let answer = resolve_external_ips_with(
            name,
            &spec,
            "default/backend",
            cached,
            Duration::from_secs(ttl),
            &cache,
            |_, seen| {
                assert_eq!(seen, name);
                queries.set(queries.get() + 1);
                std::future::ready(Ok(vec!["192.0.2.4".parse().unwrap()]))
            },
        )
        .await
        .unwrap();
        assert_eq!(answer.len(), 1);
        assert_eq!(queries.get(), expected);
    }
    for literal in ["192.0.2.7", "2001:db8::7"] {
        assert_eq!(
            resolve_external_ips_with(
                literal,
                &spec,
                "default/backend",
                true,
                Duration::from_secs(30),
                &cache,
                |_, _| {
                    queries.set(queries.get() + 1);
                    std::future::ready(Err(DnsError::NoAddresses))
                }
            )
            .await
            .unwrap(),
            vec![literal.parse::<IpAddr>().unwrap()]
        );
    }
    assert_eq!(queries.get(), 6, "IP literals cannot query DNS");
    for _ in 0..2 {
        assert!(resolve_external_ips_with(
            "failed.test",
            &spec,
            "default/backend",
            true,
            Duration::from_secs(30),
            &cache,
            |_, _| {
                queries.set(queries.get() + 1);
                std::future::ready(Err(DnsError::NoAddresses))
            }
        )
        .await
        .is_err());
    }
    assert_eq!(queries.get(), 8);
    let other = ResolverSpec::StandardDns {
        servers: vec!["192.0.2.53".into()],
    };
    resolve_external_ips_with(
        "a.test",
        &other,
        "default/backend",
        true,
        Duration::from_secs(30),
        &cache,
        |_, _| {
            queries.set(queries.get() + 1);
            std::future::ready(Ok(vec!["192.0.2.99".parse().unwrap()]))
        },
    )
    .await
    .unwrap();
    assert_eq!(queries.get(), 9);
}

#[tokio::test]
async fn sticky_validation_uses_captured_section_and_self_dns_ignores_stale_target() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    publish(&coordinator, service(None, None, None, 80), Some(true));
    let old = capture(&coordinator, &chosen);
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("192.0.2.90"), Some("gateway"), 80),
        None,
    );
    let mut ctx = context("default/backend");
    ctx.ensure_session_persistence().target_addr = Some("192.0.2.8:8080".parse().unwrap());
    let endpoint = take_sp_target_or_select(&mut ctx, &old, |_| panic!("captured sticky slot must win")).unwrap();
    assert_eq!(endpoint.serving_port.get(), 8080);
    assert!(ctx.session_persistence().unwrap().sticky_consumed);
    let current = capture(&coordinator, &chosen);
    ctx.ensure_session_persistence().target_addr = Some("192.0.2.8:8080".parse().unwrap());
    assert!(matches!(
        take_sp_target_or_select(&mut ctx, &current, |_| panic!(
            "missing sticky is a terminal result here"
        )),
        Err(EdgionErrorCode::StickyBackendUnhealthy)
    ));
    let mut dnsctx = context("default/backend");
    let stale = "169.254.169.254:80".parse().unwrap();
    dnsctx.ensure_session_persistence().target_addr = Some(stale);
    let logical = SelectedLogicalBackend::Service(chosen);
    let peer = try_get_peer_with_capture(&mut dnsctx, &session().await, false, Some(&logical), |target| {
        CapturedServiceSelection::capture(&coordinator, target)
    })
    .await
    .unwrap();
    assert_eq!(peer._address.as_inet().unwrap().to_string(), "192.0.2.90:80");
    assert_eq!(dnsctx.session_persistence().unwrap().target_addr, Some(stale));
    assert!(!dnsctx.session_persistence().unwrap().sticky_consumed);
}

#[tokio::test]
async fn peer_construction_transfers_captured_lb_policy_into_attempt() {
    use crate::observe::metrics::TestType;
    use edgion_resources::resources::lb_policy::ConsistentHashOn;
    use pingora_core::upstreams::peer::Peer;

    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(&coordinator, service(None, None, None, 80), Some(true));
    let selected = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &selected);
    let session = session().await;
    let mut transferred = Vec::new();
    for test_metrics in [false, true] {
        for policy in [
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
                "x-routing-key".to_owned(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie(
                "routing-cookie".to_owned(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg(
                "routing-query".to_owned(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp)),
            Some(ParsedLBPolicy::LeastConn),
            Some(ParsedLBPolicy::Ewma),
            None,
        ] {
            let expected = serde_json::to_value(&policy).unwrap();
            let expected_hash = if test_metrics {
                match &policy {
                    Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp)) => Some("2001:db8::42"),
                    Some(ParsedLBPolicy::ConsistentHash(_)) => Some(""),
                    _ => None,
                }
            } else {
                None
            };
            let original_ptr = match &policy {
                Some(ParsedLBPolicy::ConsistentHash(
                    ConsistentHashOn::Header(name) | ConsistentHashOn::Cookie(name) | ConsistentHashOn::Arg(name),
                )) => Some(name.as_ptr()),
                _ => None,
            };
            let endpoint = captured
                .select(&None, ServiceDegradePolicy::Live, None, "", None)
                .unwrap();
            let backend = Arc::clone(&endpoint.backend);
            let mut ctx = context("default/backend");
            ctx.request_info.remote_ip = "2001:db8::42".to_owned();
            let remote_ip_ptr = ctx.request_info.remote_ip.as_ptr();
            ctx.gateway_info = Some(Arc::new(GatewayInfo::new(
                None,
                "gateway".to_owned(),
                None,
                None,
                test_metrics.then_some(TestType::Lb),
                None,
                Vec::new(),
                None,
            )));
            let peer = build_peer_from_lb_selection(
                &mut ctx,
                &session,
                "default/backend",
                policy,
                &BackendTlsPolicyLookup::Absent,
                endpoint,
                None,
            )
            .unwrap();
            assert_eq!(
                peer.address().as_inet().copied(),
                Some("192.0.2.8:8080".parse().unwrap())
            );
            assert!(!peer.is_tls());
            assert_eq!(ctx.hash_key.as_deref(), expected_hash);
            assert_eq!(ctx.request_info.remote_ip, "2001:db8::42");
            assert_eq!(ctx.request_info.remote_ip.as_ptr(), remote_ip_ptr);
            let upstream = ctx.get_current_upstream().unwrap();
            assert_eq!(serde_json::to_value(&upstream.lb_policy).unwrap(), expected);
            assert_eq!(upstream.service_key.as_deref(), Some("default/backend"));
            assert!(Arc::ptr_eq(upstream.lb_backend.as_ref().unwrap(), &backend));
            assert_eq!(upstream.lb_backend_addr.as_ref(), Some(&backend.addr));
            assert!(upstream.tls.is_none());
            if let Some(original_ptr) = original_ptr {
                let Some(ParsedLBPolicy::ConsistentHash(
                    ConsistentHashOn::Header(retained)
                    | ConsistentHashOn::Cookie(retained)
                    | ConsistentHashOn::Arg(retained),
                )) = &upstream.lb_policy
                else {
                    panic!("named policy must remain present");
                };
                transferred.push(original_ptr == retained.as_ptr());
            }
        }
    }
    assert!(
        transferred.into_iter().all(|same| same),
        "named policy buffers must transfer without cloning"
    );
}

#[test]
fn ordinary_capture_and_selection_add_no_hot_allocations() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(&coordinator, service(None, None, None, 80), Some(true));
    let selected = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let target = selected.logical().service_target().unwrap();
    let warm = capture(&coordinator, &selected);
    warm.select(&None, ServiceDegradePolicy::Live, None, "", None).unwrap();
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        for _ in 0..16 {
            let captured = CapturedServiceSelection::capture(&coordinator, target).unwrap();
            assert_eq!(native_service_path(&captured).unwrap(), ExternalNamePath::EndpointSlice);
            std::hint::black_box(
                captured
                    .select(&None, ServiceDegradePolicy::Live, None, "", None)
                    .unwrap(),
            );
        }
    });
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn resolver_precedence_uses_captured_service_then_gateway_then_egc_and_system() {
    let mut ctx = context("default/backend");
    ctx.gateway_info = Some(Arc::new(GatewayInfo::new(
        None,
        "gateway".into(),
        None,
        None,
        None,
        None,
        Vec::new(),
        Some(ResolverCandidate::Servers(vec!["192.0.2.2".into()])),
    )));
    let egc = ResolverCandidate::Servers(vec!["192.0.2.3".into()]);
    let annotations = BTreeMap::from([(edgion_annotations::DNS_RESOLVER.to_owned(), "192.0.2.1".to_owned())]);
    let lookups = Cell::new(0);
    let absent = |_: &LinkSysKey| {
        lookups.set(lookups.get() + 1);
        None
    };
    let expected = |ip: &str| {
        ResolverSpec::StandardDns {
            servers: vec![ip.to_owned()],
        }
        .fingerprint()
        .into_owned()
    };
    assert_eq!(
        compute_external_name_resolver_with(dns_candidate(&annotations).as_ref(), &ctx, Some(&egc), absent)
            .fingerprint(),
        expected("192.0.2.1")
    );
    assert_eq!(
        compute_external_name_resolver_with(None, &ctx, Some(&egc), absent).fingerprint(),
        expected("192.0.2.2")
    );
    ctx.gateway_info = None;
    assert_eq!(
        compute_external_name_resolver_with(None, &ctx, Some(&egc), absent).fingerprint(),
        expected("192.0.2.3")
    );
    assert!(matches!(
        compute_external_name_resolver_with(None, &ctx, None, absent),
        ResolverSpec::System
    ));
    assert_eq!(lookups.get(), 0);
    let dangling = BTreeMap::from([(
        edgion_annotations::DNS_RESOLVER_REF.to_owned(),
        "dns/missing".to_owned(),
    )]);
    let spec = compute_external_name_resolver_with(dns_candidate(&dangling).as_ref(), &ctx, Some(&egc), |key| {
        assert_eq!(key.namespace, "dns");
        assert_eq!(key.name, "missing");
        lookups.set(lookups.get() + 1);
        None
    });
    assert!(matches!(spec, ResolverSpec::System));
    assert_eq!(lookups.get(), 1);
}

#[tokio::test]
async fn prepared_httpdns_failure_and_missing_hostname_do_not_issue_network_queries() {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let config=serde_json::from_value(json!({"urlTemplate":"http://unused.invalid/d?host={domain}","response":{"kind":"json","ipPath":"ips"},"fallback":{"type":"none"},"connection":{"timeoutMs":0}})).unwrap();
    let mut owner = edgion_resources::LinkSys::new(
        "resolver",
        LinkSysSpec {
            config: SystemConfig::HttpDns(config),
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("dns".into());
    let prepared = edgion_common::dns::HttpDnsCandidate::from_link_sys(Arc::new(owner))
        .unwrap()
        .prepare();
    assert!(!prepared.is_ready());
    let annotations = BTreeMap::from([(
        edgion_annotations::DNS_RESOLVER_REF.to_owned(),
        "dns/resolver".to_owned(),
    )]);
    let mut ctx = context("default/backend");
    let spec = compute_external_name_resolver_with(dns_candidate(&annotations).as_ref(), &ctx, None, |key| {
        assert_eq!(key.name, "resolver");
        Some(Arc::clone(&prepared))
    });
    assert!(matches!(&spec,ResolverSpec::HttpDns(value) if Arc::ptr_eq(value,&prepared)));
    let coordinator = ServiceDiscoveryCoordinator::new();
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let cache = DnsCache::with_capacity(4, 1);
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("unused.invalid"), Some("per-request"), 80),
        None,
    );
    let captured = capture(&coordinator, &chosen);
    let calls = Cell::new(0);
    let result = build_external_name_peer_with_resolver(
        &mut ctx,
        &input(&chosen),
        &captured,
        None,
        ExternalNameResolution {
            spec: &spec,
            dns_cache: &cache,
            cache: false,
            ttl: Duration::ZERO,
        },
        |spec, name| {
            calls.set(calls.get() + 1);
            resolve(spec, name)
        },
    )
    .await;
    assert_eq!(result.unwrap_err(), EdgionErrorCode::BackendDnsResolutionFailed);
    assert_eq!(calls.get(), 1);
    for hostname in [None, Some("")] {
        publish(
            &coordinator,
            service(Some("ExternalName"), hostname, Some("gateway"), 80),
            None,
        );
        let captured = capture(&coordinator, &chosen);
        let result = build_external_name_peer_with_resolver(
            &mut ctx,
            &input(&chosen),
            &captured,
            None,
            ExternalNameResolution {
                spec: &spec,
                dns_cache: &cache,
                cache: true,
                ttl: Duration::ZERO,
            },
            |_, _| {
                calls.set(calls.get() + 1);
                std::future::ready(Err(DnsError::NoAddresses))
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), EdgionErrorCode::BackendExternalNameNotFound);
        assert_eq!(calls.get(), 1, "missing hostname must fail before resolver callback");
    }
}

#[tokio::test]
async fn actual_dns_peer_stage_preserves_ssrf_tls_app_protocol_and_captured_absence() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut source = (*service(Some("ExternalName"), Some("answers.test"), Some("per-request"), 443)).clone();
    source.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some("kubernetes.io/h2c".into());
    publish(&coordinator, Arc::new(source), None);
    let policy=Arc::new(serde_json::from_value::<edgion_resources::BackendTLSPolicy>(json!({
        "metadata":{"name":"tls","namespace":"default"},
        "spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],"validation":{"hostname":"tls.example.test"},"useSystemCa":true}
    })).unwrap());
    let tls = BackendTlsPolicyLookup::effective_from_policy(policy);
    let chosen = selected(None, 443, tls);
    let absent = selected(None, 443, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let cache = DnsCache::with_capacity(4, 1);
    let spec = ResolverSpec::System;
    let mut ctx = context("default/backend");
    for (ips, expected) in [
        (
            vec!["::ffff:127.0.0.1"],
            Some(EdgionErrorCode::BackendLocalhostNotAllowed),
        ),
        (
            vec!["::ffff:169.254.169.254"],
            Some(EdgionErrorCode::BackendAddrLinkLocalNotAllowed),
        ),
        (vec!["::ffff:169.254.169.254", "192.0.2.45"], None),
    ] {
        let calls = Cell::new(0);
        let result = build_external_name_peer_with_resolver(
            &mut ctx,
            &input(&chosen),
            &captured,
            None,
            ExternalNameResolution {
                spec: &spec,
                dns_cache: &cache,
                cache: false,
                ttl: Duration::ZERO,
            },
            |_, _| {
                calls.set(calls.get() + 1);
                std::future::ready(Ok(ips.iter().map(|ip| ip.parse().unwrap()).collect()))
            },
        )
        .await;
        assert_eq!(calls.get(), 1);
        if let Some(error) = expected {
            assert_eq!(result.unwrap_err(), error);
        } else {
            let peer = result.unwrap();
            assert!(peer.is_tls());
            assert_eq!(peer.sni, "tls.example.test");
            assert_eq!(peer._address.as_inet().unwrap().to_string(), "192.0.2.45:443");
        }
    }
    let peer = build_external_name_peer_with_resolver(
        &mut ctx,
        &input(&absent),
        &captured,
        None,
        ExternalNameResolution {
            spec: &spec,
            dns_cache: &cache,
            cache: false,
            ttl: Duration::ZERO,
        },
        |_, _| std::future::ready(Ok(vec!["192.0.2.46".parse().unwrap()])),
    )
    .await
    .unwrap();
    assert!(!peer.is_tls());
    assert_eq!(peer.options.alpn, pingora_core::protocols::tls::ALPN::H2);
    let upstream = ctx.get_current_upstream_mut().unwrap();
    assert!(upstream.tls.is_none());
}

#[test]
fn mismatched_runtime_membership_fails_capture_and_next_attempt_recovers() {
    use crate::backends::discovery::ServiceRuntimeSnapshot;
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("192.0.2.10"), Some("gateway"), 80),
        None,
    );
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let generation = coordinator.load_generation();
    let available = generation.available("default/backend").unwrap();
    let old = available.runtime_cell.active.load_full();
    available.runtime_cell.active.store(Arc::new(ServiceRuntimeSnapshot {
        membership_identity: None,
        entries: Arc::clone(&old.entries),
        health_policy_present: old.health_policy_present,
        outlier_policy_present: old.outlier_policy_present,
        degrade_threshold: old.degrade_threshold,
        degrade_selections: old.degrade_selections.clone(),
    }));
    assert!(matches!(
        CapturedServiceSelection::capture(&coordinator, chosen.logical().service_target().unwrap()),
        Err(EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault)
    ));
    available.runtime_cell.active.store(old);
    assert_eq!(external_name(&capture(&coordinator, &chosen)).unwrap(), "192.0.2.10");
}

fn dns_candidate(annotations: &BTreeMap<String, String>) -> Option<ResolverCandidate> {
    let mut source = (*service(Some("ExternalName"), Some("example.test"), Some("per-request"), 80)).clone();
    source.metadata.annotations = Some(annotations.clone());
    edgion_common::dns::resolver_select::candidate_from_dns_owner(
        edgion_resources::resources::refs::dns_fields::DnsResolverOwner::Service(&source),
    )
}

#[test]
fn cold_captured_reference_selection_does_not_parse_or_clone_unselected_layers() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut source = (*service(Some("ExternalName"), Some("example.test"), Some("per-request"), 80)).clone();
    source
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(edgion_annotations::DNS_RESOLVER_REF.into(), " dns / resolver ".into());
    publish(&coordinator, Arc::new(source), None);
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let mut ctx = context("default/backend");
    ctx.gateway_info = Some(Arc::new(GatewayInfo::new(
        None,
        "gateway".into(),
        None,
        None,
        None,
        None,
        Vec::new(),
        Some(ResolverCandidate::Servers(vec!["192.0.2.2".into()])),
    )));
    let egc = ResolverCandidate::Servers(vec!["192.0.2.3".into()]);
    let iterations = Cell::new(0usize);
    let lookup = |_: &LinkSysKey| {
        iterations.set(iterations.get() + 1);
        None
    };
    assert!(matches!(
        compute_external_name_resolver_with(captured.dns_resolver_candidate(), &ctx, Some(&egc), lookup),
        ResolverSpec::System
    ));
    iterations.set(0);
    let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        for _ in 0..32 {
            std::hint::black_box(compute_external_name_resolver_with(
                captured.dns_resolver_candidate(),
                &ctx,
                Some(&egc),
                lookup,
            ));
        }
    });
    assert_eq!(iterations.get(), 32);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[tokio::test]
async fn cold_service_candidate_observes_live_httpdns_replacement_and_removal() {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let _guard = crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK.lock();
    let store = crate::link_sys::runtime::store::get_global_link_sys_store();
    let key = "r6b-dns/resolver".to_owned();
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut source = (*service(Some("ExternalName"), Some("example.test"), Some("per-request"), 80)).clone();
    source
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(edgion_annotations::DNS_RESOLVER_REF.into(), key.clone());
    publish(&coordinator, Arc::new(source), None);
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let ctx = context("default/backend");
    store.update(HashMap::new(), &std::collections::HashSet::from([key.clone()]));
    assert!(matches!(
        compute_external_name_resolver(captured.dns_resolver_candidate(), &ctx, None),
        ResolverSpec::System
    ));
    let mut previous = None;
    for timeout in [100, 0, 100] {
        let config=serde_json::from_value(json!({"urlTemplate":"http://unused.invalid/d?host={domain}","response":{"kind":"json","ipPath":"ips"},"fallback":{"type":"none"},"connection":{"timeoutMs":timeout}})).unwrap();
        let mut owner = edgion_resources::LinkSys::new(
            "resolver",
            LinkSysSpec {
                config: SystemConfig::HttpDns(config),
                current_status: None,
                unknown_fields: vec![],
            },
        );
        owner.metadata.namespace = Some("r6b-dns".into());
        store.update(HashMap::from([(key.clone(), owner)]), &std::collections::HashSet::new());
        let ResolverSpec::HttpDns(prepared) =
            compute_external_name_resolver(captured.dns_resolver_candidate(), &ctx, None)
        else {
            panic!("live provider")
        };
        assert_eq!(prepared.is_ready(), timeout != 0);
        if let Some(old) = previous {
            assert!(!Arc::ptr_eq(&old, &prepared));
        }
        previous = Some(prepared);
    }
    store.update(HashMap::new(), &std::collections::HashSet::from([key]));
    assert!(matches!(
        compute_external_name_resolver(captured.dns_resolver_candidate(), &ctx, None),
        ResolverSpec::System
    ));
    assert!(
        previous.unwrap().is_ready(),
        "already borrowed generation completes normally"
    );
}

#[tokio::test]
async fn ordinary_selection_borrows_request_inputs_after_sticky_check() {
    use edgion_resources::resources::lb_policy::ConsistentHashOn;

    let coordinator = ServiceDiscoveryCoordinator::new();
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    publish(&coordinator, service(None, None, None, 80), Some(true));
    let captured = capture(&coordinator, &chosen);
    let session = session().await;
    let policy = Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp));
    for has_persistence in [false, true] {
        for remote_ip in ["192.0.2.42", "2001:db8::42", ""] {
            let mut ctx = context("default/backend");
            ctx.request_info.remote_ip = remote_ip.to_owned();
            if has_persistence {
                ctx.ensure_session_persistence();
            }
            let original = ctx.request_info.remote_ip.as_ptr();
            let calls = Cell::new(0);
            let endpoint = take_sp_target_or_select(&mut ctx, &captured, |ctx| {
                calls.set(calls.get() + 1);
                let borrowed = ctx.request_info.remote_ip.as_str();
                assert_eq!(borrowed.as_ptr(), original);
                assert_eq!(
                    selector::extract_hash_key(&session, &policy, borrowed, ctx.query(session.req_header())),
                    remote_ip.as_bytes()
                );
                captured.select(
                    &policy,
                    ServiceDegradePolicy::CapturedOverride(None),
                    Some(&session),
                    borrowed,
                    ctx.query(session.req_header()),
                )
            })
            .unwrap();
            assert_eq!(calls.get(), 1);
            assert_eq!(endpoint.backend.addr.as_inet().unwrap().to_string(), "192.0.2.8:8080");
            assert_eq!(ctx.session_persistence().is_some(), has_persistence);
            if let Some(persistence) = ctx.session_persistence() {
                assert!(persistence.target_addr.is_none());
                assert!(!persistence.sticky_consumed);
            }
            assert_eq!(ctx.request_info.remote_ip.as_ptr(), original);
        }
    }
}
