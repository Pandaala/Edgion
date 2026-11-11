use super::*;

#[test]
fn dns_family_precedence_is_independent_and_invalid_overrides_fail_closed() {
    let mut ctx = EdgionHttpContext::default();
    assert_eq!(
        compute_lookup_family(None, &ctx, None).unwrap(),
        DnsLookupFamily::Ipv4Only
    );
    let gateway = GatewayInfo::new(
        None,
        "family".into(),
        None,
        None,
        None,
        None,
        vec![],
        None,
        Some(Ok(DnsLookupFamily::Ipv6Only)),
    );
    ctx.gateway_info = Some(Arc::new(gateway));
    assert_eq!(
        compute_lookup_family(None, &ctx, Some(DnsLookupFamily::All)).unwrap(),
        DnsLookupFamily::Ipv6Only
    );
    assert_eq!(
        compute_lookup_family(Some(Ok(DnsLookupFamily::Ipv4Only)), &ctx, Some(DnsLookupFamily::All)).unwrap(),
        DnsLookupFamily::Ipv4Only
    );
    assert!(compute_lookup_family(Some(Err("invalid DNS family")), &ctx, None).is_err());
    ctx.gateway_info = Some(Arc::new(GatewayInfo::new(
        None,
        "family".into(),
        None,
        None,
        None,
        None,
        vec![],
        None,
        Some(Err("invalid DNS family")),
    )));
    assert!(compute_lookup_family(None, &ctx, Some(DnsLookupFamily::All)).is_err());
    assert_eq!(
        compute_lookup_family(Some(Ok(DnsLookupFamily::All)), &ctx, None).unwrap(),
        DnsLookupFamily::All
    );
}

#[test]
fn external_name_family_precedence_has_no_gateway_layer() {
    assert_eq!(external_name_family(None, None).unwrap(), DnsLookupFamily::Ipv4Only);
    assert_eq!(
        external_name_family(None, Some(DnsLookupFamily::All)).unwrap(),
        DnsLookupFamily::All
    );
    assert_eq!(
        external_name_family(Some(Ok(DnsLookupFamily::Ipv6Only)), Some(DnsLookupFamily::All)).unwrap(),
        DnsLookupFamily::Ipv6Only
    );
    assert!(external_name_family(Some(Err("invalid DNS family")), Some(DnsLookupFamily::All)).is_err());
}

#[test]
fn external_name_source_follows_annotation_then_global_switch() {
    use edgion_resources::resources::common::DnsResolutionMode as Mode;
    for (annotation, endpoint_table, expected) in [
        (Ok(None), true, Ok(ExternalNameSource::Endpoints)),
        (Ok(None), false, Ok(ExternalNameSource::System)),
        (Ok(Some(Mode::Gateway)), false, Ok(ExternalNameSource::Endpoints)),
        (Ok(Some(Mode::Controller)), false, Ok(ExternalNameSource::Endpoints)),
        (Ok(Some(Mode::System)), true, Ok(ExternalNameSource::System)),
        (Err("bad"), true, Err(EdgionErrorCode::BackendDnsResolutionFailed)),
    ] {
        assert_eq!(external_name_source(annotation, endpoint_table), expected);
    }
}

use crate::backends::discovery::ServiceDiscoveryCoordinator;
use crate::routes::grpc::{GrpcBackendHandle, GrpcRouteRuleUnit, SelectedGrpcBackend};
use crate::routes::http::match_unit::HttpRouteRuleUnit;
use edgion_resources::{GRPCRoute, HTTPRoute};
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use tokio::io::AsyncWriteExt;

/// Boxed answer future returned by [`vec_resolver`].
type AnswerFuture<'a> = std::pin::Pin<Box<dyn Future<Output = Result<DnsAnswer, DnsError>> + 'a>>;

