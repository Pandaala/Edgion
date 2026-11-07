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
    assert!(resolve(
        &ResolverSpec::HttpDns(Arc::clone(&failed)),
        "localhost",
        edgion_common::dns::DnsQueryPolicy::Native
    )
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
    assert!(resolve(
        &ResolverSpec::HttpDns(first.clone()),
        "localhost",
        edgion_common::dns::DnsQueryPolicy::Native
    )
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
async fn reconcile_lists_only_owned_slices_from_watch_cache() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            captured.lock().push((
                request.uri().path().to_string(),
                request.uri().query().unwrap_or_default().to_string(),
            ));
            Ok::<_, Infallible>(empty_kubernetes_list("discovery.k8s.io/v1", "EndpointSliceList"))
        }
    });

    let client = Client::new(service, "default");
    reconcile_once(
        &client,
        &["team-a".into(), "team-b".into()],
        &DnsResolverConfig::default(),
        &mut HashMap::new(),
        &runtime_with_link_sys([]),
    )
    .await
    .expect("namespace-scoped reconcile must succeed");

    let requests = requests.lock();
    assert_eq!(
        requests.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        vec![
            "/apis/discovery.k8s.io/v1/namespaces/team-a/endpointslices",
            "/apis/discovery.k8s.io/v1/namespaces/team-b/endpointslices",
        ],
        "Services come from the target table, never from an apiserver LIST"
    );
    assert!(requests
        .iter()
        .all(|(_, query)| query.contains("resourceVersion=0") && query.contains("labelSelector=")));
}

#[tokio::test(flavor = "current_thread")]
async fn controller_ticks_reuse_failed_preparation_and_clear_changed_or_removed_services() {
    let service = service_fn(move |request: http::Request<Body>| async move {
        let path = request.uri().path();
        assert!(path.contains("endpointslices"), "unexpected apiserver request: {path}");
        let payload =
            json!({"apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSliceList", "metadata": {}, "items": []});
        Ok::<_, Infallible>(
            Response::builder()
                .status(StatusCode::OK)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
    });
    let client = Client::new(service, "test");
    let config: edgion_resources::resources::link_sys::HttpDnsResolverConfig = serde_json::from_value(
        json!({"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}),
    )
    .unwrap();
    let runtime = runtime_with_link_sys([(*httpdns_owner(config)).clone()]);
    let target = |mode: usize| -> Service {
        serde_json::from_value(json!({"apiVersion": "v1", "kind": "Service", "metadata": {
            "namespace": "test", "name": "svc", "uid": "service-uid",
            "annotations": {"edgion.io/dns-resolution-mode": "controller", "edgion.io/dns-resolver-ref": if mode == 0 { "test/dns" } else { "" }}
            }, "spec": {"type": "ExternalName", "externalName": if mode == 0 { "unresolvable.invalid" } else { "1.2.3.4" }, "ports": [{"port": 80}]}}))
        .unwrap()
    };
    let config = DnsResolverConfig {
        enabled: true,
        ..Default::default()
    };
    let mut states = HashMap::new();
    runtime.dns_resolver_targets.sync_published(&target(0));
    reconcile_once(&client, &[], &config, &mut states, &runtime)
        .await
        .unwrap();
    let first = states["test/svc"].prepared.as_ref().unwrap().1.clone();
    assert!(!first.is_ready());
    states.get_mut("test/svc").unwrap().applied_fingerprint = Some("last-good".into());
    reconcile_once(&client, &[], &config, &mut states, &runtime)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&first, &states["test/svc"].prepared.as_ref().unwrap().1));
    assert_eq!(states["test/svc"].applied_fingerprint.as_deref(), Some("last-good"));
    // An empty resolver-ref selects System without touching LinkSys; the IP
    // literal takes resolve_system's pure fast path without any OS DNS lookup.
    runtime.dns_resolver_targets.sync_published(&target(1));
    reconcile_once(&client, &[], &config, &mut states, &runtime)
        .await
        .unwrap();
    assert!(states["test/svc"].prepared.is_none());
    runtime.dns_resolver_targets.remove_published(&target(1));
    reconcile_once(&client, &[], &config, &mut states, &runtime)
        .await
        .unwrap();
    assert!(
        states.is_empty(),
        "removed Service must release the prepared generation and slice state"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn passes_skip_targets_until_due_or_changed() {
    let client = Client::new(
        service_fn(|_request: http::Request<Body>| async move {
            Ok::<_, Infallible>(empty_kubernetes_list("discovery.k8s.io/v1", "EndpointSliceList"))
        }),
        "app",
    );
    let runtime = runtime_with_link_sys([]);
    // No ports: preparation returns before any query, but the target is handled.
    let mut service = dns_service("");
    service.spec.as_mut().unwrap().ports = None;
    runtime.dns_resolver_targets.sync_published(&service);
    let config = DnsResolverConfig::default();
    let grid = RefreshGrid::new(Duration::from_secs(60));
    let mut states = HashMap::new();
    let next = reconcile_with_runtime(&client, &[], &config, grid, &mut states, &runtime)
        .await
        .unwrap();
    let first_due = states["app/source"].next_due.unwrap();
    assert_eq!(next, first_due);
    let revision = states["app/source"].target_revision;
    assert!(revision.is_some());

    // Not due and unchanged: the pass leaves the schedule alone.
    reconcile_with_runtime(&client, &[], &config, grid, &mut states, &runtime)
        .await
        .unwrap();
    assert_eq!(states["app/source"].next_due, Some(first_due));

    // Identical content does not create a new revision.
    runtime.dns_resolver_targets.sync_published(&service);
    reconcile_with_runtime(&client, &[], &config, grid, &mut states, &runtime)
        .await
        .unwrap();
    assert_eq!(states["app/source"].target_revision, revision);

    // A content change is handled immediately.
    service.metadata.labels = Some([("edit".to_string(), "1".to_string())].into());
    runtime.dns_resolver_targets.sync_published(&service);
    reconcile_with_runtime(&client, &[], &config, grid, &mut states, &runtime)
        .await
        .unwrap();
    assert_ne!(states["app/source"].target_revision, revision);
}

#[tokio::test(flavor = "current_thread")]
async fn pass_before_service_cache_ready_neither_resolves_nor_collects() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            captured.lock().push(request.method().clone());
            async move { Ok::<_, Infallible>(empty_kubernetes_list("discovery.k8s.io/v1", "EndpointSliceList")) }
        }),
        "app",
    );
    // No Service processor is registered (startup or after a runtime reset).
    let runtime = ControllerRuntime::default();
    runtime.dns_resolver_targets.sync_published(&dns_service(""));
    let mut states = HashMap::new();
    reconcile_with_runtime(
        &client,
        &[],
        &DnsResolverConfig::default(),
        RefreshGrid::new(Duration::ZERO),
        &mut states,
        &runtime,
    )
    .await
    .unwrap();
    assert!(states.is_empty(), "no target is handled before the cache is ready");
    assert!(
        requests.lock().iter().all(|method| method == http::Method::GET),
        "an unready pass must not PATCH or DELETE"
    );
}

