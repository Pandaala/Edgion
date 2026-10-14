use super::*;

fn build_resolved_target(target: &WebhookTarget, tls_enabled: bool) -> Result<ResolvedTarget, String> {
    build_resolved_target_in_namespace(target, tls_enabled, None)
}
use edgion_resources::resources::common::external_http::WebhookServiceRef;

fn url_target(url: &str) -> WebhookTarget {
    WebhookTarget {
        url: Some(url.to_string()),
        ..Default::default()
    }
}

/// A URL target resolves to a Static base URL whose scheme comes from the URL,
/// not from `tls.is_some()`.
#[test]
fn build_resolved_target_url_uses_url_scheme() {
    // https URL with no tls block must still be https.
    let t = build_resolved_target(&url_target("https://api.example.com"), false).unwrap();
    match t {
        ResolvedTarget::Static(url) => assert_eq!(url, "https://api.example.com"),
        other => panic!("expected Static, got {other:?}"),
    }

    // http URL must stay http even when a tls block is present (custom CA only).
    let t = build_resolved_target(&url_target("http://api.example.com:8080"), true).unwrap();
    match t {
        ResolvedTarget::Static(url) => assert_eq!(url, "http://api.example.com:8080"),
        other => panic!("expected Static, got {other:?}"),
    }
}

/// An invalid URL target surfaces the schema-layer error verbatim.
#[test]
fn build_resolved_target_rejects_non_origin_url() {
    let err = build_resolved_target(&url_target("https://api.example.com/v1"), false).unwrap_err();
    assert!(err.contains("origin-only"), "got: {err}");
}

/// Service targets are unchanged.
#[test]
fn build_resolved_target_service_cluster_ip_unchanged() {
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("ServiceClusterIp".to_string()),
            name: Some("auth-svc".to_string()),
            namespace: Some("prod".to_string()),
            port: Some(8080),
            ..Default::default()
        },
        ..Default::default()
    };
    match build_resolved_target(&target, false).unwrap() {
        ResolvedTarget::Static(url) => assert_eq!(url, "http://auth-svc.prod.svc.cluster.local:8080"),
        other => panic!("expected Static, got {other:?}"),
    }
}

/// TDD test (written before implementation): Drop aborts the spawned health task.
///
/// RED: fails because ManagedOutboundClient / health_task do not exist yet.
/// GREEN: passes after from_parts + Drop impl are wired in.
#[tokio::test]
async fn drop_aborts_health_task() {
    let client = test_client_with_active_health().await;
    let handle = client.health_task.as_ref().unwrap().abort_handle();
    drop(client);
    tokio::task::yield_now().await;
    assert!(handle.is_finished());
}

/// TDD test: dial_url rejects loopback (SSRF guard).
///
/// A ManagedOutboundClient whose resolved_target points to 127.0.0.1:80
/// must return Err(SsrfRejected) from dial_url.
#[test]
fn dial_url_rejects_loopback_target() {
    let client = ManagedOutboundClient::test_with_static_target("http://127.0.0.1:80");
    let result = client.dial_url();
    assert!(
        matches!(result, Err(ManagedOutboundDialError::SsrfRejected(_))),
        "expected SsrfRejected, got {result:?}"
    );
}

/// A non-loopback IP address is allowed through dial_url.
#[test]
fn dial_url_allows_routable_ip() {
    let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
    let result = client.dial_url();
    assert!(
        matches!(result, Ok(ref u) if u == "http://10.0.0.1:8080"),
        "expected Ok with url, got {result:?}"
    );
}

/// A Static target with a DNS hostname is not blocked by the literal-level guard.
/// The skip is intentional: `classify_backend_addr` only acts on IP addresses, and
/// hostname targets are protected by `EgressResolver` at connect time instead.
#[test]
fn dial_url_skips_literal_guard_for_dns_hostname() {
    let client = ManagedOutboundClient::test_with_static_target("http://my-svc.default.svc.cluster.local:8080");
    let result = client.dial_url();
    assert!(
        matches!(result, Ok(ref u) if u == "http://my-svc.default.svc.cluster.local:8080"),
        "DNS hostnames must pass the literal guard unchanged; got {result:?}"
    );
}

/// A hostname is not an IP literal: the guard is skipped explicitly, and no
/// Unix socket address is ever manufactured.
#[test]
fn extract_ip_literal_returns_none_for_hostname() {
    let got = extract_ip_literal("http://my-svc.default.svc.cluster.local:8080").unwrap();
    assert!(got.is_none(), "a DNS hostname must yield None, got {got:?}");
}

#[test]
fn extract_ip_literal_parses_ipv4_and_ipv6() {
    let v4 = extract_ip_literal("http://10.0.0.1:8080").unwrap().unwrap();
    assert_eq!(v4.to_string(), "10.0.0.1:8080");

    let v6 = extract_ip_literal("https://[2001:db8::1]:8443").unwrap().unwrap();
    assert_eq!(v6.port(), 8443);
}

/// A scheme's default port is used when the URL omits one.
#[test]
fn extract_ip_literal_uses_default_port() {
    let addr = extract_ip_literal("https://10.0.0.1").unwrap().unwrap();
    assert_eq!(addr.port(), 443);
}

