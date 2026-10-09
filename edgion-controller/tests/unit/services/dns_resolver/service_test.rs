use super::*;

fn httpdns_owner(config: edgion_resources::resources::link_sys::HttpDnsResolverConfig) -> Arc<LinkSys> {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let mut owner = LinkSys::new(
        "dns",
        LinkSysSpec {
            config: SystemConfig::HttpDns(config),
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("test".into());
    owner.metadata.resource_version = Some("same-rv".into());
    Arc::new(owner)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_owner_failure_with_same_rv_publishes_fallback_reuses_failure_and_recovers() {
    let config = serde_json::from_value(serde_json::json!({"preset":"aliyun","fallback":{"type":"none"}})).unwrap();
    let original = httpdns_owner(config);
    let key = LinkSysKey {
        namespace: "test".into(),
        name: "dns".into(),
    };
    let mut state = ServiceDnsState {
        applied_fingerprint: Some("last-good-slice".into()),
        ..Default::default()
    };
    let ready = state.prepare(&key, Arc::clone(&original)).unwrap();
    assert!(ready.is_ready());
    let mut malformed = (*original).clone();
    malformed.metadata.name = None;
    let malformed = Arc::new(malformed);
    assert_eq!(original.metadata.resource_version, malformed.metadata.resource_version);
    let failed = state.prepare(&key, Arc::clone(&malformed)).unwrap();
    assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed));
    assert!(resolve(&ResolverSpec::HttpDns(Arc::clone(&failed)), "localhost")
        .await
        .is_err());
    assert!(Arc::ptr_eq(&failed, &state.prepare(&key, malformed).unwrap()));
    let repaired = state.prepare(&key, original).unwrap();
    assert!(repaired.is_ready() && !Arc::ptr_eq(&failed, &repaired));
    assert_eq!(state.applied_fingerprint.as_deref(), Some("last-good-slice"));
}

#[test]
fn active_httpdns_pair_failure_with_same_rv_cannot_reuse_ready_or_hide_missing_get_material() {
    use edgion_resources::resources::link_sys::SystemConfig;
    let config = serde_json::from_value(
        serde_json::json!({"preset":"aliyun","fallback":{"type":"none"},"connection":{"tls":{
            "enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"test"}}]
        }}}),
    )
    .unwrap();
    let original = httpdns_owner(config);
    let key = LinkSysKey {
        namespace: "test".into(),
        name: "dns".into(),
    };
    let mut state = ServiceDnsState::default();
    let ready = state.prepare(&key, Arc::clone(&original)).unwrap();
    assert!(ready.is_ready());
    for case in ["group", "material-namespace", "missing-get-material"] {
        let mut malformed = (*original).clone();
        let SystemConfig::HttpDns(config) = &mut malformed.spec.config else {
            unreachable!()
        };
        let tls = config.connection.as_mut().unwrap().tls.as_mut().unwrap();
        match case {
            "group" => {
                tls.outbound
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()[0]
                    .group = Some("other.example".into())
            }
            "material-namespace" => {
                tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
            }
            "missing-get-material" => tls.resolved_ca_certificates = None,
            _ => unreachable!(),
        }
        let malformed = Arc::new(malformed);
        assert_eq!(original.metadata.resource_version, malformed.metadata.resource_version);
        let failed = state.prepare(&key, Arc::clone(&malformed)).unwrap();
        assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed), "{case}");
        assert!(Arc::ptr_eq(&failed, &state.prepare(&key, malformed).unwrap()), "{case}");
        assert!(state.prepare(&key, Arc::clone(&original)).unwrap().is_ready(), "{case}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn service_preparation_reuses_failures_independently_of_applied_slice() {
    let config = serde_json::from_value(
        serde_json::json!({"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}),
    )
    .unwrap();
    let key = LinkSysKey {
        namespace: "test".into(),
        name: "dns".into(),
    };
    let mut state = ServiceDnsState {
        applied_fingerprint: Some("last-good-slice".into()),
        ..Default::default()
    };
    let first = state.prepare(&key, httpdns_owner(config)).unwrap();
    assert!(!first.is_ready());
    assert!(resolve(&ResolverSpec::HttpDns(first.clone()), "localhost")
        .await
        .is_err());
    let config = serde_json::from_value(
        serde_json::json!({"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}),
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &state.prepare(&key, httpdns_owner(config)).unwrap()
    ));
    assert_eq!(state.applied_fingerprint.as_deref(), Some("last-good-slice"));
    let other = LinkSysKey {
        namespace: "test".into(),
        name: "other".into(),
    };
    let config = serde_json::from_value(
        serde_json::json!({"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}),
    )
    .unwrap();
    assert!(
        !Arc::ptr_eq(&first, &state.prepare(&other, httpdns_owner(config)).unwrap()),
        "changing references retires the previous prepared owner"
    );
}
use http::{Response, StatusCode};
use kube::client::Body;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use tower::service_fn;

fn empty_kubernetes_list(api_version: &str, kind: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "apiVersion": api_version,
                "kind": kind,
                "metadata": {"resourceVersion": "1"},
                "items": []
            }))
            .expect("response JSON must serialize"),
        ))
        .expect("response must build")
}

#[tokio::test]
async fn reconcile_lists_only_configured_namespaces() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&paths);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let path = request.uri().path().to_string();
            captured.lock().push(path.clone());
            let response = if path.contains("endpointslices") {
                empty_kubernetes_list("discovery.k8s.io/v1", "EndpointSliceList")
            } else {
                empty_kubernetes_list("v1", "ServiceList")
            };
            Ok::<_, Infallible>(response)
        }
    });

    let client = Client::new(service, "default");
    reconcile_once(
        &client,
        &["team-a".into(), "team-b".into()],
        &DnsResolverConfig::default(),
        &mut HashMap::new(),
    )
    .await
    .expect("namespace-scoped reconcile must succeed");

    assert_eq!(
        *paths.lock(),
        vec![
            "/apis/discovery.k8s.io/v1/namespaces/team-a/endpointslices",
            "/apis/discovery.k8s.io/v1/namespaces/team-b/endpointslices",
            "/api/v1/namespaces/team-a/services",
            "/api/v1/namespaces/team-b/services"
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn controller_ticks_reuse_failed_preparation_and_clear_changed_or_removed_services() {
    let mode = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_mode = mode.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let mode = observed_mode.load(std::sync::atomic::Ordering::Relaxed);
        async move {
            let path = request.uri().path();
            let payload = if path.contains("endpointslices") {
                json!({"apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSliceList", "metadata": {}, "items": []})
            } else if path.contains("linksys") {
                json!({"apiVersion": "edgion.io/v1", "kind": "LinkSys", "metadata": {"namespace": "test", "name": "dns"},
                    "spec": {"type": "httpdns", "config": {"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}}})
            } else {
                let items = if mode == 2 {
                    vec![]
                } else {
                    vec![json!({"apiVersion": "v1", "kind": "Service", "metadata": {
                        "namespace": "test", "name": "svc", "uid": "service-uid",
                        "annotations": {"edgion.io/dns-resolution-mode": "controller", "edgion.io/dns-resolver-ref": if mode == 0 { "test/dns" } else { "" }}
                        }, "spec": {"type": "ExternalName", "externalName": "1.2.3.4", "ports": [{"port": 80}]}})]
                };
                json!({"apiVersion": "v1", "kind": "ServiceList", "metadata": {}, "items": items})
            };
            Ok::<_, Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
        }
    });
    let client = Client::new(service, "test");
    let config = DnsResolverConfig {
        enabled: true,
        ..Default::default()
    };
    let mut states = HashMap::new();
    reconcile_once(&client, &[], &config, &mut states).await.unwrap();
    let first = states["test/svc"].prepared.as_ref().unwrap().1.clone();
    assert!(!first.is_ready());
    states.get_mut("test/svc").unwrap().applied_fingerprint = Some("last-good".into());
    reconcile_once(&client, &[], &config, &mut states).await.unwrap();
    assert!(Arc::ptr_eq(&first, &states["test/svc"].prepared.as_ref().unwrap().1));
    assert_eq!(states["test/svc"].applied_fingerprint.as_deref(), Some("last-good"));
    // An empty resolver-ref selects System without touching LinkSys; the IP
    // literal takes resolve_system's pure fast path without any OS DNS lookup.
    mode.store(1, std::sync::atomic::Ordering::Relaxed);
    reconcile_once(&client, &[], &config, &mut states).await.unwrap();
    assert!(states["test/svc"].prepared.is_none());
    mode.store(2, std::sync::atomic::Ordering::Relaxed);
    reconcile_once(&client, &[], &config, &mut states).await.unwrap();
    assert!(
        states.is_empty(),
        "removed Service must release the prepared generation and slice state"
    );
}

#[test]
fn kube_less_default_resolver_falls_back_to_system_without_inline_servers() {
    assert!(
        matches!(controller_default_resolver_without_kube(), ResolverSpec::System),
        "without kube, a missing/httpdns default candidate must fail safe to the system resolver"
    );
}

#[test]
fn parse_refresh_interval_units() {
    assert_eq!(parse_duration_str("15s"), Some(Duration::from_secs(15)));
    assert_eq!(parse_duration_str("500ms"), Some(Duration::from_millis(500)));
    assert_eq!(parse_duration_str("2m"), Some(Duration::from_secs(120)));
    assert_eq!(parse_duration_str("1h"), Some(Duration::from_secs(3600)));
    assert_eq!(parse_duration_str("1h30m"), Some(Duration::from_secs(5400)));
    // Bare numbers are no longer valid GEP-2257 durations.
    assert_eq!(parse_duration_str("30"), None);
    assert_eq!(parse_duration_str("bogus"), None);
}

#[test]
fn parse_refresh_interval_defaults_and_clamps() {
    assert_eq!(parse_refresh_interval("bogus"), DEFAULT_REFRESH); // unparseable → default
    assert_eq!(parse_refresh_interval("0s"), MIN_REFRESH); // zero clamped up
    assert_eq!(parse_refresh_interval("500ms"), MIN_REFRESH); // below min clamped up
    assert_eq!(parse_refresh_interval("15s"), Duration::from_secs(15));
}

#[test]
fn can_skip_apply_requires_existence_and_matching_fingerprint() {
    let key = "app/ext".to_string();
    let fp = "10.0.0.1|80".to_string();
    let mut last_applied = HashMap::new();
    last_applied.insert(
        key.clone(),
        ServiceDnsState {
            applied_fingerprint: Some(fp.clone()),
            ..Default::default()
        },
    );

    let mut observed = HashSet::new();
    observed.insert(key.clone());

    // Present + fingerprint matches → skip.
    assert!(can_skip_apply(&observed, &last_applied, &key, &fp));
    // Slice deleted out-of-band (not observed) → must NOT skip, even though the
    // fingerprint still matches (this is the regression guard).
    assert!(!can_skip_apply(&HashSet::new(), &last_applied, &key, &fp));
    // Present but fingerprint changed → must NOT skip.
    assert!(!can_skip_apply(&observed, &last_applied, &key, "10.0.0.2|80"));
    // Never applied before → must NOT skip.
    assert!(!can_skip_apply(&observed, &HashMap::new(), &key, &fp));
}

#[test]
fn slice_fingerprint_stable_and_discriminating() {
    let a: Vec<IpAddr> = vec![IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2])];
    let ports = |nums: &[i32]| {
        nums.iter()
            .map(|&p| ServicePort {
                port: p,
                ..Default::default()
            })
            .collect::<Vec<_>>()
    };
    let fp = slice_fingerprint(&a, &ports(&[80]));
    // Same inputs → same fingerprint (no spurious re-apply).
    assert_eq!(fp, slice_fingerprint(&a, &ports(&[80])));
    // A changed IP set or port set → different fingerprint (re-apply).
    assert_ne!(fp, slice_fingerprint(&[IpAddr::from([10, 0, 0, 1])], &ports(&[80])));
    assert_ne!(fp, slice_fingerprint(&a, &ports(&[80, 443])));
}

#[test]
fn controller_default_candidate_prefers_ref_then_servers() {
    let ref_cfg = DnsResolverConfig {
        enabled: true,
        refresh_interval: "15s".to_string(),
        link_sys_ref: Some("dns/aliyun".to_string()),
        servers: Vec::new(),
    };
    assert_eq!(
        controller_default_candidate(&ref_cfg),
        Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".to_string(),
            name: "aliyun".to_string(),
        }))
    );

    let servers_cfg = DnsResolverConfig {
        enabled: true,
        refresh_interval: "15s".to_string(),
        link_sys_ref: None,
        servers: vec!["1.1.1.1".to_string()],
    };
    assert_eq!(
        controller_default_candidate(&servers_cfg),
        Some(ResolverCandidate::Servers(vec!["1.1.1.1".to_string()]))
    );

    assert_eq!(controller_default_candidate(&DnsResolverConfig::default()), None);
}