/// Adapt a test resolver that returns plain addresses to the answer-returning
/// resolver seam (no record TTL, so the fallback TTL applies).
fn vec_resolver<'a, F, Fut>(resolver: F) -> impl FnOnce(&'a ResolverSpec, &'a str, DnsQueryPolicy) -> AnswerFuture<'a>
where
    F: FnOnce(&'a ResolverSpec, &'a str, DnsQueryPolicy) -> Fut + 'a,
    Fut: Future<Output = Result<Vec<IpAddr>, DnsError>> + 'a,
{
    move |spec, name, policy| {
        let lookup = resolver(spec, name, policy);
        Box::pin(async move { lookup.await.map(DnsAnswer::without_ttl) })
    }
}

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
            service(kind, Some("192.0.2.44"), Some("system"), 443),
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
            Some(service(Some("ExternalName"), Some("unused.test"), Some("system"), 81)),
            EdgionErrorCode::BackendPortResolutionFailed,
        ),
        (
            Some(service(Some("ExternalName"), None, Some("system"), 80)),
            EdgionErrorCode::BackendExternalNameNotFound,
        ),
        (
            Some(service(Some("ExternalName"), Some(""), Some("system"), 80)),
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
async fn modes_never_fall_back_and_controller_readiness_keeps_runtime_health() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let selected = selected(None, 80, BackendTlsPolicyLookup::Absent);
    // `controller` reads conf_sync slices only; `system` resolves per attempt;
    // `gateway` (the default) reads Gateway-local slices only, which this
    // private coordinator does not have, and never falls back to DNS.
    for (mode, ready, expected) in [
        (Some("controller"), true, Ok("192.0.2.8:8080")),
        (Some("controller"), false, Err(())),
        (Some("system"), true, Ok("192.0.2.40:80")),
        (None, true, Err(())),
        (Some("gateway"), true, Err(())),
    ] {
        publish(
            &coordinator,
            service(Some("ExternalName"), Some("192.0.2.40"), mode, 80),
            Some(ready),
        );
        let mut ctx = context("default/backend");
        let choice = SelectedLogicalBackend::Service(selected.clone());
        let peer = try_get_peer_with_capture(&mut ctx, &session().await, false, Some(&choice), |target| {
            CapturedServiceSelection::capture(&coordinator, target)
        })
        .await;
        match expected {
            Ok(address) => assert_eq!(
                peer.unwrap()._address.as_inet().unwrap().to_string(),
                address,
                "{mode:?}"
            ),
            Err(()) => assert!(peer.is_err(), "{mode:?} must not fall back"),
        }
    }
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("192.0.2.40"), Some("controller"), 80),
        Some(true),
    );
    let captured = capture(&coordinator, &selected);
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
    assert_eq!(native_service_path(&captured).unwrap(), ExternalNameSource::Endpoints);
    assert!(captured
        .select(&None, ServiceDegradePolicy::CapturedOverride(Some(0)), None, "", None)
        .is_err());
}

#[test]
fn dns_mode_only_updates_preserve_captured_paths() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let selected = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let mut retained = Vec::new();
    let failed = Err(EdgionErrorCode::BackendDnsResolutionFailed);
    for (mode, expected) in [
        (None, Ok(ExternalNameSource::Endpoints)),
        (Some("gateway"), Ok(ExternalNameSource::Endpoints)),
        (Some("system"), Ok(ExternalNameSource::System)),
        (Some(" \tgateway \t"), failed),
        (Some("controller"), Ok(ExternalNameSource::Endpoints)),
        (Some("invalid"), failed),
        (Some("per-request"), failed),
        (None, Ok(ExternalNameSource::Endpoints)),
    ] {
        publish(
            &coordinator,
            service(Some("ExternalName"), Some("answers.test"), mode, 80),
            Some(true),
        );
        let current = capture(&coordinator, &selected);
        assert_eq!(native_service_path(&current), expected, "{mode:?}");
        retained.push((current, expected));
        for (previous, original_path) in &retained {
            assert_eq!(native_service_path(previous), *original_path);
        }
    }
}