#[test]
fn refresh_grid_aligns_due_instants() {
    let period = Duration::from_secs(10);
    let grid = RefreshGrid::new(period);
    let epoch = grid.epoch;
    assert_eq!(grid.next_after(epoch), epoch + period);
    assert_eq!(grid.next_after(epoch + Duration::from_secs(3)), epoch + period);
    assert_eq!(grid.next_after(epoch + period), epoch + period * 2);
    assert_eq!(grid.next_after(epoch + Duration::from_secs(27)), epoch + period * 3);
    let immediate = RefreshGrid::new(Duration::ZERO);
    assert_eq!(immediate.next_after(epoch), epoch);
}

#[test]
fn default_resolver_falls_back_to_system_without_inline_servers() {
    assert!(
        matches!(
            controller_default_resolver(&ControllerRuntime::default()),
            ResolverSpec::System
        ),
        "a missing/httpdns default candidate must fail safe to the system resolver"
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
    let fp = slice_fingerprint(&a, &ports(&[80]), DnsLookupFamily::All);
    // Same inputs → same fingerprint (no spurious re-apply).
    assert_eq!(fp, slice_fingerprint(&a, &ports(&[80]), DnsLookupFamily::All));
    // A changed IP set or port set → different fingerprint (re-apply).
    assert_ne!(
        fp,
        slice_fingerprint(&[IpAddr::from([10, 0, 0, 1])], &ports(&[80]), DnsLookupFamily::All)
    );
    assert_ne!(fp, slice_fingerprint(&a, &ports(&[80, 443]), DnsLookupFamily::All));
}

#[test]
fn slice_fingerprint_tracks_published_optional_fields_and_ignores_target_port() {
    let ips = ["192.0.2.1".parse().unwrap()];
    let base = ServicePort {
        port: 80,
        ..Default::default()
    };
    let fingerprint = slice_fingerprint(&ips, std::slice::from_ref(&base), DnsLookupFamily::All);
    for field in ["name", "protocol", "appProtocol"] {
        let mut absent = serde_json::to_value(&base).unwrap();
        absent[field] = json!("");
        let empty: ServicePort = serde_json::from_value(absent.clone()).unwrap();
        absent[field] = json!("value");
        let present: ServicePort = serde_json::from_value(absent).unwrap();
        let empty_fp = slice_fingerprint(&ips, &[empty], DnsLookupFamily::All);
        let present_fp = slice_fingerprint(&ips, &[present], DnsLookupFamily::All);
        assert_ne!(fingerprint, empty_fp, "{field}");
        assert_ne!(fingerprint, present_fp, "{field}");
        assert_ne!(empty_fp, present_fp, "{field}");
    }
    let mut target_only = base.clone();
    target_only.target_port = Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(8080));
    assert_eq!(
        fingerprint,
        slice_fingerprint(&ips, &[target_only], DnsLookupFamily::All)
    );
    let sibling = ServicePort {
        port: 443,
        name: Some("https".into()),
        ..base.clone()
    };
    assert_eq!(
        slice_fingerprint(&ips, &[base.clone(), sibling.clone()], DnsLookupFamily::All),
        slice_fingerprint(&ips, &[sibling, base], DnsLookupFamily::All),
    );
}