/// dial_url returns NoEndpoint when the Service target has no ready backends.
#[test]
fn dial_url_returns_no_endpoint_for_empty_service() {
    use std::sync::atomic::AtomicUsize;
    let client = ManagedOutboundClient {
        http_client: get_http_client_arc(),
        resolved_target: ResolvedTarget::Service {
            service_key: "nonexistent-ns/nonexistent-svc".to_string(),
            port: Some(8080),
            scheme: "http",
            cursor: AtomicUsize::new(0),
        },
        compiled_request: CompiledRequest::from_spec(
            &edgion_resources::resources::common::outbound_request::OutboundRequestSpec::default(),
        )
        .unwrap(),
        health: HealthState::new(),
        rate: None,
        success: None,
        max_response_bytes: 1024,
        timeout: Duration::from_secs(5),
        timeout_template: None,
        retry: None,
        egress_policy: None,
        passive_health_check: None,
        health_task: None,
        resolved_secrets: None,
    };
    let result = client.dial_url();
    assert!(
        matches!(result, Err(ManagedOutboundDialError::NoEndpoint)),
        "expected NoEndpoint, got {result:?}"
    );
}

/// from_parts rejects an invalid target (missing port).
#[test]
fn from_parts_rejects_missing_port() {
    use edgion_resources::resources::common::external_http::{
        ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
    };
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            service: WebhookServiceRef {
                kind: None,
                name: Some("svc".to_string()),
                namespace: Some("default".to_string()),
                port: None, // missing port — must be rejected
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let result = owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default");
    assert!(result.is_err(), "from_parts must reject missing port");
    let msg = result.unwrap_err();
    assert!(msg.contains("port"), "error should mention port, got: {msg}");
}

/// blockPrivate must reject an IP-literal URL target, which never reaches
/// EgressResolver because hyper-util skips DNS for IP literals.
#[test]
fn dial_url_blocks_private_ip_literal_when_policy_set() {
    let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
    client.egress_policy = Some(EgressPolicy { block_private: true });
    match client.dial_url() {
        Err(ManagedOutboundDialError::SsrfRejected(msg)) => {
            assert!(msg.contains("blockPrivate"), "got: {msg}");
        }
        other => panic!("expected SsrfRejected, got {other:?}"),
    }
}

/// The same literal passes when blockPrivate is off — matching the resolver path,
/// which allows private ranges by default.
#[test]
fn dial_url_allows_private_ip_literal_by_default() {
    let mut client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
    client.egress_policy = Some(EgressPolicy { block_private: false });
    assert!(client.dial_url().is_ok());
}

/// Link-local is rejected regardless of blockPrivate, on both paths.
#[test]
fn dial_url_blocks_metadata_ip_literal_regardless_of_policy() {
    let mut client = ManagedOutboundClient::test_with_static_target("http://169.254.169.254:80");
    client.egress_policy = Some(EgressPolicy { block_private: false });
    assert!(matches!(
        client.dial_url(),
        Err(ManagedOutboundDialError::SsrfRejected(_))
    ));
}

/// 0.0.0.0 must be rejected regardless of blockPrivate: `validate_endpoint_backend_addr`
/// does not block Unspecified (Service targets legitimately allow it), so the URL-target
/// path needs its own unconditional check independent of the `block_private` gate.
#[test]
fn dial_url_blocks_unspecified_ip_literal_regardless_of_policy() {
    let mut client = ManagedOutboundClient::test_with_static_target("http://0.0.0.0:8080");
    client.egress_policy = Some(EgressPolicy { block_private: false });
    assert!(matches!(
        client.dial_url(),
        Err(ManagedOutboundDialError::SsrfRejected(_))
    ));
}

#[test]
fn active_health_url_preserves_origin_and_parsed_query() {
    let url = prepare_active_health_url(
        "https://10.1.2.3:8443/original?stale=1",
        Some("/healthz?ready=1&scope=%2Fapi"),
        None,
    )
    .expect("a safe Service probe URL must compose");

    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("10.1.2.3"));
    assert_eq!(url.port_or_known_default(), Some(8443));
    assert_eq!(url.path(), "/healthz");
    assert_eq!(url.query(), Some("ready=1&scope=%2Fapi"));
    assert!(url.username().is_empty());
    assert!(url.password().is_none());

    let encoded = prepare_active_health_url("http://10.1.2.3:8080", Some("/%2F%5C%23?next=%2F"), None)
        .expect("percent-encoded structural bytes are path data");
    assert_eq!(encoded.path(), "/%2F%5C%23");
    assert_eq!(encoded.query(), Some("next=%2F"));
}

#[test]
fn active_health_url_defensively_rejects_authority_references() {
    for path in [
        "healthz",
        "https://evil.example/healthz",
        "//evil.example/healthz",
        "///evil.example/healthz",
        r"/\evil.example/healthz",
        "/healthz#details",
    ] {
        assert!(
            prepare_active_health_url("http://10.1.2.3:8080", Some(path), None).is_err(),
            "path {path:?} must fail closed at runtime"
        );
    }
}

#[test]
fn active_health_url_applies_target_specific_literal_policy() {
    assert!(
        prepare_active_health_url("http://10.1.2.3:8080", Some("/healthz"), None).is_ok(),
        "Service probes must allow RFC1918 endpoints"
    );
    assert!(
        prepare_active_health_url("http://169.254.169.254:80", Some("/healthz"), None).is_err(),
        "Service probes must reject link-local metadata endpoints"
    );
    assert!(prepare_active_health_url(
        "http://10.1.2.3:8080",
        Some("/healthz"),
        Some(EgressPolicy { block_private: true }),
    )
    .is_err());
    assert!(prepare_active_health_url(
        "http://0.0.0.0:8080",
        Some("/healthz"),
        Some(EgressPolicy { block_private: false }),
    )
    .is_err());
    assert!(prepare_active_health_url(
        "http://127.0.0.1:8080",
        Some("/healthz"),
        Some(EgressPolicy { block_private: false }),
    )
    .is_err());
}