// Original tests retain their call shape; production always receives its runtime.
async fn reconcile_once(
    client: &Client,
    namespaces: &[String],
    config: &DnsResolverConfig,
    states: &mut HashMap<String, ServiceDnsState>,
) -> Result<()> {
    super::reconcile_with_runtime(client, namespaces, config, states, &ControllerRuntime::default()).await
}

fn can_skip_apply(
    observed: &HashSet<String>,
    states: &HashMap<String, ServiceDnsState>,
    key: &str,
    fingerprint: &str,
) -> bool {
    let (namespace, name) = key.split_once('/').unwrap();
    let service = Service {
        metadata: ObjectMeta {
            namespace: Some(namespace.into()),
            name: Some(name.into()),
            uid: Some("fixture-uid".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let ips = ["10.0.0.1".parse().unwrap()];
    let slices = if observed.contains(key) {
        build_endpoint_slices(
            name,
            namespace,
            service_owner_ref(&service).unwrap(),
            &[ServicePort {
                port: 80,
                ..Default::default()
            }],
            &ips,
            "fixture",
        )
    } else {
        vec![]
    };
    let records = slices
        .iter()
        .map(|slice| {
            (
                (
                    slice.metadata.namespace.as_deref().unwrap(),
                    slice.metadata.name.as_deref().unwrap(),
                ),
                slice,
            )
        })
        .collect();
    super::can_skip_owned_slices(&records, states, key, fingerprint, &service, &ips)
}

fn dns_service(reference: &str) -> Service {
    serde_json::from_value(json!({"metadata":{"namespace":"app","name":"source","uid":"current-uid","annotations":{"edgion.io/dns-resolver-ref":reference}},"spec":{"type":"ExternalName","externalName":"192.0.2.1","ports":[{"port":80}]}})).unwrap()
}
fn dns_grant(runtime: &ControllerRuntime, kind: &str) {
    let group = if kind == "Service" {
        ""
    } else {
        "gateway.networking.k8s.io"
    };
    runtime.ref_grant_store.upsert("dns/allow".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"allow","namespace":"dns"},"spec":{"from":[{"group":group,"kind":kind,"namespace":"app"}],"to":[{"group":"edgion.io","kind":"LinkSys","name":"resolver"}]}})).unwrap()));
}