#[tokio::test]
async fn published_port_edits_reapply_both_families_then_skip_unchanged_generation() {
    let requests = Arc::new(Mutex::new(Vec::<EndpointSlice>::new()));
    let captured = requests.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            let captured = captured.clone();
            async move {
                assert_eq!(request.method(), http::Method::PATCH);
                assert!(request.uri().query().unwrap().contains("force=true"));
                let bytes = request.into_body().collect_bytes().await.unwrap();
                captured.lock().push(serde_json::from_slice(&bytes).unwrap());
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(bytes))
                        .unwrap(),
                )
            }
        }),
        "app",
    );
    let mut service = dns_service("");
    let original_ports: Vec<ServicePort> = serde_json::from_value(json!([
        {"port":80,"name":"old","protocol":"TCP","appProtocol":"http","targetPort":8080},
        {"port":443,"name":"https","protocol":"TCP"}
    ]))
    .unwrap();
    for field in ["name", "protocol", "appProtocol"] {
        let mut states = HashMap::new();
        let mut ips = vec![
            "2001:db8::1".parse().unwrap(),
            "192.0.2.1".parse().unwrap(),
            "192.0.2.1".parse().unwrap(),
        ];
        service.spec.as_mut().unwrap().ports = Some(original_ports.clone());
        let before = requests.lock().len();
        apply_resolved_slices(
            &client,
            &service,
            &original_ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &HashMap::new(),
        )
        .await;
        assert_eq!(requests.lock().len(), before + 2);
        let published = requests.lock()[before..].to_vec();
        let observed = published
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
        let mut changed = serde_json::to_value(&original_ports).unwrap();
        changed[0][field] = json!(match field {
            "name" => "new",
            "protocol" => "UDP",
            _ => "kubernetes.io/h2c",
        });
        let changed: Vec<ServicePort> = serde_json::from_value(changed).unwrap();
        service.spec.as_mut().unwrap().ports = Some(changed.clone());
        apply_resolved_slices(
            &client,
            &service,
            &changed,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert_eq!(requests.lock().len(), before + 4, "{field}");
        let repaired = requests.lock()[before + 2..].to_vec();
        for slice in &repaired {
            let ports = slice.ports.as_ref().unwrap();
            assert_eq!(ports[0].name, changed[0].name);
            assert_eq!(ports[0].protocol, changed[0].protocol);
            assert_eq!(ports[0].app_protocol, changed[0].app_protocol);
            assert_eq!(ports[0].port, Some(80), "Service port, not targetPort");
            assert_eq!(slice.endpoints.len(), 1, "duplicate IP is removed");
        }
        let observed = repaired
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
        apply_resolved_slices(
            &client,
            &service,
            &changed,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        let mut reordered = changed;
        reordered.reverse();
        ips.reverse();
        apply_resolved_slices(
            &client,
            &service,
            &reordered,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert_eq!(requests.lock().len(), before + 4, "unchanged/reordered {field}");
    }
}

#[test]
fn controller_default_candidate_prefers_ref_then_servers() {
    let ref_cfg = DnsResolverConfig {
        lookup_family: DnsLookupFamily::default(),
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
        lookup_family: DnsLookupFamily::default(),
        enabled: true,
        refresh_interval: "15s".to_string(),
        link_sys_ref: None,
        servers: vec!["1.1.1.1".to_string()],
    };
    assert_eq!(
        controller_default_candidate(&servers_cfg),
        Some(ResolverCandidate::Servers(edgion_common::dns::StandardDnsServers::new(
            vec!["1.1.1.1".to_string()]
        )))
    );

    assert_eq!(controller_default_candidate(&DnsResolverConfig::default()), None);
}

/// One Service through the production prepare → query → publish phases.
#[allow(clippy::too_many_arguments)]
async fn resolve_and_apply(
    client: &Client,
    svc: &Service,
    default_candidate: &Option<ResolverCandidate>,
    default_family: DnsLookupFamily,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
    runtime: &ControllerRuntime,
) {
    let Some(resolution) = prepare_resolution(
        client,
        svc,
        default_candidate,
        default_family,
        last_applied,
        observed,
        runtime,
    )
    .await
    else {
        return;
    };
    let result = query(&resolution).await;
    finish_resolution(client, svc, resolution, result, last_applied, observed).await;
}

/// One pass in which every target is due.
async fn reconcile_once(
    client: &Client,
    namespaces: &[String],
    config: &DnsResolverConfig,
    states: &mut HashMap<String, ServiceDnsState>,
    runtime: &ControllerRuntime,
) -> Result<()> {
    super::reconcile_with_runtime(
        client,
        namespaces,
        config,
        RefreshGrid::new(Duration::ZERO),
        states,
        runtime,
    )
    .await
    .map(|_| ())
}

struct LinkSysPassthrough;

#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ProcessorHandler<LinkSys> for LinkSysPassthrough {}

struct ServicePassthrough;

#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ProcessorHandler<Service> for ServicePassthrough {}

/// A runtime with a ready Service cache whose processed LinkSys cache holds `owners`.
fn runtime_with_link_sys(owners: impl IntoIterator<Item = LinkSys>) -> Arc<ControllerRuntime> {
    use crate::conf_mgr::sync_runtime::resource_processor::ProcessorObj;
    let runtime = Arc::new(ControllerRuntime::default());
    let services = Arc::new(crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor::<
        Service,
    >::new("Service", 16, Arc::new(ServicePassthrough), runtime.clone()));
    runtime.processor_registry.register(services.clone());
    services.set_ready();
    let processor = Arc::new(crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor::<
        LinkSys,
    >::new("LinkSys", 16, Arc::new(LinkSysPassthrough), runtime.clone()));
    for owner in owners {
        processor.save(owner, None);
    }
    runtime.processor_registry.register(processor);
    runtime
}

fn resolver_link_sys() -> LinkSys {
    serde_json::from_value(json!({"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"name":"resolver","namespace":"dns"},"spec":{"type":"httpdns","config":{"preset":"aliyun","connection":{"timeoutMs":0},"fallback":{"type":"none"}}}})).unwrap()
}

fn with_external_name(service: &Service, external_name: &str) -> Service {
    let mut service = service.clone();
    service.spec.as_mut().unwrap().external_name = Some(external_name.into());
    service
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
async fn dns_producer_gates_tenant_and_preserves_default_authority() {
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| async move {
            let path = request.uri().path();
            assert!(path.contains("endpointslices"), "unexpected apiserver request: {path}");
            let payload = if request.method() == http::Method::PATCH {
                serde_json::from_slice(&request.into_body().collect_bytes().await.unwrap()).unwrap()
            } else {
                json!({"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","metadata":{},"items":[]})
            };
            Ok::<_, Infallible>(
                Response::builder()
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
        }),
        "app",
    );
    let runtime = runtime_with_link_sys([resolver_link_sys()]);
    let mut source = dns_service("dns/resolver");
    runtime.dns_resolver_targets.sync_published(&source);
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
        reconcile_once(&client, &[], &config, &mut states, &runtime)
            .await
            .unwrap();
        assert_eq!(states["app/source"].prepared.is_some(), kind == Some("Service"));
    }
    runtime.ref_grant_store.remove("dns/allow");
    let default_config = DnsResolverConfig {
        link_sys_ref: Some("dns/resolver".into()),
        ..config.clone()
    };
    reconcile_once(&client, &[], &default_config, &mut states, &runtime)
        .await
        .unwrap();
    assert!(
        states["app/source"].prepared.is_some(),
        "administrative default independently admits the same key"
    );
    for reference in ["bare", "/resolver", "dns/", "dns/a/b"] {
        source
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("edgion.io/dns-resolver-ref".into(), reference.into());
        runtime.dns_resolver_targets.sync_published(&source);
        reconcile_once(&client, &[], &config, &mut states, &runtime)
            .await
            .unwrap();
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
        let fingerprint = slice_fingerprint(&ips, &ports, DnsLookupFamily::All);
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
                &with_external_name(&service, address),
                &None,
                DnsLookupFamily::All,
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
            &with_external_name(&service, address),
            &None,
            DnsLookupFamily::All,
            &mut states,
            &HashMap::new(),
            &runtime,
        )
        .await;
        assert!(states["app/source"].applied_fingerprint.is_none());
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        resolve_and_apply(
            &client,
            &with_external_name(&service, address),
            &None,
            DnsLookupFamily::All,
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
    let fingerprint = slice_fingerprint(&ips, &ports, DnsLookupFamily::All);
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
async fn producer_validation_off_still_rejects_invalid_identity_before_lookup() {
    const CHILD: &str = "EDGION_R6A_PRODUCER_VALIDATION_OFF_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","services::dns_resolver::service::tests::producer_validation_off_still_rejects_invalid_identity_before_lookup","--nocapture"]).env(CHILD,"1").output().unwrap();
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
    let runtime = runtime_with_link_sys([resolver_link_sys()]);
    let mut source = dns_service("dns/resolver");
    let mut state = ServiceDnsState::default();
    assert!(
        matches!(
            effective_spec(
                &runtime,
                service_resolver_candidate(&source, &runtime),
                &None,
                &mut state
            ),
            ResolverSpec::HttpDns(_)
        ),
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
        assert!(
            matches!(
                effective_spec(
                    &runtime,
                    service_resolver_candidate(&source, &runtime),
                    &None,
                    &mut state
                ),
                ResolverSpec::System
            ),
            "{case}"
        );
    }
}

#[tokio::test]
async fn dual_family_partial_apply_invalidates_fingerprint_until_complete_retry() {
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
        apply_resolved_slices(
            &client,
            &source,
            ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &HashMap::new(),
        )
        .await;
        assert_eq!(records.lock().len(), 2, "both families attempted for {failed_family}");
        assert_eq!(ips.len(), 2, "production publication deduplicates addresses");
        assert!(states["app/source"].applied_fingerprint.is_none());
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
        apply_resolved_slices(
            &client,
            &source,
            ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert_eq!(records.lock().len(), 4, "complete retry re-applies both families");
        assert_eq!(
            states["app/source"].applied_fingerprint.as_deref(),
            Some(slice_fingerprint(&ips, ports, DnsLookupFamily::All).as_str())
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
        apply_resolved_slices(
            &client,
            &source,
            ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert_eq!(
            records.lock().len(),
            4,
            "matching complete generation performs no PATCH"
        );
    }
}

fn observed_dns_slices(service: &Service) -> Vec<EndpointSlice> {
    let mut slices = build_endpoint_slices(
        "source",
        "app",
        service_owner_ref(service).unwrap(),
        service.spec.as_ref().unwrap().ports.as_ref().unwrap(),
        &["192.0.2.1".parse().unwrap(), "2001:db8::1".parse().unwrap()],
        "fixture",
    );
    for (index, slice) in slices.iter_mut().enumerate() {
        slice.metadata.uid = Some(format!("slice-{index}"));
        slice.metadata.resource_version = Some(format!("rv-{index}"));
    }
    slices
}

fn observations(slices: &[EndpointSlice]) -> HashMap<(&str, &str), &EndpointSlice> {
    slices
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
        .collect()
}

type SliceRequests = Arc<Mutex<Vec<(http::Method, String, serde_json::Value)>>>;

fn slice_client(
    requests: SliceRequests,
    patch_code: Arc<std::sync::atomic::AtomicU16>,
    delete_code: Arc<std::sync::atomic::AtomicU16>,
) -> Client {
    Client::new(
        service_fn(move |request: http::Request<Body>| {
            let (requests, patch_code, delete_code) = (requests.clone(), patch_code.clone(), delete_code.clone());
            async move {
                let method = request.method().clone();
                let path = request.uri().path().to_string();
                assert!(
                    method == http::Method::PATCH || method == http::Method::DELETE,
                    "unexpected I/O: {method} {path}"
                );
                let body: serde_json::Value =
                    serde_json::from_slice(&request.into_body().collect_bytes().await.unwrap()).unwrap();
                let code = if method == http::Method::PATCH {
                    patch_code.load(std::sync::atomic::Ordering::SeqCst)
                } else {
                    delete_code.load(std::sync::atomic::Ordering::SeqCst)
                };
                requests.lock().push((method.clone(), path, body.clone()));
                let payload = if code == 200 && method == http::Method::PATCH {
                    body
                } else {
                    json!({"kind":"Status","apiVersion":"v1","status":if code == 200 {"Success"} else {"Failure"},"reason":"Fixture","message":"fixture","code":code})
                };
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(code)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                        .unwrap(),
                )
            }
        }),
        "app",
    )
}

#[tokio::test]
async fn partial_publication_cannot_skip_when_dns_returns_to_previous_answer() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for failure in ["DELETE", "IPv4", "IPv6"] {
        let service = dns_service("");
        let ports = service.spec.as_ref().unwrap().ports.as_ref().unwrap();
        let live = Arc::new(Mutex::new(Vec::<EndpointSlice>::new()));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(AtomicBool::new(false));
        let (stored, events, reject) = (live.clone(), writes.clone(), failing.clone());
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let (stored, events, reject) = (stored.clone(), events.clone(), reject.clone());
                async move {
                    let method = request.method().clone();
                    let name = request.uri().path().rsplit('/').next().unwrap().to_string();
                    let body = request.into_body().collect_bytes().await.unwrap();
                    events.lock().push(method.clone());
                    let mut code = 200;
                    let payload = if method == http::Method::PATCH {
                        let mut slice: EndpointSlice = serde_json::from_slice(&body).unwrap();
                        if reject.load(Ordering::SeqCst) && slice.address_type == failure {
                            code = 500;
                        } else {
                            slice.metadata.uid = Some(format!("uid-{name}"));
                            slice.metadata.resource_version = Some("1".into());
                            let mut slices = stored.lock();
                            slices.retain(|old| old.metadata.name.as_deref() != Some(name.as_str()));
                            slices.push(slice.clone());
                        }
                        serde_json::to_value(slice).unwrap()
                    } else {
                        assert_eq!(method, http::Method::DELETE);
                        if reject.load(Ordering::SeqCst) && failure == "DELETE" {
                            code = 409;
                        } else {
                            stored
                                .lock()
                                .retain(|old| old.metadata.name.as_deref() != Some(name.as_str()));
                        }
                        json!({"kind":"Status","apiVersion":"v1","status":"Success","code":200})
                    };
                    let payload = if code == 200 {
                        payload
                    } else {
                        json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Fixture","message":"fixture","code":code})
                    };
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(code)
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                            .unwrap(),
                    )
                }
            }),
            "app",
        );
        let original: Vec<IpAddr> = vec!["192.0.2.1".parse().unwrap(), "2001:db8::1".parse().unwrap()];
        let mut states = HashMap::new();
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut original.clone(),
            DnsLookupFamily::All,
            true,
            &mut states,
            &HashMap::new(),
        )
        .await;
        let original_fingerprint = states["app/source"].applied_fingerprint.clone();
        assert!(original_fingerprint.is_some());
        failing.store(true, Ordering::SeqCst);
        let snapshot = live.lock().clone();
        let mut changed = vec!["192.0.2.2".parse().unwrap()];
        if failure != "DELETE" {
            changed.push("2001:db8::2".parse().unwrap());
        }
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut changed,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observations(&snapshot),
        )
        .await;
        let partial = live.lock().clone();
        assert_eq!(partial.len(), 2, "both families still exist after {failure}");
        assert!(partial
            .iter()
            .flat_map(|slice| &slice.endpoints)
            .flat_map(|endpoint| &endpoint.addresses)
            .any(|ip| ip.ends_with('2')));

        // Returning to the old DNS answer must repair the partially changed content,
        // even though both family slices still have the expected owner and identity.
        failing.store(false, Ordering::SeqCst);
        let before = writes.lock().len();
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut original.clone(),
            DnsLookupFamily::All,
            true,
            &mut states,
            &observations(&partial),
        )
        .await;
        assert_eq!(writes.lock().len(), before + 2, "repair both families after {failure}");
        let repaired = live.lock().clone();
        let mut addresses: Vec<IpAddr> = repaired
            .iter()
            .flat_map(|slice| &slice.endpoints)
            .flat_map(|endpoint| &endpoint.addresses)
            .map(|ip| ip.parse().unwrap())
            .collect();
        addresses.sort();
        assert_eq!(addresses, original);
        assert_eq!(states["app/source"].applied_fingerprint, original_fingerprint);
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut original.clone(),
            DnsLookupFamily::All,
            true,
            &mut states,
            &observations(&repaired),
        )
        .await;
        assert_eq!(writes.lock().len(), before + 2, "fully repaired generation may skip");
    }
}