fn active_health_thresholds() -> ActiveHealthCheck {
    ActiveHealthCheck {
        path: Some("/healthz".to_string()),
        interval_sec: 10,
        timeout_ms: 2_000,
        healthy_threshold: 2,
        unhealthy_threshold: 2,
    }
}

#[test]
fn active_only_health_opens_and_recovers_at_thresholds() {
    let health = HealthState::new();
    let active = active_health_thresholds();
    let mut failures = 0;
    let mut successes = 0;

    record_active_health_result(&health, &active, false, &mut failures, &mut successes, false);
    assert!(health.healthy.load(Ordering::Relaxed));
    record_active_health_result(&health, &active, false, &mut failures, &mut successes, false);
    assert!(!health.healthy.load(Ordering::Relaxed));

    record_active_health_result(&health, &active, false, &mut failures, &mut successes, true);
    assert!(!health.healthy.load(Ordering::Relaxed));
    record_active_health_result(&health, &active, false, &mut failures, &mut successes, true);
    assert!(health.healthy.load(Ordering::Relaxed));
}

#[test]
fn combined_health_passive_opens_and_active_only_recovers() {
    let health = HealthState::new();
    let active = active_health_thresholds();
    let mut failures = 0;
    let mut successes = 0;

    record_active_health_result(&health, &active, true, &mut failures, &mut successes, false);
    record_active_health_result(&health, &active, true, &mut failures, &mut successes, false);
    assert!(
        health.healthy.load(Ordering::Relaxed),
        "active failure must not open a passive-owned circuit"
    );

    health.healthy.store(false, Ordering::Relaxed);
    health.passive_failures.store(7, Ordering::Relaxed);
    health.last_halfopen.store(41, Ordering::Relaxed);
    health.backoff_sec.store(13, Ordering::Relaxed);
    record_active_health_result(&health, &active, true, &mut failures, &mut successes, true);
    record_active_health_result(&health, &active, true, &mut failures, &mut successes, true);

    assert!(health.healthy.load(Ordering::Relaxed));
    assert_eq!(health.passive_failures.load(Ordering::Relaxed), 7);
    assert_eq!(health.last_halfopen.load(Ordering::Relaxed), 41);
    assert_eq!(health.backoff_sec.load(Ordering::Relaxed), 13);
}

/// A URL target with no TLS block must still get a dedicated client — otherwise
/// it would fall back to the shared global client, which carries no EgressResolver.
#[test]
fn url_target_without_tls_gets_a_dedicated_client() {
    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            url: Some("https://api.example.com".to_string()),
            ..Default::default()
        },
        tls: None,
        ..Default::default()
    };
    let client =
        owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default").expect("url target must build");

    // The shared global client is a distinct Arc allocation; a dedicated client
    // must not alias it.
    assert!(
        !Arc::ptr_eq(&client.http_client, &get_http_client_arc()),
        "a URL target must not reuse the shared global client"
    );
}

/// A Service-POOL target (`Service`/`ServiceEndpointSlice`, i.e. it resolves
/// to `ResolvedTarget::Service`) with no TLS block now gets its OWN client
/// carrying `AnchorPinnedResolver`, even with no TLS and no
/// `upstreamAuthority` policy currently active. This is the intended cost
/// of closing the build-time/request-time split-brain SSRF: resolver
/// installation must not depend on whether a policy exists yet, so it is
/// unconditional for every target that can become the host-rewrite target
/// in `outbound_call` (see the actual-owner managed client builder).
#[test]
fn service_pool_target_without_tls_gets_a_dedicated_client() {
    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            service: WebhookServiceRef {
                name: Some("auth-svc".to_string()),
                port: Some(8080),
                ..Default::default()
            },
            ..Default::default()
        },
        tls: None,
        ..Default::default()
    };
    let client =
        owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default").expect("service target must build");
    assert!(
        !Arc::ptr_eq(&client.http_client, &get_http_client_arc()),
        "a Service-pool target must carry its own client with the pinned resolver, \
             never the shared global client"
    );
}

/// `ServiceClusterIp` is NOT a Service-pool target — it resolves to a
/// `Static` cluster.local URL, `authority_service_key()` is `None` for it,
/// and a pinned resolver would wrongly bypass the ClusterIP. It must keep
/// reusing the shared global client when no TLS block is configured.
#[test]
fn service_cluster_ip_target_without_tls_reuses_global_client() {
    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            service: WebhookServiceRef {
                kind: Some("ServiceClusterIp".to_string()),
                name: Some("auth-svc".to_string()),
                port: Some(8080),
                ..Default::default()
            },
            ..Default::default()
        },
        tls: None,
        ..Default::default()
    };
    let client = owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default")
        .expect("ServiceClusterIp target must build");
    assert!(
        Arc::ptr_eq(&client.http_client, &get_http_client_arc()),
        "ServiceClusterIp must keep reusing the shared global client, unchanged"
    );
    assert!(
        client.authority_service_key().is_none(),
        "ServiceClusterIp resolves to Static, so it has no authority_service_key"
    );
}

