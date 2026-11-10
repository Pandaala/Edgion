use super::*;
use edgion_resources::resources::edgion_plugins::plugin_configs::LinkSysRef;
use edgion_resources::resources::link_sys::httpdns::{HttpDnsFallback, HttpDnsResponseFormat};
use edgion_resources::resources::link_sys::HttpDnsResolverConfig;

fn prepare_provider(config: edgion_resources::resources::link_sys::SystemConfig) -> Option<Arc<PreparedHttpDns>> {
    use edgion_resources::resources::link_sys::LinkSysSpec;
    let mut owner = edgion_resources::LinkSys::new(
        "aliyun",
        LinkSysSpec {
            config,
            current_status: None,
            unknown_fields: vec![],
        },
    );
    owner.metadata.namespace = Some("dns".into());
    super::super::HttpDnsCandidate::from_link_sys(Arc::new(owner)).map(super::super::HttpDnsCandidate::prepare)
}

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
fn lookup_aliyun(key: &LinkSysKey) -> Option<Arc<PreparedHttpDns>> {
    if key.namespace == "dns" && key.name == "aliyun" {
        prepare_provider(edgion_resources::resources::link_sys::SystemConfig::HttpDns(
            dummy_httpdns(),
        ))
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
        lookup_family: Default::default(),
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        ..Default::default()
    });
    let spec = resolve_effective_spec(&[service, gateway, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns(servers) => assert_eq!(servers.servers(), vec!["9.9.9.9".to_string()]),
        other => panic!("expected service inline to win, got {other:?}"),
    }
}

#[test]
fn gateway_wins_when_service_absent() {
    let gateway = candidate_from_annotations(&ann(&[(DNS_RESOLVER, "1.1.1.1")]));
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        lookup_family: Default::default(),
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        ..Default::default()
    });
    let spec = resolve_effective_spec(&[None, gateway, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns(servers) => assert_eq!(servers.servers(), vec!["1.1.1.1".to_string()]),
        other => panic!("expected gateway to win, got {other:?}"),
    }
}

#[test]
fn egc_default_wins_when_service_and_gateway_absent() {
    let egc = candidate_from_egc_default(&EgcDnsResolver {
        lookup_family: Default::default(),
        link_sys_ref: None,
        servers: Some(vec!["8.8.8.8".to_string()]),
        ..Default::default()
    });
    let spec = resolve_effective_spec(&[None, None, egc], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns(servers) => assert_eq!(servers.servers(), vec!["8.8.8.8".to_string()]),
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
        lookup_family: Default::default(),
        link_sys_ref: Some(LinkSysRef {
            namespace: "dns".to_string(),
            name: "aliyun".to_string(),
        }),
        servers: None,
        ..Default::default()
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
    let controller_default = Some(ResolverCandidate::Servers(crate::dns::StandardDnsServers::new(vec![
        "8.8.8.8".to_string(),
    ])));
    let spec = resolve_effective_spec(&[service, controller_default], lookup_aliyun);
    match spec {
        ResolverSpec::StandardDns(servers) => assert_eq!(servers.servers(), vec!["9.9.9.9".to_string()]),
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
    let fallback = Some(ResolverCandidate::Servers(crate::dns::StandardDnsServers::new(vec![
        "9.9.9.9".into(),
    ])));
    for config in [
        SystemConfig::HttpDns(dummy_httpdns()),
        SystemConfig::Unsupported,
        SystemConfig::HttpDns(dummy_httpdns()),
    ] {
        // The actual provider candidate must not recreate a retired HTTPDNS handle.
        let spec = resolve_effective_spec(&[reference.clone(), fallback.clone()], |_| {
            prepare_provider(config.clone())
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
        resolve_effective_spec(&[reference], |_| prepare_provider(SystemConfig::Unsupported)),
        ResolverSpec::System
    ));
}

#[test]
fn dns_key_preserves_finite_annotation_grammar_without_dns_label_tightening() {
    let key = LinkSysKey::parse(" namespace / Name_With_Underscore ").unwrap();
    assert_eq!(key.namespace, "namespace");
    assert_eq!(key.name, "Name_With_Underscore");
    for value in ["bare", " /name", "ns/", "ns/a/b"] {
        assert!(LinkSysKey::parse(value).is_none());
    }
}

// Historical selection fixtures now enter through actual owning resources.
use edgion_resources::constants::annotations::edgion::{DNS_RESOLVER, DNS_RESOLVER_REF};
use edgion_resources::resources::edgion_gateway_config::DnsResolverConfig as EgcDnsResolver;
use std::collections::BTreeMap;
fn candidate_from_annotations(annotations: &BTreeMap<String, String>) -> Option<ResolverCandidate> {
    let owner = k8s_openapi::api::core::v1::Service {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("source".into()),
            annotations: Some(annotations.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    candidate_from_dns_owner(DnsResolverOwner::Service(&owner))
}
fn candidate_from_egc_default(config: &EgcDnsResolver) -> Option<ResolverCandidate> {
    let mut owner = edgion_resources::EdgionGatewayConfig::new("source", Default::default());
    owner.spec.dns_resolver = Some(config.clone());
    super::candidate_from_egc_default(&owner)
}
fn resolve_effective_spec<F>(layers: &[Option<ResolverCandidate>], lookup: F) -> ResolverSpec
where
    F: Fn(&LinkSysKey) -> Option<Arc<PreparedHttpDns>>,
{
    super::resolve_effective_spec(&layers.iter().map(Option::as_ref).collect::<Vec<_>>(), lookup)
}

#[test]
fn owning_candidate_reference_failures_preserve_only_independent_inline_settings() {
    let mut service = k8s_openapi::api::core::v1::Service::default();
    service.metadata.annotations = Some(ann(&[(DNS_RESOLVER_REF, "dns/resolver"), (DNS_RESOLVER, "192.0.2.53")]));
    assert_eq!(
        candidate_from_dns_owner(DnsResolverOwner::Service(&service)),
        Some(ResolverCandidate::Servers(crate::dns::StandardDnsServers::new(vec![
            "192.0.2.53".into()
        ])))
    );
    service.metadata.name = Some("source".into());
    assert!(matches!(
        candidate_from_dns_owner(DnsResolverOwner::Service(&service)),
        Some(ResolverCandidate::HttpDnsRef(_))
    ));
    let mut egc = edgion_resources::EdgionGatewayConfig::new("source", Default::default());
    egc.spec.dns_resolver = Some(EgcDnsResolver {
        lookup_family: Default::default(),
        link_sys_ref: Some(LinkSysRef {
            namespace: "dns".into(),
            name: "resolver".into(),
        }),
        servers: Some(vec!["192.0.2.53".into()]),
        ..Default::default()
    });
    egc.metadata.namespace = Some("".into());
    assert!(
        super::candidate_from_egc_default(&egc).is_none(),
        "invalid ref owner cannot use same-layer inline"
    );
    egc.spec.dns_resolver.as_mut().unwrap().link_sys_ref = None;
    assert_eq!(
        super::candidate_from_egc_default(&egc),
        Some(ResolverCandidate::Servers(crate::dns::StandardDnsServers::new(vec![
            "192.0.2.53".into()
        ])))
    );
}