#[tokio::test]
async fn issue_008_complete_family_replacement_deletes_obsolete_sibling_and_retries() {
    use std::sync::atomic::{AtomicU16, Ordering};
    for survivor in ["192.0.2.1", "2001:db8::1"] {
        let service = dns_service("");
        let ports = service.spec.as_ref().unwrap().ports.as_ref().unwrap();
        let mut ips = vec![survivor.parse().unwrap()];
        let slices = observed_dns_slices(&service);
        let observed = observations(&slices);
        let expected = slice_fingerprint(&ips, ports, DnsLookupFamily::All);
        let mut states = HashMap::from([(
            "app/source".into(),
            ServiceDnsState {
                applied_fingerprint: Some(expected.clone()),
                ..Default::default()
            },
        )]);
        assert!(!can_skip_owned_slices(
            &observed,
            &states,
            "app/source",
            &expected,
            &service,
            &ips
        ));
        states.get_mut("app/source").unwrap().applied_fingerprint = Some("previous".into());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let delete_code = Arc::new(AtomicU16::new(409));
        let client = slice_client(requests.clone(), Arc::new(AtomicU16::new(200)), delete_code.clone());
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert!(states["app/source"].applied_fingerprint.is_none());
        delete_code.store(200, Ordering::SeqCst);
        apply_resolved_slices(
            &client,
            &service,
            ports,
            &mut ips,
            DnsLookupFamily::All,
            true,
            &mut states,
            &observed,
        )
        .await;
        assert_eq!(
            states["app/source"].applied_fingerprint.as_deref(),
            Some(expected.as_str())
        );
        let events = requests.lock();
        assert_eq!(
            events.iter().map(|event| event.0.clone()).collect::<Vec<_>>(),
            [
                http::Method::PATCH,
                http::Method::DELETE,
                http::Method::PATCH,
                http::Method::DELETE
            ]
        );
        let obsolete_index = usize::from(ips[0].is_ipv4());
        assert!(events[1]
            .1
            .ends_with(slices[obsolete_index].metadata.name.as_deref().unwrap()));
        assert_eq!(events[1].2["preconditions"]["uid"], format!("slice-{obsolete_index}"));
        assert_eq!(
            events[1].2["preconditions"]["resourceVersion"],
            format!("rv-{obsolete_index}")
        );
    }
}