#[tokio::test]
async fn actual_dns_stage_keeps_old_service_geometry_across_await_and_next_capture_recovers() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut old = (*service(Some("ExternalName"), Some("old.example.test"), Some("system"), 80)).clone();
    old.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some("kubernetes.io/h2c".into());
    publish(&coordinator, Arc::new(old), None);
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let mut ctx = context("default/backend");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let peer_input = input(&chosen);
    let attempt = build_system_external_name_peer(
        &mut ctx,
        &peer_input,
        &captured,
        None,
        DnsLookupFamily::All,
        vec_resolver(|seen_spec, hostname, _policy| async move {
            assert!(
                matches!(seen_spec, ResolverSpec::System),
                "system mode always asks the OS"
            );
            assert_eq!(hostname, "old.example.test");
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            assert_eq!(hostname, "old.example.test");
            Ok(vec!["192.0.2.10".parse().unwrap()])
        }),
    );
    let update = async {
        entered_rx.await.unwrap();
        let mut changed = (*service(Some("ExternalName"), Some("new.example.test"), Some("system"), 81)).clone();
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
    assert_eq!(new.section().identity.protocol.as_ref(), "UDP");
    assert_eq!(native_service_path(&new).unwrap(), ExternalNameSource::System);
    coordinator.replace_services(&HashMap::new());
    assert!(matches!(
        CapturedServiceSelection::capture(&coordinator, current.logical().service_target().unwrap()),
        Err(EdgionErrorCode::BackendServiceNotFound)
    ));
    assert_eq!(external_name(&captured).unwrap(), "old.example.test");
}

#[tokio::test]
async fn sticky_validation_uses_captured_section_and_self_dns_ignores_stale_target() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    publish(&coordinator, service(None, None, None, 80), Some(true));
    let old = capture(&coordinator, &chosen);
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("192.0.2.90"), Some("system"), 80),
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
    for test_type in [
        None,
        Some(TestType::None),
        Some(TestType::Retry),
        Some(TestType::Latency),
        Some(TestType::Lb),
    ] {
        for policy in [
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
                "x-routing-key".into(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie(
                "routing-cookie".into(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg(
                "routing-query".into(),
            ))),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp)),
            Some(ParsedLBPolicy::LeastConn),
            Some(ParsedLBPolicy::Ewma),
            None,
        ] {
            let expected = serde_json::to_value(&policy).unwrap();
            let expected_hash = if matches!(test_type, Some(TestType::Lb)) {
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
                test_type.clone(),
                None,
                Vec::new(),
                None,
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
            assert_eq!(ctx.test_hash_key(), expected_hash);
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
            assert_eq!(native_service_path(&captured).unwrap(), ExternalNameSource::Endpoints);
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

#[tokio::test]
async fn system_resolution_failure_and_missing_hostname_have_fixed_failures() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let mut ctx = context("default/backend");
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("unused.invalid"), Some("system"), 80),
        None,
    );
    let captured = capture(&coordinator, &chosen);
    let calls = Cell::new(0);
    let result = build_system_external_name_peer(
        &mut ctx,
        &input(&chosen),
        &captured,
        None,
        DnsLookupFamily::All,
        vec_resolver(|_, _, _policy| {
            calls.set(calls.get() + 1);
            std::future::ready(Err(DnsError::Io("refused".into())))
        }),
    )
    .await;
    assert_eq!(result.unwrap_err(), EdgionErrorCode::BackendDnsResolutionFailed);
    assert_eq!(calls.get(), 1);
    for hostname in [None, Some("")] {
        publish(
            &coordinator,
            service(Some("ExternalName"), hostname, Some("system"), 80),
            None,
        );
        let captured = capture(&coordinator, &chosen);
        let result = build_system_external_name_peer(
            &mut ctx,
            &input(&chosen),
            &captured,
            None,
            DnsLookupFamily::All,
            vec_resolver(|_, _, _policy| {
                calls.set(calls.get() + 1);
                std::future::ready(Err(DnsError::NoAddresses))
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), EdgionErrorCode::BackendExternalNameNotFound);
        assert_eq!(calls.get(), 1, "missing hostname must fail before resolver callback");
    }
}

