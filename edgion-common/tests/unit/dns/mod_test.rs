use super::*;

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
            tls: Some(tls),
            timeout_ms: None,
        }),
    };
    let fp = ResolverSpec::HttpDns(Box::new(cfg)).fingerprint();
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
