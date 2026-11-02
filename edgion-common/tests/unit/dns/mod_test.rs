use super::*;

fn owner(config: HttpDnsResolverConfig) -> Arc<edgion_resources::LinkSys> {
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    let mut owner = edgion_resources::LinkSys::new(
        "resolver",
        LinkSysSpec {
            config: SystemConfig::HttpDns(config),
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("apps".into());
    Arc::new(owner)
}
fn candidate(config: &HttpDnsResolverConfig) -> HttpDnsCandidate {
    HttpDnsCandidate::from_link_sys(owner(config.clone())).unwrap()
}
fn candidate_with_defaults(
    config: &HttpDnsResolverConfig,
    defaults: &Arc<crate::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> HttpDnsCandidate {
    HttpDnsCandidate::with_snapshot(owner(config.clone()), Arc::clone(defaults)).unwrap()
}
fn prepare(config: HttpDnsResolverConfig) -> Arc<PreparedHttpDns> {
    HttpDnsCandidate::from_link_sys(owner(config)).unwrap().prepare()
}
fn prepare_with_defaults(
    config: HttpDnsResolverConfig,
    defaults: Arc<crate::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> Arc<PreparedHttpDns> {
    HttpDnsCandidate::with_snapshot(owner(config), defaults)
        .unwrap()
        .prepare()
}

#[test]
fn httpdns_loopback_policy_changes_cache_fingerprint() {
    let mut config: HttpDnsResolverConfig = serde_json::from_value(serde_json::json!({
        "preset": "aliyun", "connection": {"allowLoopback": false}
    }))
    .unwrap();
    let before = ResolverSpec::HttpDns(prepare(config.clone()))
        .fingerprint()
        .into_owned();
    config.connection.as_mut().unwrap().allow_loopback = true;
    assert_ne!(before, ResolverSpec::HttpDns(prepare(config)).fingerprint());
}

#[tokio::test]
async fn system_spec_resolves_localhost() {
    let addrs = resolve(&ResolverSpec::System, "localhost").await.expect("resolves");
    assert!(addrs.iter().any(|ip| ip.is_loopback()));
}

#[test]
fn fingerprint_distinguishes_resolvers() {
    let system = ResolverSpec::System.fingerprint();
    let dns_a = ResolverSpec::StandardDns {
        servers: vec!["1.1.1.1".to_string()],
    }
    .fingerprint()
    .into_owned();
    let dns_b = ResolverSpec::StandardDns {
        servers: vec!["8.8.8.8".to_string()],
    }
    .fingerprint()
    .into_owned();
    assert_ne!(system, dns_a);
    assert_ne!(dns_a, dns_b, "different server lists must fingerprint differently");
    // Same servers → same fingerprint (cache hit across requests).
    assert_eq!(
        dns_a,
        ResolverSpec::StandardDns {
            servers: vec!["1.1.1.1".to_string()]
        }
        .fingerprint()
    );
}

#[test]
fn httpdns_fingerprint_redacts_resolved_secret_material() {
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsConnection, HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };
    use k8s_openapi::api::core::v1::Secret;
    use std::collections::BTreeMap;

    const SENTINEL: &str = "SUPER_SECRET_KEY_MATERIAL";
    let mut string_data = BTreeMap::new();
    string_data.insert("tls.key".to_string(), SENTINEL.to_string());
    let secret = Secret {
        string_data: Some(string_data),
        ..Default::default()
    };
    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(),
        resolved_ca_certificates: None,
        resolved_client_certificate: Some(secret),
    };
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: Some(HttpDnsConnection {
            allow_loopback: false,
            tls: Some(tls),
            timeout_ms: None,
            ..Default::default()
        }),
    };
    let fp = ResolverSpec::HttpDns(prepare(cfg)).fingerprint().into_owned();
    assert!(
        !fp.contains(SENTINEL),
        "resolved secret material must not appear in the cache fingerprint: {fp}"
    );
    assert!(
        fp.contains("[redacted]"),
        "expected redaction marker in fingerprint: {fp}"
    );
}

#[test]
fn strip_brackets_removes_matching_pair_only() {
    assert_eq!(strip_brackets("[::1]"), "::1");
    assert_eq!(strip_brackets("[2001:db8::1]"), "2001:db8::1");
    assert_eq!(strip_brackets("::1"), "::1");
    assert_eq!(strip_brackets("[::1"), "[::1");
}

#[test]
fn actual_httpdns_source_and_active_pairing_proof_precede_equal_policy_reuse() {
    let _guard = crate::link_sys_core::outbound_tls::outbound_tls_test_guard();
    let original = owner(prepared_config());
    let ready = HttpDnsCandidate::from_link_sys(Arc::clone(&original))
        .unwrap()
        .prepare();
    assert!(ready.is_ready());
    let mut malformed = (*original).clone();
    malformed.metadata.name = None;
    let malformed = Arc::new(malformed);
    let incoming = HttpDnsCandidate::from_link_sys(Arc::clone(&malformed)).unwrap();
    assert!(!incoming.matches(&ready));
    assert!(http_fetcher::ReqwestHttpDnsFetcher::new(Arc::clone(&malformed)).is_err());
    let failed = incoming.prepare();
    assert!(!failed.is_ready());
    assert!(HttpDnsCandidate::from_link_sys(malformed).unwrap().matches(&failed));
    assert!(!HttpDnsCandidate::from_link_sys(Arc::clone(&original))
        .unwrap()
        .matches(&failed));
    assert!(HttpDnsCandidate::from_link_sys(original).unwrap().prepare().is_ready());
    let mut config = prepared_config();
    config.connection = Some(
        serde_json::from_value(serde_json::json!({"tls":{
            "enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"other","namespace":"apps"}}]
        }}))
        .unwrap(),
    );
    assert!(
        !prepare(config.clone()).is_ready(),
        "verify=false cannot mask wrong material identity"
    );
    config.connection.as_mut().unwrap().tls.as_mut().unwrap().enabled = false;
    assert!(
        prepare(config).is_ready(),
        "disabled TLS keeps native defaults and ignores dormant material"
    );
}

#[test]
fn direct_fetcher_checks_full_actual_provider_and_configuration() {
    let mut wrong_provider = (*owner(prepared_config())).clone();
    wrong_provider.spec.config = edgion_resources::resources::link_sys::SystemConfig::Unsupported;
    let wrong_provider = Arc::new(wrong_provider);
    assert!(HttpDnsCandidate::from_link_sys(Arc::clone(&wrong_provider)).is_none());
    assert!(http_fetcher::ReqwestHttpDnsFetcher::new(wrong_provider).is_err());
    let mut config = prepared_config();
    config.preset = None;
    config.url_template = None;
    let malformed = owner(config);
    assert!(http_fetcher::ReqwestHttpDnsFetcher::new(Arc::clone(&malformed)).is_err());
    assert!(!HttpDnsCandidate::from_link_sys(malformed).unwrap().prepare().is_ready());
}

fn prepared_config() -> HttpDnsResolverConfig {
    serde_json::from_value(serde_json::json!({
        "urlTemplate": "http://invalid.invalid/d?host={domain}",
        "response": {"kind": "json", "ipPath": "ips"},
        "fallback": {"type": "none"}
    }))
    .unwrap()
}

#[test]
fn prepared_secret_rotation_changes_private_identity_only_and_debug_is_safe() {
    use k8s_openapi::{api::core::v1::Secret, ByteString};
    let _guard = crate::link_sys_core::outbound_tls::outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"tls": {"enabled": true}})).unwrap());
    let tls = config.connection.as_mut().unwrap().tls.as_mut().unwrap();
    tls.outbound.verify = Some(false);
    tls.outbound.validation = Some(
        serde_json::from_value(serde_json::json!({
            "caCertificateRefs":[{"kind":"Secret","name":"ca"}]
        }))
        .unwrap(),
    );
    tls.resolved_ca_certificates = Some(vec![Secret {
        metadata: serde_json::from_value(serde_json::json!({"name":"ca","namespace":"apps"})).unwrap(),
        data: Some(std::collections::BTreeMap::from([(
            "tls.ca.crt".into(),
            ByteString(b"first-secret".to_vec()),
        )])),
        ..Default::default()
    }]);
    let first = prepare(config.clone());
    assert!(first.is_ready());
    assert!(candidate(&config).matches(&first));
    config
        .connection
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .resolved_ca_certificates
        .as_mut()
        .unwrap()[0]
        .data
        .as_mut()
        .unwrap()
        .insert("tls.ca.crt".into(), ByteString(b"rotated-secret".to_vec()));
    assert!(!candidate(&config).matches(&first));
    let second = prepare(config);
    assert_eq!(first.address_fingerprint, second.address_fingerprint);
    assert!(
        first.config.connection.is_none(),
        "prepared queries must not retain TLS material"
    );
    let debug = format!("{first:?}");
    for private in ["first-secret", "invalid.invalid", "http://", "httpdns:"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn inherited_tls_changes_revalidate_prepared_runtime_but_disabled_tls_is_inert() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    use crate::link_sys_core::outbound_tls::{outbound_tls_test_guard, set_outbound_global_default};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _guard = outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"tls": {"enabled": true}})).unwrap());
    let first = prepare(config.clone());
    assert!(candidate(&config).matches(&first));
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            verify: false,
            ..Default::default()
        },
    ));
    assert!(
        !candidate(&config).matches(&first),
        "inherited verification changes require a new generation"
    );
    let second = prepare(config.clone());
    assert!(candidate(&config).matches(&second));
    assert_eq!(first.address_fingerprint, second.address_fingerprint);
    config.connection.as_mut().unwrap().tls.as_mut().unwrap().enabled = false;
    let disabled = prepare(config.clone());
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    assert!(
        candidate(&config).matches(&disabled),
        "disabled TLS does not consume inherited trust policy"
    );
}