/// CRITICAL regression (SSRF split-brain fix): a Service-pool client built
/// BEFORE an `upstreamAuthority` policy exists for its Service must
/// nonetheless carry a pinned resolver — closing the window where a policy
/// attached AFTER the client was built (a routine hot-reload ordering)
/// would otherwise leave `outbound_call`'s request-composed host to
/// resolve through the real system DNS resolver.
///
/// Reproduces the exact operator flow: (1) seed EndpointSlice endpoints and
/// build the client with no authority policy in the store, (2) attach an
/// `upstreamAuthority` policy to the SAME Service key afterward, (3) assert
/// the client (built in step 1) does not alias the shared global client —
/// the only way it can carry the pinned resolver reqwest needs before the
/// composed authority is ever rendered.
#[test]
fn service_pool_client_gets_pinned_resolver_before_authority_policy_exists() {
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use crate::backends::policy::backend_traffic::store::{get_backend_traffic_policy_store, GLOBAL_STORE_TEST_LOCK};
    use edgion_resources::resources::edgion_backend_traffic_policy::{EdgionBackendTrafficPolicySpec, PolicyTargetRef};
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};

    // Serializes against every other test that writes the process-global
    // backend-traffic-policy store (see `GLOBAL_STORE_TEST_LOCK`'s doc).
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();

    // Unique namespace/service so a leaked entry cannot collide with any
    // other test running in this process.
    let ns = "pinned-resolver-order-test";
    let svc = "anchor-svc";
    let service_key = format!("{ns}/{svc}");

    // ── Step 0: seed a ready endpoint for the anchor Service. ───────────
    let mut labels = BTreeMap::new();
    labels.insert(
        edgion_resources::constants::labels::k8s::SERVICE_NAME.to_string(),
        svc.to_string(),
    );
    let slice = EndpointSlice {
        metadata: ObjectMeta {
            name: Some("anchor-svc-eps1".to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        ports: Some(vec![EndpointPort {
            port: Some(8080),
            ..Default::default()
        }]),
        endpoints: vec![Endpoint {
            addresses: vec!["10.1.2.3".to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }],
    };
    let mut add_eps = HashMap::new();
    add_eps.insert(format!("{ns}/anchor-svc-eps1"), slice);
    get_roundrobin_store().update_data_only(add_eps, HashMap::new(), &HashSet::new());

    // ── Step 1: build the client while NO authority policy exists. ──────
    assert!(
        get_backend_traffic_policy_store().get_authority(&service_key).is_none(),
        "precondition: no authority policy must exist yet"
    );
    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            service: WebhookServiceRef {
                name: Some(svc.to_string()),
                namespace: Some(ns.to_string()),
                port: Some(8080),
                ..Default::default()
            },
            ..Default::default()
        },
        tls: None,
        ..Default::default()
    };
    let client = owned_webhook_client(&conn, &OutboundRequestSpec::default(), ns).expect("service target must build");

    // ── Step 2: attach an upstreamAuthority policy AFTER the client exists. ──
    let policy_key = format!("{ns}/authtest");
    let mut add_policy = HashMap::new();
    add_policy.insert(
        policy_key.clone(),
        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("authtest".to_string()),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                prepared_precedence: Some(
                    edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                        creation_timestamp: None,
                    },
                ),
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: Some(String::new()),
                    kind: "Service".to_string(),
                    name: svc.to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                retry_constraint: None,
                load_balancer: None,
                health_check: None,
                outlier_detection: None,
                circuit_breaker: None,
                connection: None,
                upstream_authority: Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: None,
                }),
            },
            status: None,
        }),
    );
    get_backend_traffic_policy_store().partial_update_policies(add_policy, HashMap::new(), HashSet::new());
    assert!(
        get_backend_traffic_policy_store().get_authority(&service_key).is_some(),
        "the authority policy must now be active for this Service key"
    );

    // ── Step 3: the client built in step 1 must ALREADY be safe. ────────
    // If resolver installation were still gated on a live authority
    // lookup at build time (the bug), this client would alias the shared
    // global client — which has no injected resolver at all — and the host
    // rewrite `outbound_call` now performs would resolve through the real
    // system DNS resolver.
    assert!(
        !Arc::ptr_eq(&client.http_client, &get_http_client_arc()),
        "a Service-pool client must carry its own pinned-resolver client \
             even when built before any upstreamAuthority policy existed"
    );

    // Cleanup: remove the policy this test inserted into the global store
    // so it doesn't leak into other tests running in the same process.
    get_backend_traffic_policy_store().partial_update_policies(
        HashMap::new(),
        HashMap::new(),
        HashSet::from([policy_key]),
    );
}

/// `resolve_probe_base` — the outbound active-health task's per-tick host
/// override — must rewrite the probe URL's host to `healthCheckHost` when
/// the Service anchor carries an `upstreamAuthority` policy that sets one.
#[test]
fn resolve_probe_base_uses_authority_health_check_host() {
    use crate::backends::policy::backend_traffic::store::{get_backend_traffic_policy_store, GLOBAL_STORE_TEST_LOCK};
    use edgion_resources::resources::edgion_backend_traffic_policy::{EdgionBackendTrafficPolicySpec, PolicyTargetRef};
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{HashMap, HashSet};

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();

    let ns = "probe-base-authority-test";
    let svc = "anchor-svc";
    let service_key = format!("{ns}/{svc}");

    let policy_key = format!("{ns}/hc-authority");
    let mut add_policy = HashMap::new();
    add_policy.insert(
        policy_key.clone(),
        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("hc-authority".to_string()),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                prepared_precedence: Some(
                    edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                        creation_timestamp: None,
                    },
                ),
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: Some(String::new()),
                    kind: "Service".to_string(),
                    name: svc.to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                retry_constraint: None,
                load_balancer: None,
                health_check: None,
                outlier_detection: None,
                circuit_breaker: None,
                connection: None,
                upstream_authority: Some(UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: Some("probe.sandbox.internal".to_string()),
                }),
            },
            status: None,
        }),
    );
    get_backend_traffic_policy_store().partial_update_policies(add_policy, HashMap::new(), HashSet::new());

    // `Url::to_string()` normalizes an authority-only URL by appending the
    // root path, so the rewritten host carries a trailing `/`.
    let out = resolve_probe_base("http://10.1.2.3:8080".to_string(), Some(&service_key));
    assert_eq!(out, "http://probe.sandbox.internal:8080/");

    // Cleanup: remove the policy this test inserted so it doesn't leak
    // into other tests running in the same process.
    get_backend_traffic_policy_store().partial_update_policies(
        HashMap::new(),
        HashMap::new(),
        HashSet::from([policy_key]),
    );
}

