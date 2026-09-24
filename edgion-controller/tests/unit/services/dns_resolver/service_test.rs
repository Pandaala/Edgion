use super::*;
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
    last_applied.insert(key.clone(), fp.clone());

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