#[tokio::test]
async fn failed_patch_preserves_allowed_sibling_and_404_delete_completes() {
    use std::sync::atomic::{AtomicU16, Ordering};
    let service = dns_service("");
    let ports = service.spec.as_ref().unwrap().ports.as_ref().unwrap();
    let slices = observed_dns_slices(&service);
    let observed = observations(&slices);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let patch_code = Arc::new(AtomicU16::new(500));
    let client = slice_client(requests.clone(), patch_code.clone(), Arc::new(AtomicU16::new(404)));
    let mut states = HashMap::new();
    let mut ips = vec!["192.0.2.2".parse().unwrap()];
    apply_resolved_slices(
        &client,
        &service,
        ports,
        &mut ips,
        DnsLookupFamily::All,
        true,
        &mut states,
        &observed,
    )
    .await;
    assert!(requests.lock().iter().all(|event| event.0 == http::Method::PATCH));
    assert!(states.is_empty());
    patch_code.store(200, Ordering::SeqCst);
    apply_resolved_slices(
        &client,
        &service,
        ports,
        &mut ips,
        DnsLookupFamily::All,
        true,
        &mut states,
        &observed,
    )
    .await;
    assert!(states["app/source"].applied_fingerprint.is_some());
    assert_eq!(requests.lock().last().unwrap().0, http::Method::DELETE);
}