/// Without an `upstreamAuthority` policy for the service key (or with no
/// service key at all, i.e. a `Static` target), `resolve_probe_base` must
/// leave the probe URL byte-for-byte unchanged.
#[test]
fn resolve_probe_base_unchanged_without_authority() {
    use crate::backends::policy::backend_traffic::store::GLOBAL_STORE_TEST_LOCK;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();

    let out = resolve_probe_base(
        "http://10.1.2.3:8080".to_string(),
        Some("probe-base-authority-test/nonexistent-svc"),
    );
    assert_eq!(out, "http://10.1.2.3:8080");

    let out_static = resolve_probe_base("http://10.1.2.3:8080".to_string(), None);
    assert_eq!(out_static, "http://10.1.2.3:8080");
}

/// Helper: a `WebhookRetry` block with every field set explicitly (no `Default`
/// impl exists), used by the `from_parts` retry-resolution tests below.
fn retry_block(retry_on_timeout: Option<bool>) -> WebhookRetry {
    use edgion_resources::resources::common::external_http::BackoffPolicy;
    WebhookRetry {
        max_retries: 3,
        retry_delay_ms: 100,
        retry_on_timeout,
        retry_on_connect_error: true,
        retry_on_status: vec![],
        backoff_policy: BackoffPolicy::Exponential,
        max_delay_ms: 10_000,
        jitter: false,
        honor_retry_after: false,
        retry_on_body_failure: false,
    }
}

/// Helper: build a valid URL-target `ExternalHttpConnection` carrying the given
/// retry block, for driving `from_parts`'s retry-resolution logic.
fn conn_with_retry(retry: WebhookRetry) -> ExternalHttpConnection {
    ExternalHttpConnection {
        target: WebhookTarget {
            url: Some("https://api.example.com".to_string()),
            ..Default::default()
        },
        retry: Some(retry),
        ..Default::default()
    }
}

/// The actual ForwardAuth variant fills an unset `retry_on_timeout` with `Some(true)` —
/// this is the forward_auth call-site default (idempotent auth checks).
#[test]
fn from_parts_resolves_unset_retry_on_timeout_to_true_default() {
    let conn = conn_with_retry(retry_block(None));
    let client = owned_managed_client("ForwardAuth", serde_json::to_value(&conn).unwrap(), Some("default"))
        .expect("valid URL-target config must build");

    let retry = client.retry.as_ref().expect("retry block must survive from_parts");
    assert_eq!(retry.retry_on_timeout, Some(true));
}

/// The actual Webhook variant fills an unset `retry_on_timeout` with `Some(false)` —
/// this is the webhook call-site default (non-idempotent POSTs).
#[test]
fn from_parts_resolves_unset_retry_on_timeout_to_false_default() {
    let conn = conn_with_retry(retry_block(None));
    let client = owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default")
        .expect("valid URL-target config must build");

    let retry = client.retry.as_ref().expect("retry block must survive from_parts");
    assert_eq!(retry.retry_on_timeout, Some(false));
}

/// An operator's explicit `retry_on_timeout` must survive client construction unchanged —
/// the per-consumer default only fills the gap when the operator leaves it unset.
#[test]
fn from_parts_does_not_clobber_explicit_retry_on_timeout() {
    let conn = conn_with_retry(retry_block(Some(true)));
    // Consumer default is `false`; the explicit `Some(true)` on the resource
    // must win regardless.
    let client = owned_webhook_client(&conn, &OutboundRequestSpec::default(), "default")
        .expect("valid URL-target config must build");

    let retry = client.retry.as_ref().expect("retry block must survive from_parts");
    assert_eq!(retry.retry_on_timeout, Some(true));
}

/// Helper: build a ManagedOutboundClient with an active health check config so
/// that actual Webhook construction spawns a long-running health task.
async fn test_client_with_active_health() -> ManagedOutboundClient {
    use edgion_resources::resources::common::external_http::{
        ActiveHealthCheck, ExternalHttpConnection, WebhookHealthCheck, WebhookServiceRef, WebhookTarget,
    };
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

    let conn = ExternalHttpConnection {
        target: WebhookTarget {
            service: WebhookServiceRef {
                kind: Some("Service".to_string()),
                name: Some("test-svc".to_string()),
                namespace: Some("default".to_string()),
                port: Some(8080),
                ..Default::default()
            },
            ..Default::default()
        },
        health_check: Some(WebhookHealthCheck {
            active: Some(ActiveHealthCheck {
                path: Some("/healthz".to_string()),
                interval_sec: 3600, // very long — task will not exit naturally
                timeout_ms: 2000,
                healthy_threshold: 1,
                unhealthy_threshold: 3,
            }),
            passive: None,
        }),
        ..Default::default()
    };
    let request = OutboundRequestSpec::default();

    owned_webhook_client(&conn, &request, "default").expect("from_parts must succeed for valid config")
}

