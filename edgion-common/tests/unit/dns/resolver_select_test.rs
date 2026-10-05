use super::*;
use edgion_resources::resources::edgion_plugins::plugin_configs::LinkSysRef;
use edgion_resources::resources::link_sys::httpdns::{HttpDnsFallback, HttpDnsResponseFormat};

fn ann(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn dummy_httpdns() -> HttpDnsResolverConfig {
    HttpDnsResolverConfig {
        preset: None,
        url_template: Some("https://h/d?host={domain}".to_string()),
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: None,
    }
}

/// lookup that resolves only the key `dns/aliyun`.
fn lookup_aliyun(key: &LinkSysKey) -> Option<HttpDnsResolverConfig> {
    if key.namespace == "dns" && key.name == "aliyun" {
        Some(dummy_httpdns())
    } else {
        None
    }
}

// ── LinkSysKey::parse ─────────────────────────────────────────────────────

#[test]
fn linksys_key_parses_ns_name() {
    assert_eq!(
        LinkSysKey::parse("dns/aliyun"),
        Some(LinkSysKey {
            namespace: "dns".to_string(),
            name: "aliyun".to_string()
        })
    );
}

#[test]
fn linksys_key_rejects_bare_name_and_malformed() {
    assert_eq!(LinkSysKey::parse("aliyun"), None);
    assert_eq!(LinkSysKey::parse("/aliyun"), None);
    assert_eq!(LinkSysKey::parse("dns/"), None);
    assert_eq!(LinkSysKey::parse("a/b/c"), None);
    assert_eq!(LinkSysKey::parse(""), None);
}

// ── parse_dns_server_list ─────────────────────────────────────────────────

#[test]
fn parse_server_list_does_not_prefix_bare_ips() {
    let got = parse_dns_server_list("1.1.1.1, 8.8.8.8:5353 1.1.1.1");
    assert_eq!(got, vec!["1.1.1.1".to_string(), "8.8.8.8:5353".to_string()]);
}

// ── precedence ────────────────────────────────────────────────────────────

#[test]
fn nothing_configured_returns_system() {
    let spec = resolve_effective_spec(&[None, None, None], lookup_aliyun);
    assert!(matches!(spec, ResolverSpec::System));
}

#[test]
fn service_inline_wins_over_gateway_and_egc() {
    let service = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "9.9.9.9")]));
    let gateway = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "1.1.1.1")]));
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        cache_ttl: "5s".to_string(),
    });
    let spec = resolve_effective_spec(&[service, gateway, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns { servers } => assert_eq!(servers, vec!["9.9.9.9".to_string()]),
        other => panic!("expected service inline to win, got {other:?}"),
    }
}

#[test]
fn gateway_wins_when_service_absent() {
    let gateway = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "1.1.1.1")]));
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        cache_ttl: "5s".to_string(),
    });
    let spec = resolve_effective_spec(&[None, gateway, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns { servers } => assert_eq!(servers, vec!["1.1.1.1".to_string()]),
        other => panic!("expected gateway to win, got {other:?}"),
    }
}

#[test]
fn egc_default_wins_when_service_and_gateway_absent() {
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        cache_ttl: "5s".to_string(),
    });
    let spec = resolve_effective_spec(&[None, None, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns { servers } => assert_eq!(servers, vec!["8.8.8.8".to_string()]),
        other => panic!("expected EGC default, got {other:?}"),
    }
}

#[test]
fn httpdns_ref_resolves_via_lookup() {
    let service = candidate_from_annotations(&ann(&[(DNS_RESOLVER_REF, "dns/aliyun")]));
    let spec = resolve_effective_spec(&[service, None, None], lookup_aliyun);
    assert!(matches!(spec, ResolverSpec::HttpDns(_)));
}

#[test]
fn dangling_httpdns_ref_fails_safe_to_system() {
    let service = candidate_from_annotations(&ann(&[(DNS_RESOLVER_REF, "dns/missing")]));
    let spec = resolve_effective_spec(&[service, None, None], lookup_aliyun);
    assert!(
        matches!(spec, ResolverSpec::System),
        "dangling ref must not fall through"
    );
}

#[test]
fn ref_beats_servers_on_the_same_layer() {
    let both = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "1.1.1.1"), (DNS_RESOLVER_REF, "dns/aliyun")]));
    let spec = resolve_effective_spec(&[both, None, None], lookup_aliyun);
    assert!(matches!(spec, ResolverSpec::HttpDns(_)));
}

#[test]
fn egc_link_sys_ref_candidate_built_from_struct() {
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        link_sys_ref: Some(LinkSysRef {
            namespace: "dns".to_string(),
            name: "aliyun".to_string(),
        }),
        servers: None,
        cache_ttl: "5s".to_string(),
    });
    assert_eq!(
        egc,
        Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".to_string(),
            name: "aliyun".to_string()
        }))
    );
}

// ── control-plane shape (Service → Controller default) ────────────────────

#[test]
fn control_plane_service_over_controller_default() {
    // The control plane assembles just [service, controller_default].
    let service = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "9.9.9.9")]));
    let controller_default = Some(ResolverCandidate::Servers(vec!["8.8.8.8".to_string()]));
    let spec = resolve_effective_spec(&[service, controller_default], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns { servers } => assert_eq!(servers, vec!["9.9.9.9".to_string()]),
        other => panic!("expected service to win control-plane, got {other:?}"),
    }
}

#[test]
fn unsupported_linksys_does_not_reuse_httpdns_and_falls_back_to_system() {
    use edgion_resources::resources::link_sys::SystemConfig;
    let reference = Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
        namespace: "dns".into(),
        name: "resolver".into(),
    }));
    let fallback = Some(ResolverCandidate::Servers(vec!["9.9.9.9".into()]));
    for config in [
        SystemConfig::HttpDns(dummy_httpdns()),
        SystemConfig::Unsupported,
        SystemConfig::HttpDns(dummy_httpdns()),
    ] {
        // This is the same typed projection used by Gateway's raw LinkSys lookup.
        let spec = resolve_effective_spec(&[reference.clone(), fallback.clone()], |_| {
            config.as_http_dns().cloned()
        });
        match config {
            SystemConfig::Unsupported => {
                assert!(matches!(spec, ResolverSpec::System))
            }
            SystemConfig::HttpDns(_) => assert!(matches!(spec, ResolverSpec::HttpDns(_))),
            _ => unreachable!(),
        }
    }
    assert!(matches!(
        resolve_effective_spec(&[reference], |_| SystemConfig::Unsupported.as_http_dns().cloned()),
        ResolverSpec::System
    ));
}