#[tokio::test]
async fn raw_dns_producer_gates_tenant_before_get_and_preserves_default_authority() {
    let source = Arc::new(Mutex::new(dns_service("dns/resolver")));
    let gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = source.clone();
    let count = gets.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            let source = captured.lock().clone();
            let count = count.clone();
            async move {
                let path = request.uri().path();
                let payload = if path.contains("linksys") {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    json!({"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"name":"resolver","namespace":"dns"},"spec":{"type":"httpdns","config":{"preset":"aliyun","connection":{"timeoutMs":0},"fallback":{"type":"none"}}}})
                } else if path.contains("endpointslices") {
                    if request.method() == http::Method::PATCH {
                        serde_json::from_slice(&request.into_body().collect_bytes().await.unwrap()).unwrap()
                    } else {
                        json!({"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","metadata":{},"items":[]})
                    }
                } else {
                    json!({"apiVersion":"v1","kind":"ServiceList","metadata":{},"items":[source]})
                };
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                        .unwrap(),
                )
            }
        }),
        "app",
    );
    let runtime = ControllerRuntime::default();
    let mut states = HashMap::new();
    let config = DnsResolverConfig {
        enabled: true,
        ..Default::default()
    };
    for kind in [None, Some("Gateway"), Some("Service"), None, Some("Service")] {
        runtime.ref_grant_store.remove("dns/allow");
        if let Some(kind) = kind {
            dns_grant(&runtime, kind);
        }
        let before = gets.load(std::sync::atomic::Ordering::SeqCst);
        reconcile_with_runtime(&client, &[], &config, &mut states, &runtime)
            .await
            .unwrap();
        assert_eq!(
            gets.load(std::sync::atomic::Ordering::SeqCst) - before,
            usize::from(kind == Some("Service"))
        );
        assert_eq!(states["app/source"].prepared.is_some(), kind == Some("Service"));
    }
    runtime.ref_grant_store.remove("dns/allow");
    let config = DnsResolverConfig {
        link_sys_ref: Some("dns/resolver".into()),
        ..config
    };
    let before = gets.load(std::sync::atomic::Ordering::SeqCst);
    reconcile_with_runtime(&client, &[], &config, &mut states, &runtime)
        .await
        .unwrap();
    assert_eq!(
        gets.load(std::sync::atomic::Ordering::SeqCst),
        before + 1,
        "administrative default independently admits the same key"
    );
    assert!(states["app/source"].prepared.is_some());
    for reference in ["bare", "/resolver", "dns/", "dns/a/b"] {
        source
            .lock()
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("edgion.io/dns-resolver-ref".into(), reference.into());
        let before = gets.load(std::sync::atomic::Ordering::SeqCst);
        reconcile_with_runtime(
            &client,
            &[],
            &DnsResolverConfig {
                enabled: true,
                ..Default::default()
            },
            &mut states,
            &runtime,
        )
        .await
        .unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), before);
        assert!(states["app/source"].prepared.is_none());
    }
}