fn owned_managed_client(
    kind: &str,
    mut connection: serde_json::Value,
    namespace: Option<&str>,
) -> Result<ManagedOutboundClient, String> {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    if connection.get("request").is_none() {
        connection["request"] = serde_json::json!({"path":{"template":"/decision"}});
    }
    if kind == "ForwardAuth" {
        connection["decision"] = serde_json::json!({});
    }
    let config = if kind == "AiGuard" {
        serde_json::json!({"evaluator":connection})
    } else {
        connection
    };
    let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(
        serde_json::json!({"metadata":{"name":"actual"},"spec":{"requestPlugins":[{"type":kind,"config":config}]}}),
    )
    .unwrap();
    owner.metadata.namespace = namespace.map(str::to_owned);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).map_err(|e| e.to_string())?;
    let entry = binding.request_entries().next().unwrap();
    let field = entry.managed_http_config().map_err(|e| e.to_string())?.unwrap();
    let defaults = GlobalOutboundTlsSnapshot::from_configs([]);
    PreparedManagedHttp::new(&field, &defaults)?.build_client()
}

#[test]
fn owning_managed_targets_use_actual_namespace_modes_and_enabled_tls() {
    for kind in ["ForwardAuth", "Guardrail", "AiGuard"] {
        for owner_namespace in [None, Some("tenant")] {
            for target_namespace in [None, Some("shared")] {
                for enabled in [None, Some(false), Some(true)] {
                    for mode in [None, Some("ServiceEndpointSlice"), Some("ServiceClusterIp")] {
                        let mut input = serde_json::json!({"target":{"group":"core","name":"auth","port":8080}});
                        if let Some(namespace) = target_namespace {
                            input["target"]["namespace"] = namespace.into();
                        }
                        if let Some(mode) = mode {
                            input["target"]["kind"] = mode.into();
                        }
                        if let Some(enabled) = enabled {
                            input["tls"] = serde_json::json!({"enabled":enabled});
                        }
                        let client = owned_managed_client(kind, input, owner_namespace).unwrap();
                        let namespace = target_namespace.or(owner_namespace).unwrap_or("default");
                        let scheme = if enabled == Some(true) { "https" } else { "http" };
                        match &client.resolved_target {
                            ResolvedTarget::Service {
                                service_key,
                                scheme: actual,
                                ..
                            } => {
                                assert_eq!(service_key, &format!("{namespace}/auth"));
                                assert_eq!(*actual, scheme);
                            }
                            ResolvedTarget::Static(url) => {
                                assert_eq!(url, &format!("{scheme}://auth.{namespace}.svc.cluster.local:8080"))
                            }
                        }
                        assert!(client.health_task.is_none());
                        assert!(client.egress_policy.is_none());
                        assert_eq!(client.compiled_request.method.is_some(), kind != "ForwardAuth");
                    }
                }
            }
        }
    }
}

#[test]
fn owning_managed_url_tls_and_invalid_targets_fail_before_health_tasks() {
    for kind in ["ForwardAuth", "Guardrail", "AiGuard"] {
        for enabled in [false, true] {
            let client = owned_managed_client(
                kind,
                serde_json::json!({"target":{"url":"https://example.com"},"tls":{"enabled":enabled}}),
                Some("tenant"),
            )
            .unwrap();
            assert!(matches!(&client.resolved_target, ResolvedTarget::Static(url) if url == "https://example.com"));
            assert!(client.egress_policy.is_some());
        }
        for target in [
            serde_json::json!({"name":"auth","port":8080,"namespace":""}),
            serde_json::json!({"name":"auth","port":8080,"group":"other.io"}),
            serde_json::json!({"url":"https://example.com/path"}),
        ] {
            assert!(owned_managed_client(
                kind,
                serde_json::json!({"target":target,"healthCheck":{"active":{}}}),
                Some("tenant")
            )
            .is_err());
        }
        assert!(owned_managed_client(
            kind,
            serde_json::json!({"target":{"name":"auth","port":8080}}),
            Some("")
        )
        .is_err());
        let enabled = serde_json::json!({"target":{"name":"auth","port":8080},"tls":{"enabled":true,"clientCertificateRef":{"name":"identity"}},"healthCheck":{"active":{}}});
        // This synchronous test has no Tokio runtime: invalid proof must return
        // before a configured health task could be spawned.
        assert!(owned_managed_client(kind, enabled, Some("tenant")).is_err());
        let wrong = serde_json::json!({"target":{"name":"auth","port":8080},"tls":{"enabled":true,"clientCertificateRef":{"name":"identity"},"resolvedClientCertificate":{"metadata":{"name":"decoy","namespace":"tenant"},"type":"kubernetes.io/tls","data":{"tls.crt":"ZGVjb3k=","tls.key":"ZGVjb3k="}}},"healthCheck":{"active":{}}});
        assert_eq!(
            owned_managed_client(kind, wrong, Some("tenant")).unwrap_err(),
            "outbound TLS reference/material identity mismatch"
        );
        let disabled = serde_json::json!({"target":{"name":"auth","port":8080},"tls":{"enabled":false,"clientCertificateRef":{"name":"identity"}}});
        let client = owned_managed_client(kind, disabled, Some("tenant")).unwrap();
        assert!(matches!(
            client.resolved_target,
            ResolvedTarget::Service { scheme: "http", .. }
        ));
    }
}

fn owned_route_auth_client(
    mut connection: serde_json::Value,
    namespace: Option<&str>,
    declaring: Option<&str>,
) -> Result<ManagedOutboundClient, String> {
    use edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs;
    connection["decision"] = serde_json::json!({});
    let mut wire = serde_json::json!({"metadata":{"name":"route"},"spec":{"rules":[{"filters":[{"type":"ExternalAuth","externalAuth":connection}]}]}});
    if let Some(namespace) = namespace {
        wire["metadata"]["namespace"] = namespace.into();
    }
    if let Some(declaring) = declaring {
        wire["spec"]["resolvedRules"] = wire["spec"]["rules"].clone();
        wire["spec"]["resolvedRules"][0]["filters"][0]["resolvedExternalAuth"] =
            serde_json::json!({"namespace":declaring});
    }
    let route: edgion_resources::HTTPRoute = serde_json::from_value(wire).unwrap();
    let refs = HttpRouteAuthRefs::from_route(&route).map_err(|error| error.to_string())?;
    let field = refs.effective_at(0, None, 0).map_err(|error| error.to_string())?;
    let defaults = GlobalOutboundTlsSnapshot::from_configs([]);
    PreparedRouteAuth::new(&field, &defaults)?.build_client()
}