#[tokio::test]
async fn actual_dns_peer_stage_preserves_ssrf_tls_app_protocol_and_captured_absence() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let mut source = (*service(Some("ExternalName"), Some("answers.test"), Some("system"), 443)).clone();
    source.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some("kubernetes.io/h2c".into());
    publish(&coordinator, Arc::new(source), None);
    let policy=Arc::new(serde_json::from_value::<edgion_resources::BackendTLSPolicy>(json!({
        "metadata":{"name":"tls","namespace":"default"},
        "spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],"validation":{"hostname":"tls.example.test"},"useSystemCa":true,"preparedAncestorAdmission":{"mode":"unrestricted"}}
    })).unwrap());
    let tls = BackendTlsPolicyLookup::effective_from_policy(policy);
    let chosen = selected(None, 443, tls);
    let absent = selected(None, 443, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
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
        // Unspecified answers connect to the local host: never dialed.
        (
            vec!["0.0.0.0", "::ffff:0.0.0.0"],
            Some(EdgionErrorCode::BackendDnsResolutionFailed),
        ),
        (vec!["::", "192.0.2.45"], None),
    ] {
        let calls = Cell::new(0);
        let result = build_system_external_name_peer(
            &mut ctx,
            &input(&chosen),
            &captured,
            None,
            DnsLookupFamily::All,
            vec_resolver(|_, _, _policy| {
                calls.set(calls.get() + 1);
                std::future::ready(Ok(ips.iter().map(|ip| ip.parse().unwrap()).collect()))
            }),
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
    let peer = build_system_external_name_peer(
        &mut ctx,
        &input(&absent),
        &captured,
        None,
        DnsLookupFamily::All,
        vec_resolver(|_, _, _policy| std::future::ready(Ok(vec!["192.0.2.46".parse().unwrap()]))),
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
                    selector::extract_hash_key(&session, &policy, borrowed, Some(&ctx.request_query)),
                    remote_ip.as_bytes()
                );
                captured.select(
                    &policy,
                    ServiceDegradePolicy::CapturedOverride(None),
                    Some(&session),
                    borrowed,
                    Some(&ctx.request_query),
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

#[tokio::test]
async fn system_lookups_rotate_over_the_answer() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("spread.test"), Some("system"), 80),
        None,
    );
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let answers = ["192.0.2.61", "192.0.2.62", "192.0.2.63"];
    let mut first_addresses = std::collections::BTreeSet::new();
    for _ in 0..12 {
        let mut ctx = context("default/backend");
        let peer = build_system_external_name_peer(
            &mut ctx,
            &input(&chosen),
            &captured,
            None,
            DnsLookupFamily::All,
            vec_resolver(|_, _, _policy| {
                std::future::ready(Ok(answers.iter().map(|ip| ip.parse().unwrap()).collect()))
            }),
        )
        .await
        .unwrap();
        first_addresses.insert(peer._address.as_inet().unwrap().ip().to_string());
    }
    // The process-wide rotation advances on every attempt (other tests may
    // advance it concurrently, which only adds spread).
    assert!(first_addresses.len() > 1, "system lookups rotate: {first_addresses:?}");
}

#[test]
fn address_rotation_spreads_the_first_address_and_tolerates_empty_sets() {
    let mut addrs = vec![1, 2, 3];
    rotate_addresses(&mut addrs, 4);
    assert_eq!(addrs, vec![2, 3, 1]);
    let mut empty: Vec<u8> = Vec::new();
    rotate_addresses(&mut empty, 7);
    assert!(empty.is_empty());
    let first = next_address_rotation();
    assert_ne!(next_address_rotation(), first, "each attempt advances the rotation");
}