#[test]
fn captured_global_generation_drives_httpdns_identity_and_construction() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    use crate::link_sys_core::outbound_tls::{outbound_tls_test_guard, set_outbound_global_default, test_util};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _guard = outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"tls":{"enabled":true}})).unwrap());
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    set_outbound_global_default(Arc::clone(&b));
    let prepared = prepare_with_defaults(config.clone(), Arc::clone(&a));
    assert!(prepared.is_ready());
    assert_eq!(
        prepared.runtime_identity,
        candidate_with_defaults(&config, &a).runtime_identity
    );
    assert_ne!(
        prepared.runtime_identity,
        candidate_with_defaults(&config, &b).runtime_identity
    );
    assert!(candidate_with_defaults(&config, &a).matches(&prepared));
    assert!(!candidate_with_defaults(&config, &b).matches(&prepared));
    let next = prepare(config.clone());
    assert!(next.is_ready() && candidate_with_defaults(&config, &b).matches(&next));
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"invalid","namespace":"apps"},"spec":{}
    }))
    .unwrap();
    let invalid =
        Arc::new(crate::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]));
    set_outbound_global_default(Arc::clone(&invalid));
    let captured = prepare_with_defaults(config.clone(), Arc::clone(&a));
    assert!(
        captured.is_ready(),
        "construction must use valid captured A despite invalid current default"
    );
    assert_eq!(
        captured.runtime_identity,
        candidate_with_defaults(&config, &a).runtime_identity
    );
    assert!(
        !prepare(config).is_ready(),
        "new capture must consume invalid current source"
    );
}