#[test]
fn internal_route_auth_service_transport_uses_declaring_locality_and_enabled_tls() {
    for namespace in [None, Some("root")] {
        for declaring in [None, Some("child")] {
            for enabled in [None, Some(false), Some(true)] {
                for kind in ["Service", "ServiceEndpointSlice", "ServiceClusterIp"] {
                    let mut wire = serde_json::json!({"target":{"group":"core","kind":kind,"name":"auth","port":8080}});
                    if let Some(enabled) = enabled {
                        wire["tls"] = serde_json::json!({"enabled":enabled});
                    }
                    let client = owned_route_auth_client(wire, namespace, declaring).unwrap();
                    let expected_ns = declaring.or(namespace).unwrap_or("default");
                    let expected_scheme = if enabled == Some(true) { "https" } else { "http" };
                    match (kind, &client.resolved_target) {
                        (
                            "Service" | "ServiceEndpointSlice",
                            ResolvedTarget::Service {
                                service_key,
                                scheme,
                                port,
                                ..
                            },
                        ) => {
                            assert_eq!(*port, Some(8080));
                            assert_eq!(service_key, &format!("{expected_ns}/auth"));
                            assert_eq!(*scheme, expected_scheme);
                        }
                        ("ServiceClusterIp", ResolvedTarget::Static(url)) => assert_eq!(
                            url,
                            &format!("{expected_scheme}://auth.{expected_ns}.svc.cluster.local:8080")
                        ),
                        _ => panic!("incorrect target transport for {kind}: {:?}", client.resolved_target),
                    }
                    assert!(client.health_task.is_none());
                    assert!(client.egress_policy.is_none());
                }
            }
        }
    }
    let secure = owned_route_auth_client(
        serde_json::json!({"target":{"url":"https://auth.example"},"tls":{"enabled":false}}),
        Some("root"),
        Some("child"),
    )
    .unwrap();
    assert!(matches!(&secure.resolved_target, ResolvedTarget::Static(url) if url == "https://auth.example"));
    assert!(secure.egress_policy.is_some());
}

#[test]
fn internal_route_auth_wrong_material_fails_before_health_preparation_without_tokio() {
    let mut wire = serde_json::json!({"target":{"name":"auth","port":8443},"healthCheck":{"active":{"path":"/health","intervalSec":30}},
    "tls":{"enabled":true,"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":{
        "metadata":{"name":"decoy","namespace":"child"},"type":"kubernetes.io/tls","data":{"tls.crt":"ZGVjb3k=","tls.key":"ZGVjb3k="}}
    }});
    let error = owned_route_auth_client(wire.clone(), Some("root"), Some("child")).unwrap_err();
    assert_eq!(error, "outbound TLS reference/material identity mismatch");
    wire["tls"]["resolvedClientCertificate"]["metadata"]["name"] = "client".into();
    wire["tls"]["resolvedClientCertificate"]["metadata"]["namespace"] = "root".into();
    assert_eq!(
        owned_route_auth_client(wire, Some("root"), Some("child")).unwrap_err(),
        "outbound TLS reference/material identity mismatch"
    );
}

#[test]
fn internal_route_auth_captures_nonempty_global_tls_and_inherits_the_same_generation() {
    use edgion_common::link_sys_core::outbound_tls::resolve_validated_outbound_tls;
    use edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs;
    use serde_json::json;
    let route: edgion_resources::HTTPRoute = serde_json::from_value(json!({"metadata":{"name":"route","namespace":"apps"},"spec":{"rules":[{"filters":[{"type":"ExternalAuth","externalAuth":{
        "target":{"name":"auth","port":443},"decision":{},"tls":{"enabled":true}
    }}]}]}})).unwrap();
    let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
    let field = refs.effective_at(0, None, 0).unwrap();
    let global: edgion_resources::EdgionGatewayConfig =
        serde_json::from_value(json!({"metadata":{"name":"global"},"spec":{"outboundTls":{
            "verify":false
        }}}))
        .unwrap();
    let captured = GlobalOutboundTlsSnapshot::from_configs([Arc::new(global)]);
    let prepared = PreparedRouteAuth::new(&field, &captured).unwrap();
    assert!(std::ptr::eq(prepared.defaults, &captured));
    let resolved = resolve_validated_outbound_tls(&prepared.proof, true, prepared.defaults).unwrap();
    assert!(!resolved.verify);
    let invalid: edgion_resources::EdgionGatewayConfig =
        serde_json::from_value(json!({"metadata":{"name":"next"},"spec":{"outboundTls":{
            "clientCertificateRef":{"name":"missing","namespace":"apps"}
        }}}))
        .unwrap();
    let next = GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]);
    assert!(PreparedRouteAuth::new(&field, &next).is_err());
    let client = prepared.build_client().unwrap();
    assert!(matches!(
        client.resolved_target,
        ResolvedTarget::Service { scheme: "https", .. }
    ));
    let retained = resolve_validated_outbound_tls(&prepared.proof, true, prepared.defaults).unwrap();
    assert!(!retained.verify);
}

