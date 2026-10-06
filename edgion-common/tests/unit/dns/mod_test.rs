use super::*;

#[test]
fn httpdns_loopback_policy_changes_cache_fingerprint() {
    let mut config: HttpDnsResolverConfig = serde_json::from_value(serde_json::json!({
        "preset": "aliyun", "connection": {"allowLoopback": false}
    }))
    .unwrap();
    let before = ResolverSpec::HttpDns(PreparedHttpDns::prepare(config.clone())).fingerprint();
    config.connection.as_mut().unwrap().allow_loopback = true;
    assert_ne!(
        before,
        ResolverSpec::HttpDns(PreparedHttpDns::prepare(config)).fingerprint()
    );
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
    .fingerprint();
    let dns_b = ResolverSpec::StandardDns {
        servers: vec!["8.8.8.8".to_string()],
    }
    .fingerprint();
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
    let fp = ResolverSpec::HttpDns(PreparedHttpDns::prepare(cfg)).fingerprint();
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
    tls.resolved_ca_certificates = Some(vec![Secret {
        data: Some(std::collections::BTreeMap::from([(
            "tls.ca.crt".into(),
            ByteString(b"first-secret".to_vec()),
        )])),
        ..Default::default()
    }]);
    let first = PreparedHttpDns::prepare(config.clone());
    assert!(first.matches_config(&config));
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
    assert!(!first.matches_config(&config));
    let second = PreparedHttpDns::prepare(config);
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
    use crate::link_sys_core::outbound_tls::{outbound_tls_test_guard, set_outbound_global_default};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _guard = outbound_tls_test_guard();
    let mut config = prepared_config();
    config.connection = Some(serde_json::from_value(serde_json::json!({"tls": {"enabled": true}})).unwrap());
    let first = PreparedHttpDns::prepare(config.clone());
    assert!(first.matches_config(&config));
    set_outbound_global_default(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    assert!(
        !first.matches_config(&config),
        "inherited verification changes require a new generation"
    );
    let second = PreparedHttpDns::prepare(config.clone());
    assert!(second.matches_config(&config));
    assert_eq!(first.address_fingerprint, second.address_fingerprint);
    config.connection.as_mut().unwrap().tls.as_mut().unwrap().enabled = false;
    let disabled = PreparedHttpDns::prepare(config.clone());
    set_outbound_global_default(OutboundTlsConfig::default());
    assert!(
        disabled.matches_config(&config),
        "disabled TLS does not consume inherited trust policy"
    );
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
        let prepared = PreparedHttpDns::prepare(config.clone());
        assert!(!prepared.is_ready());
        assert!(prepared.matches_config(&config));
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
    let prepared = PreparedHttpDns::prepare(config);
    assert!(prepared.is_ready());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    for _ in 0..3 {
        assert!(resolve(&ResolverSpec::HttpDns(prepared.clone()), "example.com")
            .await
            .is_err());
    }
    assert_eq!(count.load(Ordering::Relaxed), 1, "queries must not repeat TLS audit");
}