#[tokio::test]
async fn obsolete_slice_deletion_requires_proven_ownership_and_observed_identity() {
    use std::sync::atomic::AtomicU16;
    let service = dns_service("");
    for case in [
        "old-service-uid",
        "namespace",
        "name",
        "service-label",
        "managed-label",
        "synthetic-label",
        "owner-kind",
        "owner-version",
        "owner-name",
        "owner-uid",
        "slice-uid",
        "slice-rv",
    ] {
        let mut slice = observed_dns_slices(&service).remove(1);
        match case {
            "old-service-uid" => slice.metadata.owner_references.as_mut().unwrap()[0].uid = "old-incarnation".into(),
            "namespace" => slice.metadata.namespace = Some("other".into()),
            "name" => slice.metadata.name = Some("other".into()),
            "service-label" => {
                slice
                    .metadata
                    .labels
                    .as_mut()
                    .unwrap()
                    .insert(SERVICE_NAME.into(), "other".into());
            }
            "managed-label" => {
                slice.metadata.labels.as_mut().unwrap().remove(ENDPOINTSLICE_MANAGED_BY);
            }
            "synthetic-label" => {
                slice.metadata.labels.as_mut().unwrap().remove(SYNTHETIC);
            }
            "owner-kind" => slice.metadata.owner_references.as_mut().unwrap()[0].kind = "Secret".into(),
            "owner-version" => slice.metadata.owner_references.as_mut().unwrap()[0].api_version = "other/v1".into(),
            "owner-name" => slice.metadata.owner_references.as_mut().unwrap()[0].name = "other".into(),
            "owner-uid" => slice.metadata.owner_references.as_mut().unwrap()[0].uid.clear(),
            "slice-uid" => slice.metadata.uid = None,
            "slice-rv" => slice.metadata.resource_version = None,
            _ => unreachable!(),
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = slice_client(
            requests.clone(),
            Arc::new(AtomicU16::new(200)),
            Arc::new(AtomicU16::new(200)),
        );
        assert_eq!(
            delete_owned_slice(&client, &slice, "app", "source", true).await,
            case == "old-service-uid",
            "{case}"
        );
        assert_eq!(requests.lock().len(), usize::from(case == "old-service-uid"), "{case}");
    }
}

#[tokio::test]
async fn policy_withdrawal_precedes_failed_dns_and_invalid_policy_permits_no_query() {
    use std::sync::atomic::AtomicU16;
    for (policy, forbidden_literal, removed) in [
        ("ipv4Only", "2001:db8::2", 1),
        ("ipv6Only", "192.0.2.2", 1),
        ("", "invalid.example", 2),
    ] {
        let mut service = dns_service("");
        service
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(DNS_LOOKUP_FAMILY.into(), policy.into());
        let slices = observed_dns_slices(&service);
        let observed = observations(&slices);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = slice_client(
            requests.clone(),
            Arc::new(AtomicU16::new(200)),
            Arc::new(AtomicU16::new(200)),
        );
        let mut states = HashMap::new();
        resolve_and_apply(
            &client,
            &with_external_name(&service, forbidden_literal),
            &None,
            DnsLookupFamily::All,
            &mut states,
            &observed,
            &ControllerRuntime::default(),
        )
        .await;
        assert_eq!(requests.lock().len(), removed, "{policy}");
        assert!(requests.lock().iter().all(|event| event.0 == http::Method::DELETE));
        assert!(states["app/source"].applied_fingerprint.is_none());
    }
}

#[test]
fn controller_family_override_is_strict_and_independent_of_resolver_selection() {
    let mut service = dns_service("");
    assert_eq!(
        service_lookup_family(&service, DnsLookupFamily::Ipv6Only),
        Ok(DnsLookupFamily::Ipv6Only)
    );
    service
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(DNS_LOOKUP_FAMILY.into(), "all".into());
    assert_eq!(
        service_lookup_family(&service, DnsLookupFamily::Ipv4Only),
        Ok(DnsLookupFamily::All)
    );
    for invalid in ["", "ipv4", " all", "ALL"] {
        service
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(DNS_LOOKUP_FAMILY.into(), invalid.into());
        assert!(service_lookup_family(&service, DnsLookupFamily::Ipv4Only).is_err());
    }
}

#[tokio::test]
async fn failed_policy_withdrawal_stays_pending_while_allowed_family_is_published() {
    use std::sync::atomic::{AtomicU16, Ordering};
    let service = dns_service("");
    let slices = observed_dns_slices(&service);
    let observed = observations(&slices);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let delete_code = Arc::new(AtomicU16::new(409));
    let client = slice_client(requests.clone(), Arc::new(AtomicU16::new(200)), delete_code.clone());
    let mut states = HashMap::new();
    resolve_and_apply(
        &client,
        &with_external_name(&service, "192.0.2.2"),
        &None,
        DnsLookupFamily::Ipv4Only,
        &mut states,
        &observed,
        &ControllerRuntime::default(),
    )
    .await;
    assert_eq!(
        requests.lock().iter().map(|event| event.0.clone()).collect::<Vec<_>>(),
        [http::Method::DELETE, http::Method::PATCH, http::Method::DELETE]
    );
    assert!(states["app/source"].applied_fingerprint.is_none());
    delete_code.store(200, Ordering::SeqCst);
    resolve_and_apply(
        &client,
        &with_external_name(&service, "192.0.2.2"),
        &None,
        DnsLookupFamily::Ipv4Only,
        &mut states,
        &observed,
        &ControllerRuntime::default(),
    )
    .await;
    assert!(states["app/source"].applied_fingerprint.is_some());
}

#[tokio::test]
async fn orphan_gc_uses_guarded_deletion_and_preserves_desired_or_foreign_objects() {
    use std::sync::atomic::AtomicU16;
    let service = dns_service("");
    let mut slices = observed_dns_slices(&service);
    slices[1].metadata.owner_references.as_mut().unwrap()[0].kind = "Secret".into();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = slice_client(
        requests.clone(),
        Arc::new(AtomicU16::new(200)),
        Arc::new(AtomicU16::new(200)),
    );
    let mut states = HashMap::from([("app/source".into(), ServiceDnsState::default())]);
    gc_orphaned_slices(&client, &slices, &HashSet::from(["app/source".into()]), &mut states).await;
    assert!(requests.lock().is_empty());
    gc_orphaned_slices(&client, &slices, &HashSet::new(), &mut states).await;
    assert_eq!(requests.lock().len(), 1);
    assert_eq!(requests.lock()[0].2["preconditions"]["uid"], "slice-0");
    assert!(states.is_empty());
}
