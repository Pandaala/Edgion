use super::*;

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
    let first = state.prepare(&key, config);
    assert!(!first.is_ready());
    assert!(resolve(&ResolverSpec::HttpDns(first.clone()), "localhost")
        .await
        .is_err());
    let config = serde_json::from_value(
        serde_json::json!({"preset": "aliyun", "connection": {"timeoutMs": 0}, "fallback": {"type": "none"}}),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&first, &state.prepare(&key, config)));
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
        !Arc::ptr_eq(&first, &state.prepare(&other, config)),
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