#[tokio::test(start_paused = true)]
async fn system_lookups_spend_the_caller_budget() {
    let hung = std::future::pending::<Result<DnsAnswer, DnsError>>();
    let started = tokio::time::Instant::now();
    let result = within_budget(Some(std::time::Duration::from_secs(2)), hung).await;
    assert!(result.is_none(), "a hung lookup ends at the budget");
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(2));
    let answered = within_budget(None, std::future::ready(Ok(DnsAnswer::without_ttl(Vec::new())))).await;
    assert!(
        answered.is_some_and(|answer| answer.is_ok()),
        "no budget: the resolver's own limit applies"
    );
}

#[tokio::test(start_paused = true)]
async fn a_system_lookup_outliving_the_deadline_is_a_timeout_not_a_dns_failure() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(
        &coordinator,
        service(Some("ExternalName"), Some("slow.test"), Some("system"), 80),
        None,
    );
    let chosen = selected(None, 80, BackendTlsPolicyLookup::Absent);
    let captured = capture(&coordinator, &chosen);
    let mut ctx = context("default/backend");
    ctx.request_info
        .set_grpc_client_deadline(std::time::Instant::now() + std::time::Duration::from_millis(100));
    let result = build_system_external_name_peer(
        &mut ctx,
        &input(&chosen),
        &captured,
        None,
        DnsLookupFamily::All,
        vec_resolver(|_, _, _policy| std::future::pending::<Result<Vec<IpAddr>, DnsError>>()),
    )
    .await;
    assert_eq!(result.unwrap_err(), EdgionErrorCode::GrpcClientDeadlineExceeded);
}

#[tokio::test(start_paused = true)]
async fn a_slow_datagram_lookup_is_waited_for_once() {
    let key = || (Box::<str>::from("slow-datagram.test"), DnsLookupFamily::Ipv4Only);
    let budget = std::time::Duration::from_millis(250);
    let started = tokio::time::Instant::now();
    // A cached answer is served at the first poll.
    let cached = async { Ok(vec!["192.0.2.1".parse().unwrap()]) };
    assert_eq!(
        datagram_lookup(key(), cached, budget).await.map(|ips| ips.len()),
        Some(1)
    );
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    // A stalled lookup costs the first datagram the budget ...
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let stalled = async move {
        let _ = finished.await;
        Err(DnsError::Io("late".into()))
    };
    assert!(datagram_lookup(key(), stalled, budget).await.is_none());
    assert_eq!(started.elapsed(), budget);
    // ... and later datagrams nothing while it still runs.
    let queued = std::future::pending::<Result<Vec<IpAddr>, DnsError>>();
    assert!(datagram_lookup(key(), queued, budget).await.is_none());
    assert_eq!(started.elapsed(), budget, "dropped at once, not queued");
    // Once it completes, a new lookup may use the budget again.
    finish.send(()).unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    let queued = std::future::pending::<Result<Vec<IpAddr>, DnsError>>();
    assert!(datagram_lookup(key(), queued, budget).await.is_none());
    assert_eq!(started.elapsed(), budget * 2);
}

#[test]
fn system_retries_prefer_addresses_no_attempt_dialed() {
    let ip = |value: &str| value.parse::<IpAddr>().unwrap();
    let mut ips = vec![ip("192.0.2.1"), ip("192.0.2.2"), ip("192.0.2.3")];
    prefer_undialed(&mut ips, &[ip("192.0.2.1")]);
    assert_eq!(ips, vec![ip("192.0.2.2"), ip("192.0.2.3"), ip("192.0.2.1")]);
    let mut all = vec![ip("192.0.2.1"), ip("192.0.2.2")];
    prefer_undialed(&mut all, &[ip("192.0.2.2"), ip("192.0.2.1")]);
    assert_eq!(all, vec![ip("192.0.2.1"), ip("192.0.2.2")], "all dialed: order kept");

    let mut ctx = context("default/backend");
    ctx.get_current_upstream_mut().unwrap().ip = Some("192.0.2.1".into());
    ctx.push_upstream(None, None);
    assert_eq!(previously_dialed_ips(&ctx), vec![ip("192.0.2.1")]);
}