#[test]
fn internal_route_auth_requires_exact_template_keys_before_health_preparation() {
    use serde_json::json;
    let wire = json!({"target":{"name":"auth","port":8080},"request":{"path":{"template":"/check/${secretRef:child/token:key}"}}});
    for resolved in [
        serde_json::Value::Null,
        json!({}),
        json!({"root/token:key":"decoy"}),
        json!({"child/token:other":"decoy"}),
    ] {
        let mut missing = wire.clone();
        missing["resolvedSecrets"] = resolved;
        missing["healthCheck"] = json!({"active":{"path":"/health","intervalSec":30}});
        // No Tokio runtime: the missing-key gate precedes any health task spawn.
        assert_eq!(
            owned_route_auth_client(missing, Some("root"), Some("child")).unwrap_err(),
            "HTTPRoute authentication template references are unresolved"
        );
    }
    for value in ["fresh-value", ""] {
        let mut complete = wire.clone();
        complete["resolvedSecrets"] = json!({"child/token:key":value});
        let client = owned_route_auth_client(complete, Some("root"), Some("child")).unwrap();
        assert_eq!(client.resolved_secrets.as_ref().unwrap().0["child/token:key"], value);
    }
}

#[test]
fn managed_template_material_gate_precedes_tls_and_health_for_every_owner() {
    use serde_json::json;
    for kind in ["ForwardAuth", "Guardrail", "AiGuard"] {
        for disabled_tls in [false, true] {
            let mut wire = json!({"target":{"name":"auth","port":8080},
                "request":{"path":{"template":"/check/${secretRef:tenant/token:key}"}}});
            if disabled_tls {
                wire["tls"] = json!({"enabled":false,"clientCertificateRef":{"name":"unused"}});
            }
            for material in [
                serde_json::Value::Null,
                json!({}),
                json!({"other/token:key":"decoy"}),
                json!({"tenant/token:other":"decoy"}),
            ] {
                let mut missing = wire.clone();
                missing["resolvedSecrets"] = material;
                missing["healthCheck"] = json!({"active":{"path":"/health","intervalSec":30}});
                // No Tokio runtime: exact rejection must precede health task creation.
                assert_eq!(
                    owned_managed_client(kind, missing, Some("tenant")).unwrap_err(),
                    "managed HTTP template references are unresolved",
                    "{kind}/{disabled_tls}"
                );
            }
            for value in ["present-value", ""] {
                let mut complete = wire.clone();
                complete["resolvedSecrets"] = json!({"tenant/token:key":value});
                let client = owned_managed_client(kind, complete, Some("tenant")).unwrap();
                assert_eq!(client.resolved_secrets.as_ref().unwrap().0["tenant/token:key"], value);
                assert!(client.health_task.is_none());
            }
        }
    }
}

#[test]
fn managed_template_gate_covers_canonical_surfaces_without_scanning_literal_fields() {
    use serde_json::json;
    let template = "${secretRef:tenant/token:key}";
    let surfaces = [
        ("path", json!({"path":{"template":format!("/{template}")}})),
        (
            "method",
            json!({"path":{"template":"/decision"},"method":{"template":template}}),
        ),
        (
            "args",
            json!({"path":{"template":"/decision"},"args":{"custom":[{"name":"token","template":template}]}}),
        ),
        (
            "headers",
            json!({"path":{"template":"/decision"},"headers":{"custom":[{"name":"x-token","template":template}]}}),
        ),
        (
            "cookies",
            json!({"path":{"template":"/decision"},"cookies":{"custom":[{"name":"token","template":template}]}}),
        ),
        (
            "json",
            json!({"path":{"template":"/decision"},"body":{"type":"json","template":format!("{{\"token\":\"{template}\"}}")}}),
        ),
        (
            "raw",
            json!({"path":{"template":"/decision"},"body":{"type":"raw","template":template}}),
        ),
        (
            "form",
            json!({"path":{"template":"/decision"},"body":{"type":"form","fields":[{"name":"token","template":template}]}}),
        ),
    ];
    for kind in ["ForwardAuth", "Guardrail", "AiGuard"] {
        for (surface, request) in &surfaces {
            if *surface == "method" && kind != "ForwardAuth" {
                continue;
            }
            let mut wire = json!({"target":{"name":"auth","port":8080},"request":request});
            assert_eq!(
                owned_managed_client(kind, wire.clone(), Some("tenant")).unwrap_err(),
                "managed HTTP template references are unresolved",
                "{kind}/{surface}"
            );
            wire["resolvedSecrets"] = json!({"tenant/token:key":"GET"});
            let client = owned_managed_client(kind, wire, Some("tenant")).unwrap();
            assert_eq!(
                client.compiled_request.method.is_some(),
                kind != "ForwardAuth" || *surface == "method"
            );
        }
        let literal = json!({"target":{"name":"auth","port":8080},"request":{
            "path":{"template":"/${ctx:optional}"},"headers":{"custom":[{"name":"x-literal","value":template}]}
        }});
        let client = owned_managed_client(kind, literal, Some("tenant")).unwrap();
        assert!(client.resolved_secrets.is_none());
    }
}

fn owned_webhook_client(
    conn: &ExternalHttpConnection,
    request: &OutboundRequestSpec,
    namespace: &str,
) -> Result<ManagedOutboundClient, String> {
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    let mut config = serde_json::to_value(conn).unwrap();
    config["request"] = serde_json::to_value(request).unwrap();
    let owner = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"webhook","namespace":namespace},
        "spec":{"type":"webhook","config":config}
    }))
    .unwrap();
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).map_err(|error| error.to_string())?;
    let defaults = GlobalOutboundTlsSnapshot::from_configs([]);
    ManagedOutboundClient::from_webhook(&binding, &defaults, "test/webhook")
}
