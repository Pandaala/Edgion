use super::*;
use std::collections::BTreeMap;

#[test]
fn dns_grammar_and_actual_owner_sources_remain_distinct() {
    for (value, expected) in [
        (" ns / Name_With.Underscore ", Some(("ns", "Name_With.Underscore"))),
        ("ns/name", Some(("ns", "name"))),
        ("name", None),
        (" /name", None),
        ("ns/ ", None),
        ("ns/a/b", None),
    ] {
        assert_eq!(DnsLinkSysKey::parse(value).map(|k| (k.namespace(), k.name())), expected);
    }
    let mut service = Service {
        metadata: ObjectMeta {
            name: Some("source".into()),
            annotations: Some(BTreeMap::from([(DNS_RESOLVER_REF.into(), "dns/name".into())])),
            ..Default::default()
        },
        ..Default::default()
    };
    let query = DnsResolverOwner::Service(&service).query().unwrap().unwrap();
    assert_eq!(query.source().group_kind().group(), "");
    assert_eq!(query.source().group_kind().kind(), "Service");
    assert_eq!(query.source().scope().namespace(), Some("default"));
    let gateway:Gateway=serde_json::from_value(serde_json::json!({"metadata":{"name":"source","annotations":{DNS_RESOLVER_REF:"dns/name"}},"spec":{"gatewayClassName":"test","listeners":[]}})).unwrap();
    let query = DnsResolverOwner::Gateway(&gateway).query().unwrap().unwrap();
    assert_eq!(query.source().group_kind().group(), "gateway.networking.k8s.io");
    assert_eq!(query.source().group_kind().kind(), "Gateway");
    service.metadata.namespace = Some("".into());
    assert_eq!(
        DnsResolverOwner::Service(&service).query(),
        Err(ReferenceError::InvalidNamespace)
    );
    service.metadata.namespace = None;
    service.metadata.name = None;
    assert_eq!(
        DnsResolverOwner::Service(&service).query(),
        Err(ReferenceError::InvalidName)
    );
}

#[test]
fn egc_dns_reference_checks_actual_cluster_owner_and_typed_explicit_target() {
    let mut owner = EdgionGatewayConfig::new("source", Default::default());
    owner.spec.dns_resolver =
        Some(serde_json::from_value(serde_json::json!({"linkSysRef":{"namespace":"dns","name":"resolver"}})).unwrap());
    assert_eq!(
        EgcDnsResolverField::new(&owner).target().unwrap().unwrap().name(),
        "resolver"
    );
    for (namespace, name) in [
        ("", "resolver"),
        (" dns ", "resolver"),
        ("dns", "Name_With_Underscore"),
        ("dns", "a/b"),
        ("dns", ""),
    ] {
        let field = owner.spec.dns_resolver.as_mut().unwrap().link_sys_ref.as_mut().unwrap();
        field.namespace = namespace.into();
        field.name = name.into();
        assert!(EgcDnsResolverField::new(&owner).target().is_err());
    }
    let field = owner.spec.dns_resolver.as_mut().unwrap().link_sys_ref.as_mut().unwrap();
    field.namespace = "dns".into();
    field.name = "resolver".into();
    for namespace in ["", "default"] {
        owner.metadata.namespace = Some(namespace.into());
        assert_eq!(
            EgcDnsResolverField::new(&owner).target(),
            Err(ReferenceError::InvalidNamespace)
        );
    }
    owner.metadata.namespace = None;
    owner.metadata.name = None;
    assert_eq!(
        EgcDnsResolverField::new(&owner).target(),
        Err(ReferenceError::InvalidName)
    );
    owner.spec.dns_resolver.as_mut().unwrap().link_sys_ref = None;
    assert_eq!(
        EgcDnsResolverField::new(&owner).target(),
        Ok(None),
        "absent ref does not validate whole owner"
    );
}