#[test]
fn masked_global_axes_keep_the_ready_httpdns_generation() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    use crate::link_sys_core::outbound_tls::{outbound_tls_test_guard, test_util};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _guard = outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(
        serde_json::from_value(serde_json::json!({
            "tls":{"enabled":true,"verify":false,"validation":{}}
        }))
        .unwrap(),
    );
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(
        serde_json::from_value(serde_json::json!({
            "verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"missing","namespace":"apps"}]}
        }))
        .unwrap(),
    );
    let ready = prepare_with_defaults(config.clone(), Arc::clone(&a));
    assert!(ready.is_ready());
    let next = candidate_with_defaults(&config, &b);
    assert!(
        next.matches(&ready),
        "overridden verify and CA must not disturb a ready generation"
    );
    assert_eq!(next.runtime_identity, ready.runtime_identity);
    config
        .connection
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .outbound
        .validation = None;
    assert!(!candidate_with_defaults(&config, &b).matches(&ready));
    assert!(
        !prepare_with_defaults(config, b).is_ready(),
        "unmasked missing CA must fail"
    );
}

#[test]
fn malformed_global_owner_cannot_reuse_equal_policy_or_construct_httpdns_client() {
    use crate::link_sys_core::outbound_tls::{outbound_tls_test_guard, GlobalOutboundTlsSnapshot};
    let _guard = outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"tls":{"enabled":true}})).unwrap());
    let valid: edgion_resources::EdgionGatewayConfig =
        serde_json::from_value(serde_json::json!({"metadata":{"name":"actual"},"spec":{}})).unwrap();
    let mut invalid = valid.clone();
    invalid.metadata.namespace = Some("apps".into());
    let a = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(valid)]));
    let b = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]));
    assert_eq!(a.verify(), b.verify());
    assert_ne!(
        candidate_with_defaults(&config, &a).runtime_identity,
        candidate_with_defaults(&config, &b).runtime_identity
    );
    let first = prepare_with_defaults(config.clone(), Arc::clone(&a));
    assert!(first.is_ready() && !candidate_with_defaults(&config, &b).matches(&first));
    let failed = prepare_with_defaults(config.clone(), Arc::clone(&b));
    assert!(!failed.is_ready());
    assert!(
        candidate_with_defaults(&config, &b).matches(&failed),
        "unchanged failure retains its configured fallback"
    );
    assert!(
        !candidate_with_defaults(&config, &a).matches(&failed),
        "repaired authority must trigger recovery"
    );
    let recovered = prepare_with_defaults(config.clone(), Arc::clone(&a));
    assert!(recovered.is_ready());
    assert_eq!(failed.address_fingerprint, first.address_fingerprint);
    config
        .connection
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .outbound
        .verify = Some(true);
    let overridden = prepare_with_defaults(config.clone(), Arc::clone(&b));
    assert!(overridden.is_ready() && candidate_with_defaults(&config, &b).matches(&overridden));
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_preparation_retains_latest_fallback_without_query_preparation() {
    use edgion_resources::resources::link_sys::httpdns::HttpDnsFallback;
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"timeoutMs": 0})).unwrap());
    for fallback in [
        HttpDnsFallback::None,
        HttpDnsFallback::System,
        HttpDnsFallback::Dns { servers: vec![] },
    ] {
        config.fallback = fallback.clone();
        let prepared = prepare(config.clone());
        assert!(!prepared.is_ready());
        assert!(candidate(&config).matches(&prepared));
        let spec = ResolverSpec::HttpDns(prepared.clone());
        for _ in 0..2 {
            let result = resolve(&spec, "127.0.0.1").await;
            if matches!(fallback, HttpDnsFallback::System) {
                assert!(result.unwrap().iter().any(|ip| ip.is_loopback()));
            } else {
                assert!(result.is_err());
            }
            assert!(prepared.fetcher.is_none());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn tls_audit_occurs_only_during_preparation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct AuditCounter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> Layer<S> for AuditCounter {
        fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            if event.metadata().target() == "edgion_common::link_sys_core::outbound_tls"
                && *event.metadata().level() == tracing::Level::WARN
            {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let _policy_guard = crate::link_sys_core::outbound_tls::outbound_tls_test_guard();
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(AuditCounter(count.clone()));
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);
    let mut config = prepared_config();
    config.url_template = Some("http://169.254.169.254/?host={domain}".into());
    config.connection =
        Some(serde_json::from_value(serde_json::json!({"tls": {"enabled": true, "verify": false}})).unwrap());
    let prepared = prepare(config.clone());
    assert!(prepared.is_ready());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    for _ in 0..3 {
        assert!(candidate(&config).matches(&prepared));
        assert_eq!(count.load(Ordering::Relaxed), 1, "comparison must not audit TLS");
        assert!(resolve(&ResolverSpec::HttpDns(prepared.clone()), "example.com")
            .await
            .is_err());
    }
    assert_eq!(count.load(Ordering::Relaxed), 1, "queries must not repeat TLS audit");
}

#[test]
fn fingerprints_borrow_fixed_and_prepared_identity() {
    assert!(matches!(ResolverSpec::System.fingerprint(), Cow::Borrowed("system")));
    let prepared = prepare(prepared_config());
    let spec = ResolverSpec::HttpDns(Arc::clone(&prepared));
    let fingerprint = spec.fingerprint();
    let Cow::Borrowed(value) = fingerprint else {
        panic!("prepared address identity must be borrowed");
    };
    assert_eq!(value, prepared.address_fingerprint);
    assert_eq!(value.as_ptr(), prepared.address_fingerprint.as_ptr());

    let cases: &[(&[&str], &str)] = &[
        (&[], "std:"),
        (&[""], "std:"),
        (&["", ""], "std:,"),
        (&["192.0.2.53", "[2001:db8::53]:53"], "std:192.0.2.53,[2001:db8::53]:53"),
        (
            &["192.0.2.53:5353", "192.0.2.53:5353"],
            "std:192.0.2.53:5353,192.0.2.53:5353",
        ),
        (&["", "192.0.2.53", ""], "std:,192.0.2.53,"),
        (&["first,second", "third"], "std:first,second,third"),
        (&["first", "second,third"], "std:first,second,third"),
        (&["é", "π"], "std:é,π"),
    ];
    for (servers, literal) in cases {
        let servers: Vec<String> = servers.iter().map(|server| (*server).to_string()).collect();
        let expected = format!("std:{}", servers.join(","));
        let spec = ResolverSpec::StandardDns { servers };
        let fingerprint = spec.fingerprint();
        assert!(matches!(fingerprint, Cow::Owned(_)));
        assert_eq!(fingerprint, *literal);
        assert_eq!(fingerprint, expected);
    }
}