#[tokio::test]
async fn actual_slice_apply_repairs_uid_and_each_required_family_and_retries_failure() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let captured = requests.clone();
    let reject = fail.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            let captured = captured.clone();
            let reject = reject.clone();
            async move {
                assert_eq!(request.method(), http::Method::PATCH);
                assert!(request.uri().query().unwrap().contains("force=true"));
                let body: serde_json::Value =
                    serde_json::from_slice(&request.into_body().collect_bytes().await.unwrap()).unwrap();
                captured.lock().push(body.clone());
                let (status, payload) = if reject.load(std::sync::atomic::Ordering::SeqCst) {
                    (
                        500,
                        json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"fixture","reason":"InternalError","code":500}),
                    )
                } else {
                    (200, body)
                };
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                        .unwrap(),
                )
            }
        }),
        "app",
    );
    let runtime = ControllerRuntime::default();
    let service = dns_service("");
    let ports = [ServicePort {
        port: 80,
        ..Default::default()
    }];
    for address in ["192.0.2.1", "2001:db8::1"] {
        let ips = [address.parse().unwrap()];
        let fingerprint = slice_fingerprint(&ips, &ports);
        let mut states = HashMap::from([(
            "app/source".into(),
            ServiceDnsState {
                applied_fingerprint: Some(fingerprint.clone()),
                ..Default::default()
            },
        )]);
        let original = build_endpoint_slices(
            "source",
            "app",
            service_owner_ref(&service).unwrap(),
            &ports,
            &ips,
            "fixture",
        );
        for case in [
            "matching",
            "uid",
            "family",
            "label",
            "name",
            "missing",
            "owner-missing",
            "owner-name",
            "owner-kind",
            "owner-version",
        ] {
            let mut slices = original.clone();
            match case {
                "uid" => slices[0].metadata.owner_references.as_mut().unwrap()[0].uid = "previous".into(),
                "family" => slices[0].address_type = "wrong".into(),
                "label" => {
                    slices[0]
                        .metadata
                        .labels
                        .as_mut()
                        .unwrap()
                        .insert(SERVICE_NAME.into(), "other".into());
                }
                "name" => slices[0].metadata.name = Some("wrong".into()),
                "missing" => slices.clear(),
                "owner-missing" => slices[0].metadata.owner_references = None,
                "owner-name" => slices[0].metadata.owner_references.as_mut().unwrap()[0].name = "other".into(),
                "owner-kind" => slices[0].metadata.owner_references.as_mut().unwrap()[0].kind = "Secret".into(),
                "owner-version" => {
                    slices[0].metadata.owner_references.as_mut().unwrap()[0].api_version = "other/v1".into()
                }
                _ => {}
            }
            let observed = slices
                .iter()
                .map(|s| {
                    (
                        (
                            s.metadata.namespace.as_deref().unwrap(),
                            s.metadata.name.as_deref().unwrap(),
                        ),
                        s,
                    )
                })
                .collect();
            let before = requests.lock().len();
            resolve_and_apply(
                &client,
                &service,
                "app",
                "source",
                address,
                &None,
                &mut states,
                &observed,
                &runtime,
            )
            .await;
            assert_eq!(
                requests.lock().len() - before,
                usize::from(case != "matching"),
                "{case}/{address}"
            );
            if case != "matching" {
                assert_eq!(
                    requests.lock().last().unwrap()["metadata"]["ownerReferences"][0]["uid"],
                    "current-uid"
                );
            }
        }
        states.get_mut("app/source").unwrap().applied_fingerprint = Some("previous-fingerprint".into());
        fail.store(true, std::sync::atomic::Ordering::SeqCst);
        resolve_and_apply(
            &client,
            &service,
            "app",
            "source",
            address,
            &None,
            &mut states,
            &HashMap::new(),
            &runtime,
        )
        .await;
        assert_eq!(
            states["app/source"].applied_fingerprint.as_deref(),
            Some("previous-fingerprint")
        );
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        resolve_and_apply(
            &client,
            &service,
            "app",
            "source",
            address,
            &None,
            &mut states,
            &HashMap::new(),
            &runtime,
        )
        .await;
        assert_eq!(
            states["app/source"].applied_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );
    }
}

#[test]
fn dual_family_skip_requires_both_current_owned_records() {
    let service = dns_service("");
    let ports = [ServicePort {
        port: 80,
        ..Default::default()
    }];
    let ips = ["192.0.2.1".parse().unwrap(), "2001:db8::1".parse().unwrap()];
    let slices = build_endpoint_slices(
        "source",
        "app",
        service_owner_ref(&service).unwrap(),
        &ports,
        &ips,
        "fixture",
    );
    let fingerprint = slice_fingerprint(&ips, &ports);
    let states = HashMap::from([(
        "app/source".into(),
        ServiceDnsState {
            applied_fingerprint: Some(fingerprint.clone()),
            ..Default::default()
        },
    )]);
    for missing in [None, Some(0), Some(1)] {
        let observed = slices
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != missing)
            .map(|(_, s)| {
                (
                    (
                        s.metadata.namespace.as_deref().unwrap(),
                        s.metadata.name.as_deref().unwrap(),
                    ),
                    s,
                )
            })
            .collect();
        assert_eq!(
            can_skip_owned_slices(&observed, &states, "app/source", &fingerprint, &service, &ips),
            missing.is_none()
        );
    }
}

#[tokio::test]
async fn producer_validation_off_still_rejects_invalid_identity_before_get() {
    const CHILD: &str = "EDGION_R6A_PRODUCER_VALIDATION_OFF_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","services::dns_resolver::service::tests::producer_validation_off_still_rejects_invalid_identity_before_get","--nocapture"]).env(CHILD,"1").output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let mut config = crate::config::EdgionControllerConfig::default();
    config.validation.enable_reference_grant_validation = false;
    crate::config::init_controller_config(config);
    let gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = gets.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            let count = count.clone();
            async move {
                assert!(request.uri().path().contains("linksys"));
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_,Infallible>(Response::builder().status(404).header(http::header::CONTENT_TYPE,"application/json").body(Body::from(serde_json::to_vec(&json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"NotFound","message":"fixture","code":404})).unwrap())).unwrap())
            }
        }),
        "app",
    );
    let runtime = ControllerRuntime::default();
    let mut source = dns_service("dns/resolver");
    let mut state = ServiceDnsState::default();
    assert!(matches!(
        effective_spec(
            &client,
            service_resolver_candidate(&source, &runtime),
            &None,
            &mut state
        )
        .await,
        ResolverSpec::System
    ));
    assert_eq!(
        gets.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "valid cross-namespace target reaches lookup without a Grant"
    );
    for case in ["owner", "target"] {
        if case == "owner" {
            source.metadata.namespace = Some("".into());
        } else {
            source.metadata.namespace = Some("app".into());
            source
                .metadata
                .annotations
                .as_mut()
                .unwrap()
                .insert("edgion.io/dns-resolver-ref".into(), "dns/a/b".into());
        }
        assert!(matches!(
            effective_spec(
                &client,
                service_resolver_candidate(&source, &runtime),
                &None,
                &mut state
            )
            .await,
            ResolverSpec::System
        ));
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn dual_family_partial_apply_preserves_fingerprint_until_complete_retry() {
    for failed_family in ["IPv4", "IPv6"] {
        let records = Arc::new(Mutex::new(Vec::<EndpointSlice>::new()));
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let captured = records.clone();
        let reject = fail.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let captured = captured.clone();
                let reject = reject.clone();
                async move {
                    assert_eq!(request.method(), http::Method::PATCH);
                    assert!(request.uri().query().unwrap().contains("force=true"));
                    let slice: EndpointSlice =
                        serde_json::from_slice(&request.into_body().collect_bytes().await.unwrap()).unwrap();
                    assert_eq!(slice.metadata.owner_references.as_ref().unwrap()[0].uid, "current-uid");
                    let failed =
                        reject.load(std::sync::atomic::Ordering::SeqCst) && slice.address_type == failed_family;
                    captured.lock().push(slice.clone());
                    let payload = if failed {
                        json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"InternalError","message":"fixture","code":500})
                    } else {
                        serde_json::to_value(slice).unwrap()
                    };
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(if failed { 500 } else { 200 })
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                            .unwrap(),
                    )
                }
            }),
            "app",
        );
        let source = dns_service("");
        let ports = source.spec.as_ref().unwrap().ports.as_ref().unwrap();
        let mut ips = vec![
            "2001:db8::1".parse().unwrap(),
            "192.0.2.1".parse().unwrap(),
            "192.0.2.1".parse().unwrap(),
        ];
        let mut states = HashMap::from([(
            "app/source".into(),
            ServiceDnsState {
                applied_fingerprint: Some("previous".into()),
                ..Default::default()
            },
        )]);
        apply_resolved_slices(&client, &source, ports, &mut ips, &mut states, &HashMap::new()).await;
        assert_eq!(records.lock().len(), 2, "both families attempted for {failed_family}");
        assert_eq!(ips.len(), 2, "production publication deduplicates addresses");
        assert_eq!(states["app/source"].applied_fingerprint.as_deref(), Some("previous"));
        let accepted: Vec<_> = records
            .lock()
            .iter()
            .filter(|s| s.address_type != failed_family)
            .cloned()
            .collect();
        let observed = accepted
            .iter()
            .map(|s| {
                (
                    (
                        s.metadata.namespace.as_deref().unwrap(),
                        s.metadata.name.as_deref().unwrap(),
                    ),
                    s,
                )
            })
            .collect();
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        apply_resolved_slices(&client, &source, ports, &mut ips, &mut states, &observed).await;
        assert_eq!(records.lock().len(), 4, "complete retry re-applies both families");
        assert_eq!(
            states["app/source"].applied_fingerprint.as_deref(),
            Some(slice_fingerprint(&ips, ports).as_str())
        );
        let complete = records.lock()[2..].to_vec();
        let observed = complete
            .iter()
            .map(|s| {
                (
                    (
                        s.metadata.namespace.as_deref().unwrap(),
                        s.metadata.name.as_deref().unwrap(),
                    ),
                    s,
                )
            })
            .collect();
        apply_resolved_slices(&client, &source, ports, &mut ips, &mut states, &observed).await;
        assert_eq!(
            records.lock().len(),
            4,
            "matching complete generation performs no PATCH"
        );
    }
}
